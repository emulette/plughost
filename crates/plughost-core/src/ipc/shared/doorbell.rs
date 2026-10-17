//! The doorbell that wakes the helper's realtime worker: a counter in the ring's control mapping
//! that the application's callback and the helper's main thread advance. On macOS the worker
//! waits on the counter's shared address (`os_sync_wait_on_address`, macOS 14.4 or newer); on
//! Windows, where `WaitOnAddress` wakes only within one process, an auto-reset Event the
//! application duplicates into the helper also wakes it.

use std::io;
use std::sync::atomic::{AtomicU32, Ordering};

pub(super) struct Doorbell {
    #[cfg(windows)]
    pub(super) event: std::os::windows::io::OwnedHandle,
}

impl Doorbell {
    /// The application's doorbell, created with the ring.
    pub fn new() -> io::Result<Doorbell> {
        Ok(Doorbell {
            #[cfg(windows)]
            event: platform::event()?,
        })
    }

    /// Advances `counter` and wakes the worker waiting on it.
    #[inline]
    pub fn ring(&self, counter: &AtomicU32) {
        counter.fetch_add(1, Ordering::Release);
        self.wake(counter);
    }

    /// Returns once `counter` may differ from `last`, or spuriously.
    #[inline]
    pub fn wait(&self, counter: &AtomicU32, last: u32) {
        if counter.load(Ordering::Acquire) == last {
            self.sleep(counter, last);
        }
    }

    #[cfg(target_os = "macos")]
    fn wake(&self, counter: &AtomicU32) {
        // SAFETY: the counter is a live 32-bit word in a shared mapping.
        unsafe { platform::wake(counter) };
    }

    #[cfg(target_os = "macos")]
    fn sleep(&self, counter: &AtomicU32, last: u32) {
        // SAFETY: as in `wake`.
        unsafe { platform::wait(counter, last) };
    }

    #[cfg(windows)]
    fn wake(&self, _counter: &AtomicU32) {
        platform::set(&self.event);
    }

    #[cfg(windows)]
    fn sleep(&self, _counter: &AtomicU32, _last: u32) {
        platform::wait(&self.event);
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::c_void;
    use std::sync::atomic::AtomicU32;

    /// `OS_SYNC_WAIT_ON_ADDRESS_SHARED` and `OS_SYNC_WAKE_BY_ADDRESS_SHARED`.
    const SHARED: u32 = 1;

    unsafe extern "C" {
        fn os_sync_wait_on_address(addr: *mut c_void, value: u64, size: usize, flags: u32) -> i32;
        fn os_sync_wake_by_address_any(addr: *mut c_void, size: usize, flags: u32) -> i32;
    }

    /// # Safety
    ///
    /// `counter` must lie in memory shared with the waiting process.
    pub unsafe fn wake(counter: &AtomicU32) {
        // SAFETY: forwarded from the caller. No waiter is not an error worth reporting.
        unsafe { os_sync_wake_by_address_any(counter.as_ptr().cast(), 4, SHARED) };
    }

    /// # Safety
    ///
    /// As for [`wake`].
    pub unsafe fn wait(counter: &AtomicU32, last: u32) {
        // SAFETY: forwarded from the caller. Interruptions and spurious wakes return; the
        // caller checks the counter again.
        unsafe { os_sync_wait_on_address(counter.as_ptr().cast(), u64::from(last), 4, SHARED) };
    }
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows_sys::Win32::System::Threading::{
        CreateEventW, INFINITE, SetEvent, WaitForSingleObject,
    };

    /// An unnamed auto-reset event, initially unsignaled.
    pub fn event() -> io::Result<OwnedHandle> {
        // SAFETY: creates a new event with default security.
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the new handle is owned here.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    pub fn set(event: &OwnedHandle) {
        // SAFETY: the event is live.
        unsafe { SetEvent(event.as_raw_handle()) };
    }

    pub fn wait(event: &OwnedHandle) {
        // SAFETY: the event is live.
        unsafe { WaitForSingleObject(event.as_raw_handle(), INFINITE) };
    }
}
