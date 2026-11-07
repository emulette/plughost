//! What a remote session is told without asking. A timer in the run loop's common modes, which
//! also fire inside dialogs', menus' and panels' loops, compares the shown editor's windows, the
//! nesting depth and the region's display with what it last told. The host records its remote
//! editors after each request and tick, and the editors that went are told closed.

use std::cell::{OnceCell, RefCell};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2_app_kit::NSWindow;
use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopTimer, kCFRunLoopCommonModes,
};
use plughost_core::remote::{EditorId, Notice, Region, Window, WindowKind};

use super::session;
use crate::editor;

/// How often the timer looks.
const INTERVAL: f64 = 0.05;

/// The shown remote editor.
pub struct Shown {
    pub id: EditorId,
    pub window: Retained<NSWindow>,
    pub region: Region,
}

#[derive(Default)]
struct Watch {
    /// The open remote editors and their slots.
    editors: Vec<(EditorId, usize)>,
    shown: Option<Shown>,
    /// The other editors' windows, which are not the shown editor's.
    others: Vec<Retained<NSWindow>>,
    told_windows: Option<(EditorId, Vec<Window>)>,
    told_depth: Option<u32>,
    told_lost: bool,
    /// Windows already moved into the region, which are not moved again.
    placed: Vec<u64>,
}

thread_local! {
    static WATCH: RefCell<Watch> = RefCell::default();
    static TIMER: OnceCell<CFRetained<CFRunLoopTimer>> = const { OnceCell::new() };
}

static START: OnceLock<Instant> = OnceLock::new();
/// When the timer last fired, in milliseconds from `START`.
static FIRED: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Whether the main run loop ran the timer within `within`: the main thread is in a loop that
/// runs, its own or a nested one, rather than held up outside any.
pub fn running_within(within: Duration) -> bool {
    let fired = FIRED.load(Ordering::Relaxed);
    fired != 0 && now().saturating_sub(fired) <= within.as_millis() as u64
}

unsafe extern "C-unwind" fn fired(_timer: *mut CFRunLoopTimer, _info: *mut std::ffi::c_void) {
    FIRED.store(now().max(1), Ordering::Relaxed);
    check();
}

fn start_timer() {
    TIMER.with(|timer| {
        timer.get_or_init(|| {
            // SAFETY: the callout matches the timer's signature and takes no context.
            let created = unsafe {
                CFRunLoopTimer::new(
                    None,
                    CFAbsoluteTimeGetCurrent() + INTERVAL,
                    INTERVAL,
                    0,
                    0,
                    Some(fired),
                    std::ptr::null_mut(),
                )
            }
            .expect("a run loop timer");
            if let Some(main) = CFRunLoop::main() {
                main.add_timer(Some(&created), unsafe { kCFRunLoopCommonModes });
            }
            created
        });
    });
}

/// Records the open remote editors, the shown one and the other editors' windows, and tells
/// the session of the editors that closed since.
pub fn update(
    editors: Vec<(EditorId, usize)>,
    shown: Option<Shown>,
    others: Vec<Retained<NSWindow>>,
) {
    start_timer();
    WATCH.with_borrow_mut(|watch| {
        for (id, slot) in &watch.editors {
            if !editors.iter().any(|(open, _)| open == id) {
                session::notify(Notice::editor_closed(*id, *slot));
            }
        }
        watch.editors = editors;
        watch.shown = shown;
        watch.others = others;
    });
    check();
}

/// The shown remote editor as input sees it.
pub struct Target {
    pub id: EditorId,
    pub window: Retained<NSWindow>,
    pub region: Region,
    /// The other editors' windows, which take no input for this one.
    pub others: Vec<Retained<NSWindow>>,
}

/// The shown remote editor, if one is.
pub fn shown() -> Option<Target> {
    WATCH.with_borrow(|watch| {
        watch.shown.as_ref().map(|shown| Target {
            id: shown.id,
            window: shown.window.clone(),
            region: shown.region,
            others: watch.others.clone(),
        })
    })
}

/// Tells a new session the current state.
pub fn tell_all() {
    WATCH.with_borrow_mut(|watch| {
        watch.told_windows = None;
        watch.told_depth = None;
        watch.told_lost = false;
    });
    super::cursor::tell_all();
    super::menu::tell_all();
    super::panel::tell_all();
    check();
}

fn check() {
    super::menu::expire();
    let depth = crate::events::nested_depth();
    WATCH.with_borrow_mut(|watch| {
        if let Some(shown) = &watch.shown {
            let lost = !editor::region_fits(&shown.region);
            if lost && !watch.told_lost {
                session::notify(Notice::DisplayLost);
            }
            watch.told_lost = lost;
            let mut windows =
                editor::remote_window_list(&shown.window, &shown.region, &watch.others);
            if place_strays(&windows, &shown.region, &mut watch.placed) {
                windows = editor::remote_window_list(&shown.window, &shown.region, &watch.others);
            }
            if watch.told_windows.as_ref() != Some(&(shown.id, windows.clone())) {
                session::notify(Notice::windows(shown.id, windows.clone()));
                watch.told_windows = Some((shown.id, windows));
            }
        }
        if watch.told_depth != Some(depth) {
            session::notify(Notice::nested(depth));
            watch.told_depth = Some(depth);
        }
    });
}

/// Moves dialogs and popups that opened on another display than the region's, such as an alert
/// centered on the main display, to where remote editors open: the application reads only the
/// region. Menus stay where AppKit puts them, and a window is moved once. Returns whether one
/// moved.
fn place_strays(windows: &[Window], region: &Region, placed: &mut Vec<u64>) -> bool {
    let Some(display) = editor::display_in_region(region) else {
        return false;
    };
    placed.retain(|id| windows.iter().any(|window| window.id == *id));
    let mut moved = false;
    for window in windows {
        let center = plughost_core::remote::Rect::new(
            window.frame.x + window.frame.width / 2.0,
            window.frame.y + window.frame.height / 2.0,
            0.0,
            0.0,
        );
        if matches!(window.kind, WindowKind::Dialog | WindowKind::Popup)
            && !display.contains(&center)
            && !placed.contains(&window.id)
        {
            editor::place_in_region(window.id, region);
            placed.push(window.id);
            moved = true;
        }
    }
    moved
}
