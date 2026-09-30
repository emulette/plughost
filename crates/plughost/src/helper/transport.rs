//! Request and response I/O on the supervising thread itself. Every read and write stops at its
//! deadline and keeps its place in the frame, so the supervisor enforces deadlines and polls
//! cancellation without handing messages to other threads. Windows named pipes have no read
//! timeout, so they use overlapped I/O; Unix sockets are polled.

use std::io;
use std::time::{Duration, Instant};

use interprocess::local_socket::Stream;
use plughost_core::ipc::{FrameDecoder, Hello, Request, Response, encode_message};

pub(super) enum Incoming {
    Hello(Hello),
    Response(Response),
}

/// Why a read or write ended without its message.
#[derive(Debug)]
pub(super) enum Stopped {
    /// The deadline passed; the operation can be continued.
    TimedOut,
    /// The connection closed or failed, or the transport was closed.
    Closed,
}

pub(super) struct Transport {
    pipe: Option<pipe::Pipe>,
    decoder: FrameDecoder,
    greeted: bool,
    /// The frame being written, and how much of it was written.
    outgoing: Vec<u8>,
    written: usize,
}

impl Transport {
    pub fn new(stream: Stream) -> io::Result<Self> {
        Ok(Self {
            pipe: Some(pipe::Pipe::new(stream)?),
            decoder: FrameDecoder::default(),
            greeted: false,
            outgoing: Vec::new(),
            written: 0,
        })
    }

    /// Starts writing `request`; [`Transport::flush`] writes it. The previous request must have
    /// been flushed.
    pub fn queue(&mut self, request: &Request) -> Result<(), Stopped> {
        if self.pipe.is_none() || self.written < self.outgoing.len() {
            return Err(Stopped::Closed);
        }
        encode_message(&mut self.outgoing, request).map_err(|_| Stopped::Closed)?;
        self.written = 0;
        Ok(())
    }

    /// Writes the rest of the queued request. A request that times out part-written leaves the
    /// stream mid-frame: continue it, or close the connection.
    pub fn flush(&mut self, timeout: Duration) -> Result<(), Stopped> {
        let deadline = Instant::now() + timeout;
        let pipe = self.pipe.as_mut().ok_or(Stopped::Closed)?;
        while self.written < self.outgoing.len() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match pipe.write(&self.outgoing[self.written..], remaining) {
                Ok(Some(0)) | Err(_) => return Err(Stopped::Closed),
                Ok(Some(count)) => self.written += count,
                Ok(None) => return Err(Stopped::TimedOut),
            }
        }
        Ok(())
    }

    /// Reads the next message. The first message is the helper's hello; a read that times out
    /// part-way keeps the partial frame for the next call. A malformed frame is `InvalidData`.
    pub fn receive(&mut self, timeout: Duration) -> Result<io::Result<Incoming>, Stopped> {
        let deadline = Instant::now() + timeout;
        let pipe = self.pipe.as_mut().ok_or(Stopped::Closed)?;
        loop {
            if self.greeted {
                if let Some(message) = self.decoder.message::<Response>() {
                    return Ok(message.map(Incoming::Response));
                }
            } else if let Some(message) = self.decoder.message::<Hello>() {
                self.greeted = message.is_ok();
                return Ok(message.map(Incoming::Hello));
            }
            let unfilled = match self.decoder.unfilled() {
                Ok(unfilled) => unfilled,
                Err(error) => return Ok(Err(error)),
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            match pipe.read(unfilled, remaining) {
                Ok(Some(0)) | Err(_) => return Err(Stopped::Closed),
                Ok(Some(count)) => self.decoder.advance(count),
                Ok(None) => return Err(Stopped::TimedOut),
            }
        }
    }

    /// Closes the connection. The owner terminates the helper first; later calls report `Closed`.
    pub fn close(&mut self) {
        self.pipe = None;
    }
}

#[cfg(windows)]
mod pipe {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::time::Duration;

