//! Captures of a real remote editor. They need screen recording permission, which processes
//! started over SSH do not have: run them from a terminal of the logged-in session that has it.
//! The region is on the display `PLUGHOST_TEST_DISPLAY` names, or the main display. Build the
//! helper and the test plugins first (`scripts/build-helper.sh`, `scripts/build-test-plugins.sh`).
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use objc2_core_foundation::CFRetained;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use plughost::remote::{Rect, Region};
use plughost::{Chain, ChainSlot, PluginRef, SpawnOptions};
use plughost_capture::{Capture, CaptureError, CaptureOptions, Frame};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
}

/// Captures share the window server; one test at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn target() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")
}

fn display() -> u32 {
    std::env::var("PLUGHOST_TEST_DISPLAY")
        .ok()
        .and_then(|display| display.parse().ok())
        .unwrap_or_else(|| unsafe { CGMainDisplayID() })
}

fn region(x: f64, y: f64, width: f64, height: f64) -> Region {
    Region::new(display(), Rect::new(x, y, width, height))
}

/// A remote chain showing the delay fixture's editor variant, a 240 by 120 editor, in `region`.
fn editor(region: Region) -> Chain {
    let plugin = PluginRef::clap(
        target().join("test-plugins/plughost-test-editor.clap"),
        "com.studio.plughost.test-delay.7900000b",
    );
    let mut options = SpawnOptions::default();
    options.remote = true;
    let helper = target().join("helper/PlughostHelper.app/Contents/MacOS/plughost-helper");
    let mut chain = Chain::spawn(&helper, &[ChainSlot::from(plugin)], &options).unwrap();
    chain.set_remote_region(region).unwrap();
    chain.open_remote_editor(0).unwrap();
    chain
}

/// Has the editor paint its window red, then green the next time.
fn paint(chain: &mut Chain) {
    chain.set_parameter_text(0, 17, "7").unwrap();
    chain.parameters(0).unwrap();
}

/// The frames a capture hands over, kept for the test.
#[derive(Clone, Default)]
struct Frames(Arc<Mutex<Vec<Frame>>>);

