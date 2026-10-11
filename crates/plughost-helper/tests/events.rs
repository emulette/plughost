//! AppKit startup is lazy in scan mode, including when the idle event loop runs first.
#[cfg(target_os = "macos")]
#[path = "../src/events.rs"]
mod events;

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
    // An interactive retry initializes the application and returns foreground activation
    // when no plugin window remains. Host initialization can then be repeated safely.
    events::activate();
    assert!(!unsafe { APPLICATION.is_null() });
    events::pump();
    let _host = events::init(true).unwrap();
    events::pump();
}

#[cfg(not(target_os = "macos"))]
fn main() {}
