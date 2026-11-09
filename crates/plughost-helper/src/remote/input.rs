//! Commands from a remote session, run on the main thread wherever it is: in the helper's own
//! loop, or in a dialog's, a menu's or a panel's nested in it. The session's reader thread queues
//! them and signals a run loop source in the common modes, which every such loop runs.
//!
//! All input goes into the application's event queue, in the order it came, which calls no
//! plugin code here: whatever loop runs then takes it, a dialog's the key before opened
//! included. The application hands each event out, through the helper's mouse's monitor: mouse
//! events go on to their windows, with the tracking areas under a move messaged; keys and wheel
//! events, which AppKit would send to the key window or the real cursor's, go to the window they
//! are for directly. Menu and panel commands, which are no events, run in order from one-shot
//! timers.
//!
//! A relative move goes on from the helper's mouse, which a plugin may warp while it handles the
//! move before. Moves that come before the application handed out the pointer event posted last
//! are summed, and go in once it did, from where the warp left the mouse: posted at once, the
//! plugin would add its warp's offset to each of them again.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock, PoisonError};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSWindow};
use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext,
    CFRunLoopTimer, CGPoint, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{CGEvent, CGEventField, CGScrollEventUnit};
use objc2_foundation::{NSPoint, NSProcessInfo, NSString};
use plughost_core::remote::{
    Button, Command, KeyInput, Modifiers, MotionInput, PanelAnswer, Point, PointerInput,
    PointerKind, WheelInput, WheelUnit,
};

use super::{cursor, hover, menu, mouse, panel, watch};
use crate::editor;

/// How many commands wait for the main thread; the oldest goes past this.
const BACKLOG: usize = 1024;

enum Queued {
    Command(Command),
    /// The session that sent the commands before ended, with no session after it: what it
    /// holds is released, and its menus and panels are closed.
    Ended,
    /// A new session replaced the one before: what that one held is released, and its menus
    /// and panels wait for the new one.
    Replaced,
    /// A session started, which closes menus open past the timeout.
    Started(std::time::Duration),
}

static QUEUE: Mutex<VecDeque<Queued>> = Mutex::new(VecDeque::new());

/// The source the reader thread signals, on the main run loop.
struct Wake {
    source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
}

// SAFETY: signaling a run loop source and waking a run loop are thread-safe.
unsafe impl Send for Wake {}
unsafe impl Sync for Wake {}

static WAKE: OnceLock<Wake> = OnceLock::new();

thread_local! {
    /// The window a held button went down in, the button, and the last point there.
    static PRESSED: RefCell<Option<(Retained<NSWindow>, Button, NSPoint)>> =
        const { RefCell::new(None) };
    /// The window the last pointer down went to, which takes keys and text while it shows.
    static FOCUS: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
    /// The keys down, each with the window its key down went to.
    static HELD: RefCell<Vec<(KeyInput, Retained<NSWindow>)>> = const { RefCell::new(Vec::new()) };
    /// The modifiers held.
    static MODIFIERS: Cell<Modifiers> = const { Cell::new(Modifiers::empty()) };
    /// The menu and panel commands to run, in order, one per timer.
    static DELIVERIES: RefCell<VecDeque<Command>> = const { RefCell::new(VecDeque::new()) };
    /// The events posted and not yet handed out, by tag, with what each is for.
    static POSTED: RefCell<VecDeque<(i64, Posted)>> = const { RefCell::new(VecDeque::new()) };
    /// The tag of the pointer event posted last, until the application hands it out.
    static AWAITING: Cell<Option<i64>> = const { Cell::new(None) };
    /// The relative moves summed meanwhile.
    static PENDING: Cell<Option<MotionInput>> = const { Cell::new(None) };
    /// The tag of the event posted last.
    static TAG: Cell<i64> = const { Cell::new(0) };
}

