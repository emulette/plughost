//! One helper connection, waited on by the OS while the supervisor keeps its deadline.
use std::io;
use std::time::Duration;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{
    GenericNamespaced, ListenerNonblockingMode, ListenerOptions, Stream,
};

#[cfg(target_os = "macos")]
pub(super) struct Listener(interprocess::local_socket::Listener);

#[cfg(target_os = "macos")]
impl Listener {
    pub fn new(name: &str) -> io::Result<Self> {
        ListenerOptions::new()
            .name(name.to_ns_name::<GenericNamespaced>()?)
            .nonblocking(ListenerNonblockingMode::Accept)
            .create_sync()
            .map(Self)
    }

    pub fn wait(&mut self, timeout: Duration) -> io::Result<bool> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let interprocess::local_socket::Listener::UdSocket(listener) = &self.0;
        let timeout = Timespec::try_from(timeout).map_err(io::Error::other)?;
        let mut descriptors = [PollFd::new(listener, PollFlags::IN)];
        match poll(&mut descriptors, Some(&timeout)) {
            Ok(count) => Ok(count != 0),
            Err(rustix::io::Errno::INTR) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub fn accept(self) -> io::Result<Stream> {
        self.0.accept()
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows_sys::Win32::Foundation::{
        ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::IO::{
        CancelIoEx, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
    };
    use windows_sys::Win32::System::Pipes::ConnectNamedPipe;
    use windows_sys::Win32::System::Threading::CreateEventW;

    use super::*;

    pub(in crate::helper) struct Listener {
        handle: Option<OwnedHandle>,
        // Stable until completion, including if the listener moves or its deadline passes.
        overlapped: Box<OVERLAPPED>,
        pending: bool,
        _event: OwnedHandle,
    }

    impl Listener {
        pub fn new(name: &str) -> io::Result<Self> {
            let socket = ListenerOptions::new()
                .name(name.to_ns_name::<GenericNamespaced>()?)
                .nonblocking(ListenerNonblockingMode::Neither)
                .create_sync()?;
            let interprocess::local_socket::Listener::NamedPipe(socket) = socket;
            // interprocess creates its pipe instances with FILE_FLAG_OVERLAPPED.
            let handle: OwnedHandle = socket.into();
            let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: this is a new event handle; OVERLAPPED is plain C data kept in a Box.
            let event = unsafe { OwnedHandle::from_raw_handle(event) };
            let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
            overlapped.hEvent = event.as_raw_handle();
            let connected = unsafe { ConnectNamedPipe(handle.as_raw_handle(), &mut *overlapped) };
            let pending = if connected != 0 {
                false
            } else {
                match unsafe { GetLastError() } {
                    ERROR_PIPE_CONNECTED => false,
                    ERROR_IO_PENDING => true,
                    error => return Err(io::Error::from_raw_os_error(error as i32)),
                }
            };
            Ok(Self {
                handle: Some(handle),
                overlapped,
                pending,
                _event: event,
            })
        }

        pub fn wait(&mut self, timeout: Duration) -> io::Result<bool> {
            if !self.pending {
                return Ok(true);
            }
            let millis = u32::try_from(timeout.as_micros().div_ceil(1000))
                .unwrap_or(u32::MAX - 1)
                .min(u32::MAX - 1);
            let mut transferred = 0;
            // SAFETY: the listener owns the handle, event and stable OVERLAPPED until completion.
            let completed = unsafe {
                GetOverlappedResultEx(
                    self.handle.as_ref().unwrap().as_raw_handle(),
                    &*self.overlapped,
                    &mut transferred,
                    millis,
                    0,
                )
            };
            if completed != 0 {
                self.pending = false;
                return Ok(true);
            }
            match unsafe { GetLastError() } {
                WAIT_TIMEOUT | ERROR_IO_INCOMPLETE => Ok(false),
                error => Err(io::Error::from_raw_os_error(error as i32)),
            }
        }

        pub fn accept(mut self) -> io::Result<Stream> {
            debug_assert!(!self.pending);
            interprocess::os::windows::named_pipe::local_socket::Stream::try_from(
                self.handle.take().unwrap(),
            )
            .map(Stream::NamedPipe)
            .map_err(io::Error::from)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            if self.pending {
                let handle = self.handle.as_ref().unwrap().as_raw_handle();
                // SAFETY: cancellation is followed by completion before the OVERLAPPED is freed.
                unsafe {
                    CancelIoEx(handle, &*self.overlapped);
                    let mut transferred = 0;
                    GetOverlappedResult(handle, &*self.overlapped, &mut transferred, 1);
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) use windows::Listener;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn readiness_keeps_timeouts_resumable_and_preserves_the_connected_stream() {
        let name = format!("plughost-listener-test-{:016x}", super::super::random());
        let mut listener = Listener::new(&name).unwrap();
        for _ in 0..3 {
            assert!(!listener.wait(Duration::ZERO).unwrap());
        }
        let mut client = Stream::connect(name.to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
        client.write_all(b"hello").unwrap();
        assert!(listener.wait(Duration::from_secs(2)).unwrap());
        let mut server = listener.accept().unwrap();
        let mut bytes = [0; 5];
        server.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"hello");
        server.write_all(b"reply").unwrap();
        client.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"reply");
    }

    #[test]
    fn an_unconnected_listener_can_be_cancelled_and_replaced() {
        let name = format!("plughost-listener-test-{:016x}", super::super::random());
        let mut listener = Listener::new(&name).unwrap();
        assert!(!listener.wait(Duration::ZERO).unwrap());
        drop(listener);
        let mut replacement = Listener::new(&name).unwrap();
        let _client = Stream::connect(name.to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
        assert!(replacement.wait(Duration::from_secs(2)).unwrap());
        replacement.accept().unwrap();
    }
}