    use interprocess::local_socket::Stream;
    use windows_sys::Win32::Foundation::{
        ERROR_BROKEN_PIPE, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED,
        GetLastError, HANDLE, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows_sys::Win32::System::IO::{
        CancelIoEx, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
    };
    use windows_sys::Win32::System::Threading::CreateEventW;

    /// The pipe handle, which interprocess opens for overlapped I/O, and the events its reads and
    /// writes signal.
    pub struct Pipe {
        handle: OwnedHandle,
        read_event: OwnedHandle,
        write_event: OwnedHandle,
        /// A write that outlived its timeout. It is never cancelled while the connection is open:
        /// a cancelled pipe write can leave part of its buffer sent while reporting none sent.
        pending_write: Option<Box<OVERLAPPED>>,
    }

    fn event() -> io::Result<OwnedHandle> {
        // SAFETY: a manual-reset event with default security; the handle is owned below.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateEventW returned a new handle that nothing else owns.
        Ok(unsafe { OwnedHandle::from_raw_handle(event) })
    }

    /// Milliseconds to wait, rounded up so that a remaining fraction still waits.
    fn millis(timeout: Duration) -> u32 {
        u32::try_from(timeout.as_micros().div_ceil(1000))
            .unwrap_or(u32::MAX - 1)
            .min(u32::MAX - 1)
    }

    impl Pipe {
        pub fn new(stream: Stream) -> io::Result<Self> {
            let Stream::NamedPipe(pipe) = stream;
            Ok(Self {
                handle: OwnedHandle::from(pipe),
                read_event: event()?,
                write_event: event()?,
                pending_write: None,
            })
        }

        /// Reads into `buffer`: the byte count, 0 at the end of the stream, or `None` when the
        /// timeout passes first.
        pub fn read(&mut self, buffer: &mut [u8], timeout: Duration) -> io::Result<Option<usize>> {
            let length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
            let data = buffer.as_mut_ptr();
            // SAFETY: `buffer` outlives the operation, which `overlapped` waits for or cancels.
            self.overlapped(timeout, |handle, overlapped| unsafe {
                ReadFile(handle, data, length, std::ptr::null_mut(), overlapped)
            })
        }

        /// Writes from `buffer`, returning the byte count or `None` when the timeout passes first.
        /// A write that times out stays in progress: the next call waits for it and must pass
        /// the same, unchanged buffer, which stays in use until the write completes or the pipe
        /// is dropped.
        pub fn write(&mut self, buffer: &[u8], timeout: Duration) -> io::Result<Option<usize>> {
            let handle = self.handle.as_raw_handle();
            if self.pending_write.is_none() {
                // SAFETY: OVERLAPPED is plain C data, boxed so it stays in place while the write
                // is in progress.
                let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
                overlapped.hEvent = self.write_event.as_raw_handle();
                let length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
                // SAFETY: the caller keeps `buffer` unchanged until the write completes, and
                // `Drop` waits for a write still in progress.
                let started = unsafe {
                    WriteFile(
                        handle,
                        buffer.as_ptr(),
                        length,
                        std::ptr::null_mut(),
                        &mut *overlapped,
                    )
                };
                if started == 0 {
                    match unsafe { GetLastError() } {
                        ERROR_IO_PENDING => {}
                        ERROR_BROKEN_PIPE => return Ok(Some(0)),
                        error => return Err(io::Error::from_raw_os_error(error as i32)),
                    }
                }
                self.pending_write = Some(overlapped);
            }
            let Some(overlapped) = &self.pending_write else {
                unreachable!()
            };
            let mut count = 0;
            // SAFETY: `overlapped` belongs to the write in progress on this handle.
            let done = unsafe {
                GetOverlappedResultEx(handle, &**overlapped, &mut count, millis(timeout), 0)
            };
            if done != 0 {
                self.pending_write = None;
                return Ok(Some(count as usize));
            }
            match unsafe { GetLastError() } {
                // A zero timeout reports an unfinished operation as incomplete instead.
                WAIT_TIMEOUT | ERROR_IO_INCOMPLETE => Ok(None),
                ERROR_BROKEN_PIPE => {
                    self.pending_write = None;
                    Ok(Some(0))
                }
                error => {
                    self.pending_write = None;
                    Err(io::Error::from_raw_os_error(error as i32))
                }
            }
        }

        /// Starts a read and waits for it until the timeout; a timed-out read is cancelled and
        /// waited for, since it may have completed meanwhile.
        fn overlapped(
            &mut self,
            timeout: Duration,
            start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> i32,
        ) -> io::Result<Option<usize>> {
            let handle = self.handle.as_raw_handle();
            // SAFETY: OVERLAPPED is plain C data; the operation owns it until it completes.
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            overlapped.hEvent = self.read_event.as_raw_handle();
            if start(handle, &mut overlapped) == 0 {
                match unsafe { GetLastError() } {
                    ERROR_IO_PENDING => {}
                    ERROR_BROKEN_PIPE => return Ok(Some(0)),
                    error => return Err(io::Error::from_raw_os_error(error as i32)),
                }
            }
            let mut count = 0;
            // SAFETY: `overlapped` belongs to the operation started above on this handle.
            let done = unsafe {
                GetOverlappedResultEx(handle, &overlapped, &mut count, millis(timeout), 0)
            };
            if done != 0 {
                return Ok(Some(count as usize));
            }
            // A zero timeout reports an unfinished operation as incomplete instead.
            match unsafe { GetLastError() } {
                WAIT_TIMEOUT | ERROR_IO_INCOMPLETE => {
                    // SAFETY: as above; the final result is read before `overlapped` is dropped.
                    unsafe { CancelIoEx(handle, &overlapped) };
                    if unsafe { GetOverlappedResult(handle, &overlapped, &mut count, 1) } != 0 {
                        return Ok(Some(count as usize));
                    }
                    match unsafe { GetLastError() } {
                        ERROR_OPERATION_ABORTED => Ok(None),
                        ERROR_BROKEN_PIPE => Ok(Some(0)),
                        error => Err(io::Error::from_raw_os_error(error as i32)),
                    }
                }
                ERROR_BROKEN_PIPE => Ok(Some(0)),
                error => Err(io::Error::from_raw_os_error(error as i32)),
            }
        }
    }

    impl Drop for Pipe {
        /// Stops a write still in progress and waits for it, so that neither its OVERLAPPED nor
        /// the buffer it reads is freed while Windows uses them. The owner terminates the helper
        /// first, so the stream is abandoned either way.
        fn drop(&mut self) {
            if let Some(overlapped) = &self.pending_write {
                let handle = self.handle.as_raw_handle();
                let mut count = 0;
                // SAFETY: `overlapped` belongs to the write in progress on this handle.
                unsafe {
                    CancelIoEx(handle, &**overlapped);
                    GetOverlappedResult(handle, &**overlapped, &mut count, 1);
                }
            }
        }
    }
}

#[cfg(unix)]
mod pipe {
    use std::io::{self, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    use interprocess::local_socket::Stream;
    use rustix::event::{PollFd, PollFlags, Timespec, poll};

    /// A non-blocking socket, waited on with `poll`.
    pub struct Pipe {
        stream: UnixStream,
    }

    impl Pipe {
        pub fn new(stream: Stream) -> io::Result<Self> {
            let Stream::UdSocket(socket) = stream;
            let stream = socket.inner().try_clone()?;
            stream.set_nonblocking(true)?;
            Ok(Self { stream })
        }

        /// Reads into `buffer`: the byte count, 0 at the end of the stream, or `None` when the
        /// timeout passes first.
        pub fn read(&mut self, buffer: &mut [u8], timeout: Duration) -> io::Result<Option<usize>> {
            self.retry(timeout, PollFlags::IN, |stream| stream.read(buffer))
        }

        /// Writes from `buffer`, returning the byte count or `None` when the timeout passes first.
        pub fn write(&mut self, buffer: &[u8], timeout: Duration) -> io::Result<Option<usize>> {
            self.retry(timeout, PollFlags::OUT, |stream| stream.write(buffer))
        }

        fn retry(
            &mut self,
            timeout: Duration,
            ready: PollFlags,
            mut operation: impl FnMut(&mut UnixStream) -> io::Result<usize>,
        ) -> io::Result<Option<usize>> {
            let deadline = Instant::now() + timeout;
            loop {
                match operation(&mut self.stream) {
                    Ok(count) => return Ok(Some(count)),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok(None);
                }
                let timespec = Timespec::try_from(remaining).map_err(io::Error::other)?;
                let mut fds = [PollFd::new(&self.stream, ready)];
                match poll(&mut fds, Some(&timespec)) {
                    Ok(_) | Err(rustix::io::Errno::INTR) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