/// What a posted event is for, once the application hands it out.
enum Posted {
    /// A mouse event, which goes on to its window.
    Mouse,
    /// A move with no button down over a window, to a point of the screen, whose tracking areas
    /// are messaged as it goes on.
    Hover(Retained<NSWindow>, NSPoint, NSEventModifierFlags),
    /// A key or wheel event for a window, which goes to it directly.
    Direct(Retained<NSWindow>),
}

unsafe extern "C-unwind" fn drained(_info: *mut std::ffi::c_void) {
    drain();
}

/// A posted event's tag: its timestamp in whole microseconds. A posted event goes into the
/// queue as a new event with its own event number, keeping only the timestamp, to the
/// nanosecond.
fn tag(timestamp: f64) -> i64 {
    (timestamp * 1e6).round() as i64
}

/// The tag for the next event posted: each its own, a microsecond past the one before at least.
fn next_tag() -> i64 {
    let tag = tag(NSProcessInfo::processInfo().systemUptime()).max(TAG.get() + 1);
    TAG.set(tag);
    tag
}

/// Posts `event`, tagged `tag`, for what `posted` says.
fn post_tagged(event: &NSEvent, tag: i64, posted: Posted, mtm: MainThreadMarker) {
    POSTED.with_borrow_mut(|queue| queue.push_back((tag, posted)));
    NSApplication::sharedApplication(mtm).postEvent_atStart(event, false);
}

/// The application hands out `event`, before it goes to its window: does with a posted one what
/// it was posted for, and tells whether it took the event.
fn handed(event: &NSEvent) -> bool {
    let tag = tag(event.timestamp());
    let posted = POSTED.with_borrow_mut(|queue| {
        let index = queue.iter().position(|(posted, _)| *posted == tag)?;
        // Those before it went to a loop that takes events itself, as a menu's does.
        queue.drain(..index);
        queue.pop_front().map(|(_, posted)| posted)
    });
    let Some(posted) = posted else {
        return false;
    };
    match posted {
        Posted::Mouse => false,
        Posted::Hover(window, point, flags) => {
            hover::moved(&window, point, flags);
            false
        }
        Posted::Direct(window) => {
            window.sendEvent(event);
            true
        }
    }
}

/// The application took `event` from its queue, for its own loop or one that takes events itself,
/// as a control's tracking does: the moves summed while the pointer event posted last waited go
/// in.
fn dequeued(event: &NSEvent) {
    if AWAITING.get() == Some(tag(event.timestamp())) {
        AWAITING.set(None);
        if let Some(pending) = PENDING.take() {
            // Run after the event's window handled it, which may warp the mouse.
            queue().push_front(Queued::Command(Command::Motion(pending)));
            wake();
        }
    }
}

/// Installs the source on the main run loop and the helper's mouse. Call it on the main thread
/// before a session starts.
pub fn install(mtm: MainThreadMarker) {
    mouse::install(mtm, handed, dequeued);
    menu::install(mtm);
    panel::install(mtm);
    WAKE.get_or_init(|| {
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
            perform: Some(drained),
        };
        // SAFETY: the context is a valid version 0 context, copied by the call.
        let source = unsafe { CFRunLoopSource::new(None, 0, &mut context) }
            .expect(crate::errors::RUN_LOOP_SOURCE);
        let run_loop = CFRunLoop::main().expect(crate::errors::MAIN_RUN_LOOP);
        run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
        Wake { source, run_loop }
    });
}

fn queue() -> std::sync::MutexGuard<'static, VecDeque<Queued>> {
    QUEUE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wake() {
    if let Some(wake) = WAKE.get() {
        wake.source.signal();
        wake.run_loop.wake_up();
    }
}

