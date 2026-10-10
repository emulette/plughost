//! Shared memory whose size is fixed when it is created, so that a process cannot shrink it under
//! a mapping of its peer: a mapped file the peer truncates kills the reader with `SIGBUS`. macOS
//! takes the size of a POSIX shared memory object once and refuses later `ftruncate` calls; the
//! object is unlinked as soon as it is open, so only its handles name it. Windows backs the section
//! with the paging file and gives it a size only when it is created.

use std::io;
use std::ptr::NonNull;

#[cfg(unix)]
pub(super) use std::os::fd::OwnedFd as Handle;
#[cfg(windows)]
pub(super) use std::os::windows::io::OwnedHandle as Handle;

/// A read-write mapping of `len` bytes of shared memory, and its handle for transfer.
pub(super) struct Memory {
    pointer: NonNull<u8>,
    len: usize,
    handle: Handle,
}

// SAFETY: the mapping stays valid until `Memory` drops, whichever thread drops it. Both processes
// access it only through atomics and through copies of ranges the slot protocol hands over.
unsafe impl Send for Memory {}
unsafe impl Sync for Memory {}

impl Memory {
    pub fn new(len: usize) -> io::Result<Memory> {
        let handle = platform::create(len)?;
        // SAFETY: the shared memory was just created with this size.
        unsafe { Memory::open(handle, len) }
    }

    /// Maps the first `len` bytes of shared memory a peer created with [`Memory::new`].
    ///
    /// # Safety
    ///
    /// `handle` must be shared memory from [`Memory::new`], not a file.
    pub unsafe fn open(handle: Handle, len: usize) -> io::Result<Memory> {
        // SAFETY: forwarded from the caller.
        let pointer = unsafe { platform::map(&handle, len) }?;
        Ok(Memory {
            pointer,
            len,
            handle,
        })
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.pointer.as_ptr()
    }

    pub fn handle(&self) -> &Handle {
        &self.handle
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        // SAFETY: the mapping is not used after `Memory` drops.
        unsafe { platform::unmap(self.pointer, self.len) };
    }
}

#[cfg(unix)]
mod platform {
    use std::io;
    use std::os::fd::OwnedFd;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicU32, Ordering};

    use rustix::fs::Mode;
    use rustix::mm::{self, MapFlags, ProtFlags};
    use rustix::shm;

    use super::super::errors::{MEMORY, invalid};

    pub fn create(len: usize) -> io::Result<OwnedFd> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        loop {
            // macOS limits the name to 31 bytes.
            let name = format!(
                "/plughost-{:x}-{:x}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let flags = shm::OFlags::CREATE | shm::OFlags::EXCL | shm::OFlags::RDWR;
            match shm::open(name.as_str(), flags, Mode::RUSR | Mode::WUSR) {
                Ok(fd) => {
                    shm::unlink(name.as_str())?;
                    rustix::fs::ftruncate(&fd, len as u64)?;
                    return Ok(fd);
                }
                // A name left by an earlier process with this ID.
                Err(rustix::io::Errno::EXIST) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// # Safety
    ///
    /// `fd` must be shared memory, whose size cannot shrink while it is mapped.
    pub unsafe fn map(fd: &OwnedFd, len: usize) -> io::Result<NonNull<u8>> {
        // macOS rounds the size up to whole pages.
        if (rustix::fs::fstat(fd)?.st_size as u64) < len as u64 {
            return Err(invalid(MEMORY));
        }
        // SAFETY: a new shared mapping of an object at least `len` bytes long.
        let pointer = unsafe {
            mm::mmap(
                std::ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                fd,
                0,
            )
        }?;
        NonNull::new(pointer.cast()).ok_or_else(|| invalid(MEMORY))
    }

    /// # Safety
    ///
    /// `pointer` and `len` must be a mapping from [`map`] that is no longer used.
    pub unsafe fn unmap(pointer: NonNull<u8>, len: usize) {
        // SAFETY: forwarded from the caller.
        let _ = unsafe { mm::munmap(pointer.as_ptr().cast(), len) };
    }
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::NonNull;

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS,
        MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
    };

    pub fn create(len: usize) -> io::Result<OwnedHandle> {
        let size = len as u64;
        // SAFETY: an unnamed section backed by the paging file, with default security.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                (size >> 32) as u32,
                size as u32,
                std::ptr::null(),
            )
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the new handle is owned here.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    /// Fails when the section is smaller than `len`.
    ///
    /// # Safety
    ///
    /// `handle` must be a section.
    pub unsafe fn map(handle: &OwnedHandle, len: usize) -> io::Result<NonNull<u8>> {
        // SAFETY: a new view of the whole requested range of a live section.
        let view = unsafe {
            MapViewOfFile(
                handle.as_raw_handle(),
                FILE_MAP_READ | FILE_MAP_WRITE,
                0,
                0,
                len,
            )
        };
        NonNull::new(view.Value.cast()).ok_or_else(io::Error::last_os_error)
    }

    /// # Safety
    ///
    /// `pointer` must be a view from [`map`] that is no longer used.
    pub unsafe fn unmap(pointer: NonNull<u8>, _len: usize) {
        // SAFETY: forwarded from the caller.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: pointer.as_ptr().cast(),
            })
        };
    }
}
