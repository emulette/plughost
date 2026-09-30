//! The OS event loop the helper's main thread runs between requests.

#[cfg(target_os = "macos")]
mod platform {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

    /// Plugins create windows, timers, and dialogs, which need an application object. The helper
    /// is a background (`LSUIElement`) application.
    pub struct Runtime;

    pub fn init() -> std::io::Result<Runtime> {
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            app.finishLaunching();
        }
        Ok(Runtime)
    }

    /// Brings the helper to the front, so dialogs plugins show can be answered.
    pub fn activate() {
        if let Some(mtm) = MainThreadMarker::new() {
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        }
    }

    /// Runs `work` in an autorelease pool. The helper's threads never return to a run loop that
    /// drains one, so objects AppKit, Audio Units and plugins autorelease there, closed editor
    /// windows among them, would otherwise live until the thread exits.
    pub fn with_pool<R>(work: impl FnOnce() -> R) -> R {
        objc2::rc::autoreleasepool(|_| work())
    }

    /// Handles the events and run loop sources that are ready, without waiting.
    pub fn pump() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let now = NSDate::now();
        loop {
            let event = unsafe {
                app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&now),
                    NSDefaultRunLoopMode,
                    true,
                )
            };
            match event {
                Some(event) => app.sendEvent(&event),
                None => break,
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };

    pub struct Runtime;

    impl Drop for Runtime {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    pub fn init() -> std::io::Result<Runtime> {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
        };
        // A crashed plugin must exit so the host can report it, rather than wait in a system dialog.
        unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) };
        // Plugin editors use COM services (including WIC and drag/drop) on the owning thread.
        let result = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if result < 0 {
            return Err(std::io::Error::other(format!(
                "{}: {result:#x}",
                crate::errors::COM_INIT
            )));
        }
        Ok(Runtime)
    }

    /// Foreground permission is granted by the application before an interactive request.
    pub fn activate() {}

    /// Runs `work`; Windows has no autorelease pools.
    pub fn with_pool<R>(work: impl FnOnce() -> R) -> R {
        work()
    }

    /// Dispatches the messages waiting for this thread, without waiting.
    pub fn pump() {
        // SAFETY: MSG is plain C data PeekMessageW fills in.
        let mut message: MSG = unsafe { std::mem::zeroed() };
        unsafe {
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

pub use platform::{activate, init, pump, with_pool};
