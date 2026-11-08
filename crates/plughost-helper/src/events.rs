//! The OS event loop the helper's main thread runs between requests.

#[cfg(target_os = "macos")]
mod platform {
    use std::cell::{Cell, RefCell};

    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy, NSEventMask,
        NSRunningApplication, NSWorkspace,
    };
    use objc2_core_foundation::{
        CFRetained, CFRunLoop, CFRunLoopActivity, CFRunLoopObserver, CFRunLoopSource,
        CFRunLoopSourceContext, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
    };
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
        if let Some(mtm) = MainThreadMarker::new() {
            WAKER.get_or_init(|| Waker::new(mtm));
            if host {
                application(mtm);
            }
        }
        Ok(Runtime)
    }

    /// Wakes the main thread from [`wait`] from any thread. A run loop wakeup alone is lost when
    /// the main thread is not asleep yet; the signaled source stays signaled until the run loop
    /// handles it.
    struct Waker {
        source: CFRetained<CFRunLoopSource>,
        run_loop: CFRetained<CFRunLoop>,
    }

    // SAFETY: signaling a run loop source and waking a run loop are thread-safe.
    unsafe impl Send for Waker {}
    unsafe impl Sync for Waker {}

    static WAKER: std::sync::OnceLock<Waker> = std::sync::OnceLock::new();

    thread_local! {
        /// How many runs of the main run loop are in progress, one inside the other.
        static DEPTH: Cell<u32> = const { Cell::new(0) };
        /// The helper's own loop is running the run loop: waiting for a request or pumping. A
        /// run it started is not nested; runs inside it, or started while it handles a request
        /// or an event, are.
        static OUTER: Cell<bool> = const { Cell::new(false) };
        /// The helper handles a request.
        static REQUEST: Cell<bool> = const { Cell::new(false) };
        /// How many loops the helper runs for a plugin as its own: a panel's.
        static INTERACTIVE: Cell<u32> = const { Cell::new(0) };
        /// For each run of the main run loop in progress, innermost last, whether it is a wait.
        static RUNS: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    }

    /// Counts the runs of the main run loop. One in the default mode that host code starts while
    /// the helper handles a request, as an Audio Unit's wait for its listeners or its
    /// instantiation does, is a wait, not a loop nested in the helper's: no events are taken
    /// in it, so no one can act in it, as a plugin's dialog, menu or panel lets one.
    unsafe extern "C-unwind" fn counted(
        _observer: *mut CFRunLoopObserver,
        activity: CFRunLoopActivity,
        _info: *mut std::ffi::c_void,
    ) {
        if activity.contains(CFRunLoopActivity::Entry) {
            let wait = REQUEST.get()
                && INTERACTIVE.get() == 0
                && CFRunLoop::main()
                    .and_then(|main| main.current_mode())
                    .is_some_and(|mode| Some(&*mode) == unsafe { kCFRunLoopDefaultMode });
            RUNS.with_borrow_mut(|runs| runs.push(wait));
            if !wait {
                DEPTH.set(DEPTH.get() + 1);
            }
        } else if activity.contains(CFRunLoopActivity::Exit)
            && RUNS.with_borrow_mut(Vec::pop) == Some(false)
        {
            DEPTH.set(DEPTH.get().saturating_sub(1));
        }
    }

    /// Handles a request: runs of the run loop in it in the default mode are waits.
    pub fn handling<R>(handle: impl FnOnce() -> R) -> R {
        let previous = REQUEST.replace(true);
        let result = handle();
        REQUEST.set(previous);
        result
    }

    /// Runs a loop the helper runs for a plugin in the default mode, which counts as nested.
    pub fn interactive<R>(run: impl FnOnce() -> R) -> R {
        INTERACTIVE.set(INTERACTIVE.get() + 1);
        let result = run();
        INTERACTIVE.set(INTERACTIVE.get() - 1);
        result
    }

    /// Whether the innermost run of the main run loop is a wait, in which nothing a plugin does
    /// for someone acting should run.
    pub fn in_wait() -> bool {
        RUNS.with_borrow(|runs| runs.last() == Some(&true))
    }

    /// How many loops are nested in the helper's own on the main thread: a dialog's, a menu's,
    /// a panel's. Call it on the main thread, from inside such a loop or between requests.
    pub fn nested_depth() -> u32 {
        DEPTH.get().saturating_sub(u32::from(OUTER.get()))
    }

    /// Runs `run` as the helper's own run of the run loop.
    fn outer<R>(run: impl FnOnce() -> R) -> R {
        let previous = OUTER.replace(true);
        let result = run();
        OUTER.set(previous);
        result
    }

    unsafe extern "C-unwind" fn woken(_info: *mut std::ffi::c_void) {}

    impl Waker {
        fn new(_mtm: MainThreadMarker) -> Waker {
            let mut context = CFRunLoopSourceContext {
                version: 0,
                info: std::ptr::null_mut(),
                retain: None,
                release: None,
                copyDescription: None,
                equal: None,
                hash: None,
                schedule: None,
                cancel: None,
                perform: Some(woken),
            };
            // SAFETY: the context is a valid version 0 context, copied by the call.
            let source = unsafe { CFRunLoopSource::new(None, 0, &mut context) }
                .expect(crate::errors::RUN_LOOP_SOURCE);
            let run_loop = CFRunLoop::main().expect(crate::errors::MAIN_RUN_LOOP);
            run_loop.add_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
            // SAFETY: the callout matches the observer's signature and takes no context.
            let observer = unsafe {
                CFRunLoopObserver::new(
                    None,
                    (CFRunLoopActivity::Entry | CFRunLoopActivity::Exit).0,
                    true,
                    0,
                    Some(counted),
                    std::ptr::null_mut(),
                )
            }
            .expect(crate::errors::RUN_LOOP_OBSERVER);
            run_loop.add_observer(Some(&observer), unsafe { kCFRunLoopCommonModes });
            Waker { source, run_loop }
        }
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

    /// Waits up to `timeout` on the main run loop until a run loop source, an event or [`wake`]
    /// comes, then handles the events that are ready. Run loop sources, timers and the main
    /// queue run while the helper waits for requests, not only when it pumps.
    pub fn wait(timeout: std::time::Duration) {
        outer(|| unsafe {
            CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, timeout.as_secs_f64(), true)
        });
        pump();
    }

    /// Wakes the main thread from [`wait`], from any thread: a request is waiting for it.
    pub fn wake() {
        if let Some(waker) = WAKER.get() {
            waker.source.signal();
            waker.run_loop.wake_up();
        }
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
            outer(|| unsafe { CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, 0.0, true) });
            return;
        }
        let app = application(mtm);
        let now = NSDate::now();
        loop {
            let event = outer(|| unsafe {
                app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&now),
                    NSDefaultRunLoopMode,
                    true,
                )
            });
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

    /// The Windows helper waits for requests on its channel, which wakes it.
    pub fn wake() {}

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

pub use platform::{activate, init, pump, wake, with_pool};
#[cfg(target_os = "macos")]
pub use platform::{handling, in_wait, interactive, nested_depth, wait};
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