impl Frames {
    fn keep(&self) -> impl Fn(Frame) + Send + Sync + 'static {
        let frames = self.clone();
        move |frame| {
            let mut kept = frames.0.lock().unwrap();
            // Holding many would stall the stream.
            if kept.len() == 2 {
                kept.remove(0);
            }
            kept.push(frame);
        }
    }

    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    /// Waits for a frame that satisfies `check`.
    fn until(&self, mut check: impl FnMut(&Frame) -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.0.lock().unwrap().iter().any(&mut check) {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The blue, green and red of the pixel at `(x, y)` points of a BGRA frame.
fn pixel(frame: &Frame, x: f64, y: f64) -> [u8; 3] {
    // SAFETY: the frame's pixel buffer lives while the frame does; this retains it once more.
    let buffer: CFRetained<CVPixelBuffer> =
        unsafe { CFRetained::retain(frame.pixel_buffer().cast()) };
    let flags = CVPixelBufferLockFlags::ReadOnly;
    assert_eq!(unsafe { CVPixelBufferLockBaseAddress(&buffer, flags) }, 0);
    let row = (y * frame.scale()) as usize;
    let column = (x * frame.scale()) as usize;
    let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
    let offset = row * CVPixelBufferGetBytesPerRow(&buffer) + column * 4;
    let bgr = unsafe {
        [
            *base.add(offset),
            *base.add(offset + 1),
            *base.add(offset + 2),
        ]
    };
    unsafe { CVPixelBufferUnlockBaseAddress(&buffer, flags) };
    bgr
}

fn red(bgr: [u8; 3]) -> bool {
    bgr[2] > 200 && bgr[1] < 60 && bgr[0] < 60
}

fn start(chain: &Chain, region: Region, frames: &Frames) -> Capture {
    let pid = chain.helper_monitor().process_id();
    Capture::start(
        &region,
        pid,
        &CaptureOptions::default(),
        frames.keep(),
        |reason| panic!("the capture stopped: {reason}"),
    )
    .unwrap()
}

#[test]
#[ignore = "needs screen recording permission, the helper and the delay fixtures"]
fn a_capture_holds_the_helpers_windows_in_the_region_and_nothing_else() {
    let _serial = serial();
    let area = region(100.0, 100.0, 400.0, 300.0);
    let mut chain = editor(area);
    paint(&mut chain);
    // Another session's editor, painted red too, covers the corner of the region.
    let mut neighbour = editor(region(440.0, 340.0, 300.0, 200.0));
    paint(&mut neighbour);
    let frames = Frames::default();
    let _capture = start(&chain, area, &frames);
    // Green, then red again, so the region changes after the capture started.
    paint(&mut chain);
    paint(&mut chain);
    frames.until(
        |frame| red(pixel(frame, 20.0 + 120.0, 20.0 + 60.0)),
        "the editor's window is not in the frames",
    );
    let frames = frames.0.lock().unwrap();
    let frame = frames.last().unwrap();
    assert_eq!(
        (frame.width(), frame.height()),
        (
            (400.0 * frame.scale()) as usize,
            (300.0 * frame.scale()) as usize
        )
    );
    // The other session's editor is at the corner, past this editor, and is left out.
    assert_eq!(pixel(frame, 390.0, 290.0), [0, 0, 0]);
}

#[test]
#[ignore = "needs screen recording permission, the helper and the delay fixtures"]
fn stopping_one_capture_leaves_another_in_the_process_running() {
    let _serial = serial();
    let (left, right) = (
        region(100.0, 100.0, 300.0, 200.0),
        region(500.0, 100.0, 300.0, 200.0),
    );
    let (mut first, mut second) = (editor(left), editor(right));
    let (first_frames, second_frames) = (Frames::default(), Frames::default());
    let first_capture = start(&first, left, &first_frames);
    let _second_capture = start(&second, right, &second_frames);
    paint(&mut first);
    paint(&mut second);
    first_frames.until(|_| true, "no frame of the first");
    second_frames.until(|_| true, "no frame of the second");
    drop(first_capture);
    second_frames.clear();
    paint(&mut second);
    second_frames.until(|_| true, "the second capture stopped with the first");
}

#[test]
#[ignore = "needs screen recording permission, the helper and the delay fixtures"]
fn a_paused_capture_hands_over_no_frames_until_it_resumes() {
    let _serial = serial();
    let area = region(100.0, 100.0, 300.0, 200.0);
    let mut chain = editor(area);
    let frames = Frames::default();
    let capture = start(&chain, area, &frames);
    paint(&mut chain);
    frames.until(|_| true, "no frame before pausing");
    capture.pause();
    std::thread::sleep(Duration::from_millis(200));
    frames.clear();
    paint(&mut chain);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(frames.count(), 0);
    capture.resume();
    paint(&mut chain);
    frames.until(|_| true, "no frame after resuming");
}

#[test]
#[ignore = "needs screen recording permission, the helper and the delay fixtures"]
fn a_capture_follows_its_region_on_its_display() {
    let _serial = serial();
    let area = region(100.0, 100.0, 300.0, 200.0);
    let mut chain = editor(area);
    let frames = Frames::default();
    let capture = start(&chain, area, &frames);
    let moved = region(200.0, 150.0, 400.0, 300.0);
    chain.set_remote_region(moved).unwrap();
    capture.set_region(&moved).unwrap();
    frames.clear();
    paint(&mut chain);
    frames.until(
        |frame| frame.width() == (400.0 * frame.scale()) as usize && red(pixel(frame, 140.0, 80.0)),
        "the capture did not follow the region",
    );
    // Off the display, or onto another one, is refused.
    assert!(matches!(
        capture.set_region(&region(-10.0, 0.0, 300.0, 200.0)),
        Err(CaptureError::Region)
    ));
    assert!(matches!(
        capture.set_region(&Region::new(u32::MAX, Rect::new(0.0, 0.0, 300.0, 200.0))),
        Err(CaptureError::Region)
    ));
}

#[test]
#[ignore = "needs screen recording permission"]
fn a_capture_of_no_helper_or_no_display_is_refused() {
    let _serial = serial();
    let none = |_| {};
    let stopped = |_| {};
    assert!(matches!(
        Capture::start(
            &region(0.0, 0.0, 100.0, 100.0),
            u32::MAX,
            &CaptureOptions::default(),
            none,
            stopped
        ),
        Err(CaptureError::NoHelper)
    ));
    assert!(matches!(
        Capture::start(
            &Region::new(u32::MAX, Rect::new(0.0, 0.0, 100.0, 100.0)),
            std::process::id(),
            &CaptureOptions::default(),
            none,
            stopped,
        ),
        Err(CaptureError::NoDisplay)
    ));
}

#[test]
#[ignore = "needs screen recording permission, the helper and the delay fixtures"]
fn a_window_another_session_opens_later_is_left_out() {
    let _serial = serial();
    let area = region(400.0, 300.0, 400.0, 300.0);
    let mut chain = editor(area);
    // Another session's editor sits up and left of the region; the popup it opens 250 points
    // right of and 100 below its corner reaches into the region's top-left corner.
    let mut neighbour = editor(region(130.0, 210.0, 300.0, 200.0));
    let frames = Frames::default();
    let _capture = start(&chain, area, &frames);
    neighbour.set_parameter_text(0, 17, "5").unwrap();
    neighbour.parameters(0).unwrap();
    // The captures look again for other helpers' windows every half second.
    std::thread::sleep(Duration::from_millis(1500));
    frames.clear();
    paint(&mut chain);
    frames.until(
        |frame| red(pixel(frame, 140.0, 80.0)),
        "no frame after the popup opened",
    );
    let frames = frames.0.lock().unwrap();
    // The popup covers the region's left edge up to and under this editor, 20 points in.
    assert_eq!(pixel(frames.last().unwrap(), 10.0, 50.0), [0, 0, 0]);
}