/// Queues `command` from the reader thread. A move without a button change replaces a move
/// still waiting before it, and a relative move adds to one waiting before it. A full queue drops
/// its oldest move or wheel command, or else its oldest command, never a session's start or end.
pub fn receive(mut command: Command) {
    {
        let mut queue = queue();
        let replaces = match (&command, queue.back()) {
            (
                Command::Pointer(PointerInput {
                    kind: PointerKind::Move,
                    editor,
                    ..
                }),
                Some(Queued::Command(Command::Pointer(PointerInput {
                    kind: PointerKind::Move,
                    editor: before,
                    ..
                }))),
            ) => editor == before,
            (Command::Motion(input), Some(Queued::Command(Command::Motion(before))))
                if input.editor == before.editor =>
            {
                command = Command::Motion(MotionInput::new(
                    input.editor,
                    before.dx + input.dx,
                    before.dy + input.dy,
                    input.modifiers,
                ));
                true
            }
            _ => false,
        };
        if replaces {
            queue.pop_back();
        } else if queue.len() >= BACKLOG {
            let oldest = queue
                .iter()
                .position(|queued| {
                    matches!(
                        queued,
                        Queued::Command(
                            Command::Pointer(PointerInput {
                                kind: PointerKind::Move,
                                ..
                            }) | Command::Motion(_)
                                | Command::Wheel(_)
                        )
                    )
                })
                .or_else(|| {
                    queue
                        .iter()
                        .position(|queued| matches!(queued, Queued::Command(_)))
                });
            if let Some(oldest) = oldest {
                queue.remove(oldest);
            }
        }
        queue.push_back(Queued::Command(command));
    }
    wake();
}

/// Queues the end of the session from the reader thread: what it holds is released.
pub fn ended() {
    queue().push_back(Queued::Ended);
    wake();
}

/// Queues the replacement of the session by a new one from the reader thread.
pub fn replaced() {
    queue().push_back(Queued::Replaced);
    wake();
}

/// Queues the start of a session: it is told the current state.
pub fn started(menu_timeout: std::time::Duration) {
    queue().push_back(Queued::Started(menu_timeout));
    wake();
}

fn drain() {
    loop {
        let Some(queued) = queue().pop_front() else {
            return;
        };
        match queued {
            Queued::Command(command) => run(command),
            Queued::Ended => {
                release();
                menu::close_all();
                for id in panel::waiting() {
                    deliver(Command::AnswerPanel(PanelAnswer::new(id, None)));
                }
            }
            Queued::Replaced => release(),
            Queued::Started(menu_timeout) => {
                menu::set_timeout(menu_timeout);
                watch::tell_all();
            }
        }
    }
}

fn run(command: Command) {
    // A menu or a panel is the one its identifier names, whichever editor opened it.
    if matches!(
        command,
        Command::ChooseMenuItem(_) | Command::CloseMenu(_) | Command::AnswerPanel(_)
    ) {
        deliver(command);
        return;
    }
    // The real panel's modal loop drops input to other windows.
    if panel::modal()
        && matches!(
            command,
            Command::Pointer(_)
                | Command::Motion(_)
                | Command::Wheel(_)
                | Command::Key(_)
                | Command::Text(_)
        )
    {
        return;
    }
    // A point or distance that is not finite would place the helper's mouse nowhere.
    if !finite(&command) {
        return;
    }
    let Some(shown) = watch::shown() else {
        return;
    };
    let id = match &command {
        Command::Pointer(input) => input.editor,
        Command::Motion(input) => input.editor,
        Command::Wheel(input) => input.editor,
        Command::Key(input) => input.editor,
        Command::Text(input) => input.editor,
        Command::Modifiers(input) => input.editor,
        Command::ReleaseAll(editor) => *editor,
        _ => return,
    };
    // Input for an editor shown before, or for one this helper no longer shows.
    if id != shown.id {
        return;
    }
    let target = Target {
        editor: shown.window,
        region: shown.region,
        others: shown.others,
    };
    match command {
        Command::Pointer(input) => {
            // A point the application placed the pointer at supersedes moves from where it was.
            PENDING.set(None);
            target.pointer(&input);
        }
        Command::Motion(input) => target.motion(input),
        Command::Modifiers(input) => {
            MODIFIERS.set(input.modifiers);
            mouse::set_modifiers(flags(Modifiers::empty()));
        }
        Command::ReleaseAll(_) => release(),
        Command::Wheel(input) => target.wheel(&input),
        Command::Key(input) => key(&input, &target.editor),
        Command::Text(input) => {
            let window = focus(&target.editor);
            for character in input.text.chars() {
                let text = character.to_string();
                for down in [true, false] {
                    post_key(
                        &KeyInput::new(
                            input.editor,
                            down,
                            0,
                            text.clone(),
                            text.clone(),
                            false,
                            Modifiers::empty(),
                        ),
                        &window,
                    );
                }
            }
        }
        _ => {}
    }
}

