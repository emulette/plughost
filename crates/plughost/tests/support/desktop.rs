//! The macOS desktop as editor tests see it: the helper's windows from the window server, the
//! frontmost application, and the delay fixture's editor variant acting as a user would.

use std::ptr::NonNull;
use std::time::{Duration, Instant};

use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use plughost::Chain;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const CFArray;
    pub fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> objc2_core_foundation::CGRect;
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> objc2_core_foundation::CGPoint;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *mut std::ffi::c_void);
}

/// Where the machine's cursor is, in the window server's global points.
pub fn cursor() -> (f64, f64) {
    // SAFETY: an event made from no source is released once read.
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        let point = CGEventGetLocation(event);
        CFRelease(event);
        (point.x, point.y)
    }
}

/// The display remote editor tests use: `PLUGHOST_TEST_DISPLAY`, a virtual display's identifier
/// for instance, or the main display.
pub fn test_display() -> u32 {
    std::env::var("PLUGHOST_TEST_DISPLAY")
        .ok()
        .and_then(|display| display.parse().ok())
        .unwrap_or_else(|| unsafe { CGMainDisplayID() })
}

/// The display's top-left corner in the window server's global points.
pub fn display_origin(display: u32) -> (f64, f64) {
    let bounds = unsafe { CGDisplayBounds(display) };
    (bounds.origin.x, bounds.origin.y)
}

type Info = CFDictionary<CFString, CFType>;

/// A window of the helper: its number, place, size and whether it is on screen.
#[derive(Debug, PartialEq)]
pub struct Window {
    pub number: f64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub on_screen: bool,
}

fn number(info: &Info, key: &'static str) -> Option<f64> {
    info.get(&CFString::from_static_str(key))?
        .downcast::<CFNumber>()
        .ok()?
        .as_f64()
}

/// The windows the window server lists for process `pid`, including hidden ones.
pub fn windows(pid: u32) -> Vec<Window> {
    // kCGWindowListOptionAll, relative to no window.
    let Some(list) = NonNull::new(unsafe { CGWindowListCopyWindowInfo(0, 0) }.cast_mut()) else {
        return Vec::new();
    };
    // SAFETY: the copy returns +1 an array of window information dictionaries.
    let list: CFRetained<CFArray<Info>> = unsafe { CFRetained::from_raw(list.cast()) };
    (0..list.len())
        .filter_map(|index| list.get(index))
        .filter(|info| number(info, "kCGWindowOwnerPID") == Some(f64::from(pid)))
        .filter_map(|info| {
            let bounds = info
                .get(&CFString::from_static_str("kCGWindowBounds"))?
                .downcast::<CFDictionary>()
                .ok()?;
            // SAFETY: window bounds map CFString keys to CFNumbers.
            let bounds: CFRetained<Info> = unsafe { CFRetained::cast_unchecked(bounds) };
            Some(Window {
                number: number(&info, "kCGWindowNumber")?,
                x: number(&bounds, "X")?,
                y: number(&bounds, "Y")?,
                width: number(&bounds, "Width")?,
                height: number(&bounds, "Height")?,
                on_screen: info
                    .get(&CFString::from_static_str("kCGWindowIsOnscreen"))
                    .and_then(|value| value.downcast::<CFBoolean>().ok())
                    .is_some_and(|value| value.as_bool()),
            })
        })
        .collect()
}

/// Waits until `check` holds, with the chain's editor state polled so the helper serves requests.
pub fn wait(chain: &mut Chain, mut check: impl FnMut(&mut Chain) -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check(chain) {
        let pid = chain.helper_monitor().process_id();
        assert!(
            Instant::now() < deadline,
            "{what}; windows {:?}",
            windows(pid)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Has the delay fixture's editor variant act as a user would: 1 hides the helper, 2 shows it
/// again, 3 closes the editor's window, 4 activates the helper as a click in a JUCE editor does,
/// 5 opens a popup 250 points right of and 100 below the editor's top-left corner, 6 runs a
/// dialog for eight seconds, 7 paints the window red, then green. Reading the parameters
/// delivers the edit to an active plugin that gets no blocks.
pub fn act(chain: &mut Chain, action: u8) {
    const EDITOR_WINDOW: u64 = 17;
    chain
        .set_parameter_text(0, EDITOR_WINDOW, &action.to_string())
        .unwrap();
    chain.parameters(0).unwrap();
}

/// The editor variant's parameter value, plain.
pub fn value(chain: &mut Chain, id: u64) -> f64 {
    let (_, value) = chain
        .parameters(0)
        .unwrap()
        .into_iter()
        .find(|(info, _)| info.id == id)
        .unwrap();
    chain.parameter_to_plain(0, id, value.unwrap()).unwrap()
}

/// The frontmost application's process, as Launch Services has it now. `NSWorkspace` follows
/// it through notifications on the main run loop, which a test's thread does not run, so it
/// keeps telling the application that was in front first.
pub fn frontmost() -> Option<i32> {
    let lsappinfo = |arguments: &[&str]| {
        std::process::Command::new("lsappinfo")
            .args(arguments)
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap()
    };
    let front = lsappinfo(&["front"]);
    lsappinfo(&["info", "-only", "pid", &front])
        .strip_prefix("\"pid\"=")?
        .parse()
        .ok()
}
