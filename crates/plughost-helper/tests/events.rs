//! AppKit startup is lazy in scan mode, including when the idle event loop runs first.
#[cfg(target_os = "macos")]
#[path = "../src/events.rs"]
mod events;
// The messages `events` uses; the rest of them serve the helper.
#[cfg(target_os = "macos")]
#[path = "../src/errors.rs"]
#[allow(dead_code)]
mod errors;

#[cfg(target_os = "macos")]
use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRunLoop, CFRunLoopTimer, kCFRunLoopDefaultMode,
};

#[cfg(target_os = "macos")]
fn main() {
    use objc2_app_kit::NSApplication;
    unsafe extern "C" {
        #[link_name = "NSApp"]
        static APPLICATION: *mut NSApplication;
    }
    assert!(objc2::MainThreadMarker::new().is_some());
    let _runtime = events::init(false).unwrap();
    assert!(unsafe { APPLICATION.is_null() });
    events::with_pool(events::pump);
    assert!(unsafe { APPLICATION.is_null() });
    // Waiting for a request keeps it lazy too, and a request that arrives wakes the wait.
    let woken = |what: &str| {
        events::wake();
        let started = std::time::Instant::now();
        events::with_pool(|| events::wait(std::time::Duration::from_secs(5)));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "the wait {what} was not woken"
        );
    };
    woken("without an application");
    assert!(unsafe { APPLICATION.is_null() });
    // An interactive retry initializes the application and returns foreground activation
    // when no plugin window remains. Host initialization can then be repeated safely.
    events::activate();
    assert!(!unsafe { APPLICATION.is_null() });
    events::pump();
    let _host = events::init(true).unwrap();
    events::pump();
    woken("with an application");
    // A run of the run loop the helper's own loop did not start is nested; the helper's own
    // wait for a request is not.
    let run = || unsafe {
        CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, 0.2, true);
    };
    assert_eq!(depth_seen(run), (1, false));
    assert_eq!(
        depth_seen(|| events::wait(std::time::Duration::from_millis(200))),
        (0, false)
    );
    // One in the default mode while the helper handles a request is the host's wait, as an Audio
    // Unit's; one the helper runs for a plugin's panel then is nested.
    assert_eq!(depth_seen(|| events::handling(run)), (0, true));
    assert_eq!(
        depth_seen(|| events::handling(|| events::interactive(run))),
        (1, false)
    );
}

/// The nesting depth a timer sees that fires during `run`, which runs again until it has, and
/// whether it fires in a wait.
#[cfg(target_os = "macos")]
fn depth_seen(mut run: impl FnMut()) -> (u32, bool) {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    static SEEN: AtomicU32 = AtomicU32::new(u32::MAX);
    static IN_WAIT: AtomicBool = AtomicBool::new(false);
    unsafe extern "C-unwind" fn fired(_timer: *mut CFRunLoopTimer, _info: *mut std::ffi::c_void) {
        IN_WAIT.store(events::in_wait(), Ordering::Relaxed);
        SEEN.store(events::nested_depth(), Ordering::Relaxed);
    }
    SEEN.store(u32::MAX, Ordering::Relaxed);
    let timer = unsafe {
        CFRunLoopTimer::new(
            None,
            CFAbsoluteTimeGetCurrent() + 0.05,
            0.0,
            0,
            0,
            Some(fired),
            std::ptr::null_mut(),
        )
    }
    .unwrap();
    CFRunLoop::main()
        .unwrap()
        .add_timer(Some(&timer), unsafe { kCFRunLoopDefaultMode });
    let started = std::time::Instant::now();
    while SEEN.load(Ordering::Relaxed) == u32::MAX {
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        run();
    }
    (
        SEEN.load(Ordering::Relaxed),
        IN_WAIT.load(Ordering::Relaxed),
    )
}

#[cfg(not(target_os = "macos"))]
fn main() {}
