//! A real OS timer: it advances only when the host pumps its native event loop.
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use objc2_core_foundation::{
        CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext,
        kCFRunLoopDefaultMode,
    };
    use std::ffi::c_void;

    pub struct SystemTimer {
        timer: CFRetained<CFRunLoopTimer>,
        _calls: Arc<AtomicU32>,
    }

    unsafe extern "C-unwind" fn tick(_: *mut CFRunLoopTimer, context: *mut c_void) {
        // The owner invalidates the timer before releasing this Arc's storage.
        unsafe { &*context.cast::<AtomicU32>() }.fetch_add(1, Ordering::Relaxed);
    }

    impl SystemTimer {
        pub fn new(calls: Arc<AtomicU32>) -> Self {
            let mut context = CFRunLoopTimerContext {
                version: 0,
                info: Arc::as_ptr(&calls).cast_mut().cast(),
                retain: None,
                release: None,
                copyDescription: None,
            };
            let timer = unsafe {
                CFRunLoopTimer::new(
                    None,
                    CFAbsoluteTimeGetCurrent() + 0.001,
                    0.001,
                    0,
                    0,
                    Some(tick),
                    &mut context,
                )
            }
            .unwrap();
            CFRunLoop::current()
                .unwrap()
                .add_timer(Some(&timer), unsafe { kCFRunLoopDefaultMode });
            Self {
                timer,
                _calls: calls,
            }
        }
    }
    impl Drop for SystemTimer {
        fn drop(&mut self) {
            self.timer.invalidate();
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

    thread_local! {
        static TIMERS: RefCell<HashMap<usize, Arc<AtomicU32>>> = RefCell::new(HashMap::new());
    }
    pub struct SystemTimer(usize);

    unsafe extern "system" fn tick(_: HWND, _: u32, id: usize, _: u32) {
        TIMERS.with_borrow(|timers| {
            if let Some(calls) = timers.get(&id) {
                calls.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
    impl SystemTimer {
        pub fn new(calls: Arc<AtomicU32>) -> Self {
            let id = unsafe { SetTimer(std::ptr::null_mut(), 0, 1, Some(tick)) };
            assert_ne!(id, 0);
            TIMERS.with_borrow_mut(|timers| timers.insert(id, calls));
            Self(id)
        }
    }
    impl Drop for SystemTimer {
        fn drop(&mut self) {
            unsafe { KillTimer(std::ptr::null_mut(), self.0) };
            TIMERS.with_borrow_mut(|timers| timers.remove(&self.0));
        }
    }
}

pub use platform::SystemTimer;