/// Whether the command's points and distances are finite.
fn finite(command: &Command) -> bool {
    let point = |at: &Point| at.x.is_finite() && at.y.is_finite();
    match command {
        Command::Pointer(input) => point(&input.at),
        Command::Motion(input) => input.dx.is_finite() && input.dy.is_finite(),
        Command::Wheel(input) => point(&input.at) && input.dx.is_finite() && input.dy.is_finite(),
        _ => true,
    }
}

/// Where input goes: the shown editor's region and windows.
struct Target {
    editor: Retained<NSWindow>,
    region: plughost_core::remote::Region,
    others: Vec<Retained<NSWindow>>,
}

impl Target {
    /// The helper's topmost window under `at` that takes the mouse, and the screen point.
    fn window_at(&self, at: Point) -> Option<(Retained<NSWindow>, NSPoint)> {
        let point = editor::screen_point(&self.region, at)?;
        Some((self.window_at_screen(point), point))
    }

    /// The helper's topmost window under the screen point that takes the mouse, or the editor's.
    fn window_at_screen(&self, point: NSPoint) -> Retained<NSWindow> {
        editor::front_to_back(&self.others)
            .into_iter()
            .find(|window| {
                let frame = window.frame();
                !window.ignoresMouseEvents()
                    && point.x >= frame.origin.x
                    && point.y >= frame.origin.y
                    && point.x < frame.origin.x + frame.size.width
                    && point.y < frame.origin.y + frame.size.height
            })
            .unwrap_or_else(|| self.editor.clone())
    }

