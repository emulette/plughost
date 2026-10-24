//! The OS event loop the helper's main thread runs between requests.

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::Cell;

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy, NSEventMask,
        NSRunningApplication, NSWorkspace,
    };
    use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

    unsafe extern "C" {
        // objc2-app-kit's NSApp function calls sharedApplication, creating it unconditionally.
        #[link_name = "NSApp"]
        static APPLICATION: *mut NSApplication;
    }

    thread_local! {
        /// The application that was frontmost when the helper last brought itself to the front.
        static PREVIOUS: Cell<Option<Retained<NSRunningApplication>>> = const { Cell::new(None) };
        static LAUNCHED: Cell<bool> = const { Cell::new(false) };
    }

    /// Plugins create windows, timers, and dialogs, which need an application object. The helper
    /// is a background (`LSUIElement`) application.
    pub struct Runtime;

    fn application(mtm: MainThreadMarker) -> Retained<NSApplication> {
        let app = NSApplication::sharedApplication(mtm);
        if !LAUNCHED.replace(true) {
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            app.finishLaunching();
        }
        app
    }

    pub fn init(host: bool) -> std::io::Result<Runtime> {
        if host && let Some(mtm) = MainThreadMarker::new() {
            application(mtm);
        }
        Ok(Runtime)
    }

    /// Brings the helper to the front, so editors and the dialogs plugins show can be used. The
    /// application that was frontmost gets the activation back once the helper shows no windows.
    pub fn activate() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = application(mtm);
        let previous = NSWorkspace::sharedWorkspace().frontmostApplication();
        if let Some(previous) =
            previous.filter(|app| *app != NSRunningApplication::currentApplication())
        {
            PREVIOUS.set(Some(previous));
        }
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }

    /// An accessory application stays active after its last window closes, which leaves key input
    /// with no window to go to. Returns the activation to the application that had it.
    fn return_activation(app: &NSApplication) {
        if !app.isActive()
            || app
                .windows()
                .iter()
                .any(|window| window.isVisible() || window.isMiniaturized())
        {
            return;
        }
        if let Some(previous) = PREVIOUS.take() {
            previous.activateWithOptions(NSApplicationActivationOptions::empty());
        }
    }

    /// Runs `work` in an autorelease pool. The helper's threads never return to a run loop that
    /// drains one, so objects AppKit, Audio Units and plugins autorelease there, closed editor
    /// windows among them, would otherwise live until the thread exits.
    pub fn with_pool<R>(work: impl FnOnce() -> R) -> R {
        objc2::rc::autoreleasepool(|_| work())
    }

    /// Handles the events and run loop sources that are ready, without waiting, then returns the
    /// activation if the last window closed.
    pub fn pump() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // SAFETY: AppKit's application pointer is inspected only on the main thread. A scan
        // does not need an application unless a plugin created one or an interactive retry did.
        if unsafe { APPLICATION.is_null() } {
            unsafe { CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, 0.0, true) };
            return;
        }
        let app = application(mtm);
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
        return_activation(&app);
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
    };

    use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_NOREMOVE,
        PM_QS_SENDMESSAGE, PM_REMOVE, PeekMessageW, QS_SENDMESSAGE, TranslateMessage,
    };

    pub struct Runtime;

    impl Drop for Runtime {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    pub fn init(_host: bool) -> std::io::Result<Runtime> {
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

    /// An unnamed auto-reset event, initially unsignaled, that another thread sets to end
    /// [`wait_sent`].
    pub fn signal() -> std::io::Result<OwnedHandle> {
        // SAFETY: creates a new event with default security.
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: the new handle is owned here.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    pub fn raise(signal: &OwnedHandle) {
        // SAFETY: the event is live.
        unsafe { SetEvent(signal.as_raw_handle()) };
    }

    /// Returns once `signal` is set, or once another thread sent this one messages, which it
    /// handles first: a plugin that sends the main thread a message from its processing waits
    /// for them. Posted messages stay queued for the event loop.
    pub fn wait_sent(signal: &OwnedHandle) {
        let handle = signal.as_raw_handle();
        // SAFETY: one live event handle; MSG is plain C data PeekMessageW fills in, and a peek
        // limited to sent messages removes nothing from the queue.
        unsafe {
            MsgWaitForMultipleObjectsEx(1, &handle, INFINITE, QS_SENDMESSAGE, MWMO_INPUTAVAILABLE);
            let mut message: MSG = std::mem::zeroed();
            PeekMessageW(
                &mut message,
                std::ptr::null_mut(),
                0,
                0,
                PM_NOREMOVE | PM_QS_SENDMESSAGE,
            );
        }
    }
}

pub use platform::{activate, init, pump, with_pool};
#[cfg(target_os = "windows")]
pub use platform::{raise, signal, wait_sent};

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, SendMessageW, WM_NULL,
    };

    use super::{raise, signal, wait_sent};

    #[test]
    fn a_wait_handles_what_another_thread_sends_until_it_is_signaled() {
        let class: Vec<u16> = "STATIC".encode_utf16().chain([0]).collect();
        // SAFETY: a message-only window of a system class, on this thread.
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!window.is_null());
        let answered = Arc::new(signal().unwrap());
        let raised = Arc::clone(&answered);
        let done = Arc::new(AtomicBool::new(false));
        let finished = Arc::clone(&done);
        let address = window as usize;
        // As a plugin's processing would: it waits for this thread before it answers.
        let sender = thread::spawn(move || {
            // SAFETY: the window lives until this thread is joined.
            unsafe { SendMessageW(address as _, WM_NULL, 0, 0) };
            finished.store(true, Ordering::Release);
            raise(&raised);
        });
        while !done.load(Ordering::Acquire) {
            wait_sent(&answered);
        }
        sender.join().unwrap();
        // SAFETY: this thread created the window.
        unsafe { DestroyWindow(window) };
    }
}