    fn pointer(&self, input: &PointerInput) {
        let Some(point) = editor::screen_point(&self.region, input.at) else {
            return;
        };
        let flags = flags(input.modifiers);
        match input.kind {
            PointerKind::Down => {
                let Some((window, point)) = self.window_at(input.at) else {
                    return;
                };
                // The helper never activates, so nothing raises the window a click lands on.
                editor::raise(&window);
                // One button is held at a time: the one held before goes up first.
                if let Some((held, button, last)) = PRESSED.take() {
                    post(&held, up(button), last, 1, flags, Posted::Mouse);
                }
                post(
                    &window,
                    down(input.button),
                    point,
                    input.clicks,
                    flags,
                    Posted::Mouse,
                );
                FOCUS.set(Some(window.clone()));
                PRESSED.set(Some((window, input.button, point)));
            }
            PointerKind::Up => {
                if let Some((window, button, _)) = PRESSED.take() {
                    post(
                        &window,
                        up(button),
                        point,
                        input.clicks,
                        flags,
                        Posted::Mouse,
                    );
                }
            }
            PointerKind::Move => {
                let dragged = PRESSED.with_borrow_mut(|pressed| {
                    pressed.as_mut().map(|(window, button, last)| {
                        *last = point;
                        (window.clone(), *button)
                    })
                });
                match dragged {
                    Some((window, button)) => {
                        cursor::moved();
                        post(&window, drag(button), point, 1, flags, Posted::Mouse);
                    }
                    None => {
                        if let Some((window, point)) = self.window_at(input.at) {
                            self.hover(window, point, flags);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Moves the pointer by the input's distances from the helper's mouse, or sums them while
    /// the pointer event posted last waits.
    fn motion(&self, input: MotionInput) {
        if AWAITING.get().is_some() {
            let pending = PENDING.get().map_or(input, |pending| {
                MotionInput::new(
                    input.editor,
                    pending.dx + input.dx,
                    pending.dy + input.dy,
                    input.modifiers,
                )
            });
            PENDING.set(Some(pending));
            return;
        }
        let from = mouse::location();
        // AppKit's screen points run up.
        let point = NSPoint::new(from.x + input.dx, from.y - input.dy);
        let flags = flags(input.modifiers);
        let dragged = PRESSED.with_borrow_mut(|pressed| {
            pressed.as_mut().map(|(window, button, last)| {
                *last = point;
                (window.clone(), *button)
            })
        });
        match dragged {
            Some((window, button)) => {
                cursor::moved();
                post(&window, drag(button), point, 1, flags, Posted::Mouse);
            }
            None => {
                let window = self.window_at_screen(point);
                self.hover(window, point, flags);
            }
        }
    }

    /// Moves the pointer over `window` with no button down: posts the move, which messages the
    /// tracking areas under it as the application hands it out.
    fn hover(&self, window: Retained<NSWindow>, point: NSPoint, flags: NSEventModifierFlags) {
        cursor::moved();
        let posted = Posted::Hover(window.clone(), point, flags);
        post(&window, NSEventType::MouseMoved, point, 0, flags, posted);
    }
}

/// Releases a held button, the keys down and the modifiers, and drops relative moves not yet
/// made.
fn release() {
    PENDING.set(None);
    if let Some((window, button, point)) = PRESSED.take() {
        let flags = NSEventModifierFlags::empty();
        post(&window, up(button), point, 1, flags, Posted::Mouse);
    }
    for (mut input, window) in HELD.take() {
        input.down = false;
        input.repeat = false;
        input.modifiers = Modifiers::empty();
        post_key(&input, &window);
    }
    MODIFIERS.set(Modifiers::empty());
    mouse::set_modifiers(NSEventModifierFlags::empty());
}

fn down(button: Button) -> NSEventType {
    match button {
        Button::Right => NSEventType::RightMouseDown,
        Button::Left => NSEventType::LeftMouseDown,
        _ => NSEventType::OtherMouseDown,
    }
}

fn up(button: Button) -> NSEventType {
    match button {
        Button::Right => NSEventType::RightMouseUp,
        Button::Left => NSEventType::LeftMouseUp,
        _ => NSEventType::OtherMouseUp,
    }
}

fn drag(button: Button) -> NSEventType {
    match button {
        Button::Right => NSEventType::RightMouseDragged,
        Button::Left => NSEventType::LeftMouseDragged,
        _ => NSEventType::OtherMouseDragged,
    }
}

fn flags(modifiers: Modifiers) -> NSEventModifierFlags {
    let modifiers = modifiers | MODIFIERS.get();
    let mut flags = NSEventModifierFlags::empty();
    for (modifier, flag) in [
        (Modifiers::SHIFT, NSEventModifierFlags::Shift),
        (Modifiers::CONTROL, NSEventModifierFlags::Control),
        (Modifiers::OPTION, NSEventModifierFlags::Option),
        (Modifiers::COMMAND, NSEventModifierFlags::Command),
        (Modifiers::CAPS_LOCK, NSEventModifierFlags::CapsLock),
    ] {
        if modifiers.contains(modifier) {
            flags |= flag;
        }
    }
    flags
}

/// A point of the screen in the window's base coordinates, up from its bottom-left corner.
fn in_window(window: &NSWindow, point: NSPoint) -> NSPoint {
    let frame = window.frame();
    NSPoint::new(point.x - frame.origin.x, point.y - frame.origin.y)
}

/// Posts a mouse event for `window` to the application's queue.
fn post(
    window: &NSWindow,
    kind: NSEventType,
    point: NSPoint,
    clicks: u32,
    flags: NSEventModifierFlags,
    posted: Posted,
) {
    let pressure = match kind {
        NSEventType::MouseMoved
        | NSEventType::LeftMouseUp
        | NSEventType::RightMouseUp
        | NSEventType::OtherMouseUp => 0.0,
        _ => 1.0,
    };
    let tag = next_tag();
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind,
        in_window(window, point),
        flags,
        tag as f64 / 1e6,
        window.windowNumber(),
        None,
        0,
        clicks as isize,
        pressure,
    );
    if let Some(event) = event {
        AWAITING.set(Some(tag));
        post_tagged(&event, tag, posted, window.mtm());
    }
}

/// Runs a menu or panel command from a one-shot timer: choosing an item or answering a panel
/// calls plugin code, which may run a loop of its own. Each timer runs the first command waiting,
/// so they run in the order they came whichever timer fires first.
fn deliver(command: Command) {
    DELIVERIES.with_borrow_mut(|deliveries| deliveries.push_back(command));
    if !arm(0.0) {
        DELIVERIES.with_borrow_mut(VecDeque::pop_back);
    }
}

/// How long a command that came during a wait waits before it is tried again.
const AFTER_WAIT: f64 = 0.01;

/// Adds a timer that runs the first command waiting `after` seconds from now, in every mode.
fn arm(after: f64) -> bool {
    let mut context = objc2_core_foundation::CFRunLoopTimerContext {
        version: 0,
        info: std::ptr::null_mut(),
        retain: None,
        release: None,
        copyDescription: None,
    };
    let fire = CFAbsoluteTimeGetCurrent() + after;
    // SAFETY: the context is a valid version 0 context with no info, copied by the call.
    let timer =
        unsafe { CFRunLoopTimer::new(None, fire, 0.0, 0, 0, Some(delivered), &mut context) };
    let (Some(timer), Some(main)) = (timer, CFRunLoop::main()) else {
        return false;
    };
    main.add_timer(Some(&timer), unsafe { kCFRunLoopCommonModes });
    true
}

/// A command that comes while host code waits in a request runs after the wait, as one acting
/// on the machine could not act before it either.
unsafe extern "C-unwind" fn delivered(_timer: *mut CFRunLoopTimer, _info: *mut std::ffi::c_void) {
    if crate::events::in_wait() && arm(AFTER_WAIT) {
        return;
    }
    match DELIVERIES.with_borrow_mut(VecDeque::pop_front) {
        Some(Command::ChooseMenuItem(choice)) => menu::choose(&choice),
        Some(Command::CloseMenu(id)) => menu::close(id),
        Some(Command::AnswerPanel(answer)) => panel::answer(&answer),
        _ => {}
    }
}

impl Target {
    /// Posts a scroll event for the window under the point. A scroll event has no NSEvent
    /// constructor, and one converted from a CGEvent names no window, so its location reads as
    /// the window's: the CGEvent is placed where that is the point in the window.
    fn wheel(&self, input: &WheelInput) {
        let Some((window, point)) = self.window_at(input.at) else {
            return;
        };
        let local = in_window(&window, point);
        let (unit, dy, dx) = match input.unit {
            WheelUnit::Point => (CGScrollEventUnit::Pixel, -input.dy, -input.dx),
            _ => (CGScrollEventUnit::Line, -input.dy, -input.dx),
        };
        let Some(event) = CGEvent::new_scroll_wheel_event2(
            None,
            unit,
            2,
            dy.round() as i32,
            dx.round() as i32,
            0,
        ) else {
            return;
        };
        CGEvent::set_location(
            Some(&event),
            CGPoint::new(local.x, editor::main_height() - local.y),
        );
        let number = window.windowNumber() as i64;
        for field in [
            CGEventField::MouseEventWindowUnderMousePointer,
            CGEventField::MouseEventWindowUnderMousePointerThatCanHandleThisEvent,
        ] {
            CGEvent::set_integer_value_field(Some(&event), field, number);
        }
        let flags = flags(input.modifiers);
        CGEvent::set_flags(
            Some(&event),
            objc2_core_graphics::CGEventFlags(flags.0 as u64),
        );
        let tag = next_tag();
        CGEvent::set_timestamp(Some(&event), tag as u64 * 1000);
        if let Some(converted) = NSEvent::eventWithCGEvent(&event) {
            post_tagged(
                &converted,
                tag,
                Posted::Direct(window.clone()),
                window.mtm(),
            );
        }
    }
}

/// The window that takes keys and text: the one the last pointer down went to while it shows,
/// or else the shown editor's.
fn focus(editor: &Retained<NSWindow>) -> Retained<NSWindow> {
    FOCUS.with_borrow_mut(|focus| {
        if focus.as_ref().is_some_and(|window| !window.isVisible()) {
            *focus = None;
        }
        focus.clone().unwrap_or_else(|| editor.clone())
    })
}

/// Posts a key event for the window that takes keys, and keeps the key while it is down. A key
/// goes up in the window it went down in.
fn key(input: &KeyInput, editor: &Retained<NSWindow>) {
    let held = HELD.with_borrow_mut(|held| {
        let index = held
            .iter()
            .position(|(down, _)| down.key_code == input.key_code)?;
        if input.down {
            Some(held[index].1.clone())
        } else {
            Some(held.remove(index).1)
        }
    });
    let window = held.unwrap_or_else(|| focus(editor));
    if input.down && !input.repeat {
        HELD.with_borrow_mut(|held| {
            if !held.iter().any(|(down, _)| down.key_code == input.key_code) {
                held.push((input.clone(), window.clone()));
            }
        });
    }
    post_key(input, &window);
}

/// Posts a key event for `window`. It is not the key window, which the helper never makes one,
/// so the event goes to it directly once handed out.
fn post_key(input: &KeyInput, window: &Retained<NSWindow>) {
    let flags = flags(input.modifiers);
    let tag = next_tag();
    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        if input.down {
            NSEventType::KeyDown
        } else {
            NSEventType::KeyUp
        },
        NSPoint::new(0.0, 0.0),
        flags,
        tag as f64 / 1e6,
        window.windowNumber(),
        None,
        &NSString::from_str(&input.characters),
        &NSString::from_str(&input.characters_ignoring_modifiers),
        input.repeat,
        input.key_code,
    );
    if let Some(event) = event {
        post_tagged(&event, tag, Posted::Direct(window.clone()), window.mtm());
    }
}

#[cfg(test)]
mod tests {
    use plughost_core::remote::EditorId;

    use super::*;

    /// The queue is the process's; one test uses it at a time.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wheel(editor: EditorId, dy: f64) -> Command {
        Command::Wheel(WheelInput::new(
            editor,
            Point::new(0.0, 0.0),
            0.0,
            dy,
            WheelUnit::Line,
            Modifiers::empty(),
        ))
    }

    #[test]
    fn a_full_queue_keeps_the_session_start_and_drops_the_oldest_wheel_first() {
        let _serial = serial();
        let editor = EditorId(1);
        queue().clear();
        started(std::time::Duration::from_secs(1));
        let key = Command::Key(KeyInput::new(
            editor,
            true,
            0,
            "a",
            "a",
            false,
            Modifiers::empty(),
        ));
        receive(key.clone());
        let count = 2 * BACKLOG;
        for dy in 0..count {
            receive(wheel(editor, dy as f64));
        }
        let queued: Vec<Queued> = queue().drain(..).collect();
        assert_eq!(queued.len(), BACKLOG);
        assert!(matches!(queued[0], Queued::Started(_)));
        assert!(matches!(&queued[1], Queued::Command(command) if *command == key));
        assert!(matches!(
            queued.last(),
            Some(Queued::Command(command)) if *command == wheel(editor, (count - 1) as f64)
        ));
    }

    #[test]
    fn relative_moves_waiting_in_a_row_add_up() {
        let _serial = serial();
        let editor = EditorId(2);
        queue().clear();
        receive(Command::Motion(MotionInput::new(
            editor,
            1.0,
            2.0,
            Modifiers::empty(),
        )));
        receive(Command::Motion(MotionInput::new(
            editor,
            3.0,
            4.0,
            Modifiers::SHIFT,
        )));
        let queued: Vec<Queued> = queue().drain(..).collect();
        assert!(matches!(
            queued.as_slice(),
            [Queued::Command(Command::Motion(motion))]
                if *motion == MotionInput::new(editor, 4.0, 6.0, Modifiers::SHIFT)
        ));
    }
}
