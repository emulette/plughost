//! Remote editors of a real helper started in remote mode on the macOS desktop. The region is on
//! the display `PLUGHOST_TEST_DISPLAY` names, or the main display; tests do not create a virtual
//! display, which leaves load on the system until a restart. Build the helper and the test
//! plugins first (see `support`).
#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

use plughost::remote::{
    Button, Command, CursorShape, EDITOR_INSET, EditorId, KeyInput, MenuChoice, MenuId, MenuItem,
    MenuItemState, Modifiers, MotionInput, Notice, PanelAnswer, PanelId, PanelKind, Point,
    PointerInput, PointerKind, Rect, Region, Sender, Session, TextInput, WheelInput, WheelUnit,
    Window, WindowKind,
};
use plughost::{
    Chain, ChainSlot, Error, FailureKind, InputError, PluginFormat, PluginRef, SpawnOptions,
    Timeouts,
};

mod support;

use support::desktop::{
    CGMainDisplayID, act, cursor, display_origin, frontmost, move_machine_cursor, test_display,
    value, wait, windows,
};
use support::{delay_variant, fixture, helper, spawn};

/// The delay fixture's editor variant: a 240 by 120 editor that drags its window wider once
/// open and refuses that size.
fn editor() -> PluginRef {
    delay_variant(PluginFormat::Clap, "editor", 0xB)
}

fn spawn_remote(plugins: &[PluginRef]) -> Chain {
    spawn_remote_with(plugins, Timeouts::default())
}

fn spawn_remote_with(plugins: &[PluginRef], timeouts: Timeouts) -> Chain {
    let mut options = SpawnOptions::default();
    options.remote = true;
    options.timeouts = timeouts;
    let slots: Vec<ChainSlot> = plugins.iter().cloned().map(ChainSlot::from).collect();
    Chain::spawn(&helper(), &slots, &options).unwrap()
}

fn region(x: f64, y: f64, width: f64, height: f64) -> Region {
    Region::new(test_display(), Rect::new(x, y, width, height))
}

/// A window at `(x, y)` on the test display, in the window server's global points.
fn global(x: f64, y: f64, width: f64, height: f64) -> (f64, f64, f64, f64) {
    let (left, top) = display_origin(test_display());
    (left + x, top + y, width, height)
}

/// Windows without their numbers.
fn layout(windows: &[Window]) -> Vec<(WindowKind, Rect, bool)> {
    windows
        .iter()
        .map(|window| (window.kind, window.frame, window.outside))
        .collect()
}

/// Reads notices until one satisfies `check`, which it returns, for up to five seconds.
fn until(session: &mut Session, mut check: impl FnMut(&Notice) -> bool, what: &str) -> Notice {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "{what}");
        if let Some(notice) = session.recv(Duration::from_millis(100)).unwrap()
            && check(&notice)
        {
            return notice;
        }
    }
}

/// Reads notices until the windows of `editor` have `expected` layout, and returns them.
fn windows_until(
    session: &mut Session,
    editor: EditorId,
    expected: &[(WindowKind, Rect, bool)],
    what: &str,
) -> Vec<Window> {
    match until(
        session,
        |notice| {
            matches!(notice, Notice::Windows { editor: shown, windows, .. }
                if *shown == editor && layout(windows) == expected)
        },
        what,
    ) {
        Notice::Windows { windows, .. } => windows,
        _ => unreachable!(),
    }
}

/// The helper's windows on screen: place and size.
fn on_screen(pid: u32) -> Vec<(f64, f64, f64, f64)> {
    windows(pid)
        .into_iter()
        .filter(|window| window.on_screen)
        .map(|window| (window.x, window.y, window.width, window.height))
        .collect()
}

fn unsupported(result: Result<impl std::fmt::Debug, Error>) {
    match result {
        Err(Error::Operation { failure, .. }) => assert_eq!(failure.kind, FailureKind::Unsupported),
        other => panic!("expected an unsupported operation, got {other:?}"),
    }
}

fn input(result: Result<impl std::fmt::Debug, Error>, expected: InputError) {
    match result {
        Err(Error::Input { error, .. }) => assert_eq!(error, expected),
        other => panic!("expected {expected:?}, got {other:?}"),
    }
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_remote_editor_opens_in_its_region_and_the_helper_never_activates() {
    const REFUSED_SIZES: u64 = 16;
    const APPLICATION_ACTIVE: u64 = 18;
    let _serial = support::editor_test();
    let mut chain = spawn_remote(&[editor()]);
    let pid = chain.helper_monitor().process_id();
    let mut session = chain.remote_session().unwrap();
    chain
        .set_remote_region(region(100.0, 100.0, 300.0, 200.0))
        .unwrap();
    let opened = chain.open_remote_editor(0).unwrap();
    assert_eq!((opened.width, opened.height), (240.0, 120.0));
    let editor = (
        WindowKind::Editor,
        Rect::new(EDITOR_INSET, EDITOR_INSET, 240.0, 120.0),
        false,
    );
    // The editor drags its window wider and refuses the size; the window keeps its place.
    wait(
        &mut chain,
        |chain| value(chain, REFUSED_SIZES) >= 1.0,
        "the editor was never offered the dragged size",
    );
    windows_until(
        &mut session,
        opened.editor,
        &[editor],
        "the editor's window is not 20 points into the region",
    );
    // The window server follows the window's restored size a little later.
    wait(
        &mut chain,
        |_| on_screen(pid) == [global(120.0, 120.0, 240.0, 120.0)],
        "the window server does not show the editor's window in place",
    );
    assert_ne!(frontmost(), Some(pid as i32));
    // A click in a JUCE editor activates its application. The helper stays behind, and tells
    // the plugin it is active.
    act(&mut chain, 4);
    wait(
        &mut chain,
        |chain| value(chain, APPLICATION_ACTIVE) == 1.0,
        "the plugin was not told its application is active",
    );
    assert_ne!(frontmost(), Some(pid as i32));
    // A popup the plugin opens comes first, here past the region's right edge.
    act(&mut chain, 5);
    let popup = (WindowKind::Popup, Rect::new(270.0, 120.0, 60.0, 40.0), true);
    windows_until(
        &mut session,
        opened.editor,
        &[popup, editor],
        "the popup is not listed in front of the editor",
    );
    // A new region moves the editor.
    chain
        .set_remote_region(region(200.0, 150.0, 300.0, 200.0))
        .unwrap();
    wait(
        &mut chain,
        |_| on_screen(pid).contains(&global(220.0, 170.0, 240.0, 120.0)),
        "the editor did not move to the new region",
    );
    chain.close_editor(0).unwrap();
    assert!(!chain.editor_open(0).unwrap());
    until(
        &mut session,
        |notice| *notice == Notice::editor_closed(opened.editor, 0),
        "the session was not told the editor closed",
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_remote_chain_shows_one_editor_and_keeps_the_others_open() {
    let _serial = support::editor_test();
    let mut chain = spawn_remote(&[editor(), editor()]);
    let pid = chain.helper_monitor().process_id();
    let mut session = chain.remote_session().unwrap();
    chain
        .set_remote_region(region(100.0, 100.0, 300.0, 200.0))
        .unwrap();
    let editor = (
        WindowKind::Editor,
        Rect::new(EDITOR_INSET, EDITOR_INSET, 240.0, 120.0),
        false,
    );
    let first = chain.open_remote_editor(0).unwrap();
    let first_window = windows_until(&mut session, first.editor, &[editor], "first editor")[0].id;
    let second = chain.open_remote_editor(1).unwrap();
    assert_ne!(first.editor, second.editor);
    let second_window =
        windows_until(&mut session, second.editor, &[editor], "second editor")[0].id;
    assert_ne!(first_window, second_window);
    wait(
        &mut chain,
        |_| on_screen(pid).len() == 1,
        "the first editor was not hidden",
    );
    // Showing the first again shows its window: it was hidden, not closed.
    chain.show_remote_editor(0).unwrap();
    let shown = windows_until(&mut session, first.editor, &[editor], "first editor again");
    assert_eq!(shown[0].id, first_window);
    assert_eq!(chain.open_remote_editor(0).unwrap().editor, first.editor);
    assert!(chain.editor_open(1).unwrap());
    assert_eq!(on_screen(pid).len(), 1);
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_region_off_its_display_changes_nothing() {
    let _serial = support::editor_test();
    let mut chain = spawn_remote(&[editor()]);
    let pid = chain.helper_monitor().process_id();
    // Opening needs a region first.
    input(chain.open_remote_editor(0), InputError::RemoteRegion);
    chain
        .set_remote_region(region(100.0, 100.0, 300.0, 200.0))
        .unwrap();
    chain.open_remote_editor(0).unwrap();
    for bad in [
        region(f64::NAN, 0.0, 300.0, 200.0),
        region(0.0, 0.0, 0.0, 200.0),
        region(-10.0, 0.0, 300.0, 200.0),
        region(0.0, 0.0, 100_000.0, 200.0),
        Region::new(u32::MAX, Rect::new(0.0, 0.0, 300.0, 200.0)),
    ] {
        input(chain.set_remote_region(bad), InputError::RemoteRegion);
    }
    wait(
        &mut chain,
        |_| on_screen(pid).contains(&global(120.0, 120.0, 240.0, 120.0)),
        "the editor left its region",
    );
    assert!(chain.editor_open(0).unwrap());
}

#[test]
#[ignore = "needs helper, delay and synth fixtures and a desktop session"]
fn editors_a_remote_chain_cannot_show_are_refused() {
    let _serial = support::editor_test();
    let synth = fixture(
        PluginFormat::Clap,
        "plughost-test-synth",
        "com.studio.plughost.test-synth",
    );
    let mut chain = spawn_remote(&[editor(), synth]);
    chain
        .set_remote_region(region(100.0, 100.0, 300.0, 200.0))
        .unwrap();
    // A local editor would open where the application does not read it.
    unsupported(chain.open_editor(0));
    input(chain.show_remote_editor(0), InputError::RemoteEditor);
    // The synth draws only in its own window, which cannot be placed in the region.
    unsupported(chain.open_remote_editor(1));
    assert!(!chain.editor_open(1).unwrap());
    // A chain not started in remote mode opens no remote editors.
    let mut local = spawn(&[editor()]);
    unsupported(local.set_remote_region(region(100.0, 100.0, 300.0, 200.0)));
    unsupported(local.open_remote_editor(0));
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_dialog_the_plugin_runs_keeps_the_chain_alive_past_its_timeout() {
    let _serial = support::editor_test();
    let mut timeouts = Timeouts::default();
    timeouts.control = Duration::from_secs(2);
    let mut chain = spawn_remote_with(&[editor()], timeouts);
    let mut session = chain.remote_session().unwrap();
    chain
        .set_remote_region(region(100.0, 100.0, 300.0, 200.0))
        .unwrap();
    chain.open_remote_editor(0).unwrap();
    until(
        &mut session,
        |notice| *notice == Notice::nested(0),
        "not told the depth",
    );
    // The editor runs a dialog for eight seconds, as an About box does until someone closes it.
    // The inactive plugin takes the edit that asks for it at once; the session is told of the
    // dialog's loop and its window, in either order.
    chain.set_parameter_text(0, 17, "6").unwrap();
    let (mut nested, mut listed) = (false, false);
    until(
        &mut session,
        |notice| {
            nested |= *notice == Notice::nested(1);
            listed |= matches!(notice, Notice::Windows { windows, .. }
                if windows.iter().any(|window| window.kind == WindowKind::Dialog));
            nested && listed
        },
        "not told the dialog's loop and window",
    );
    // A request waits for the dialog, past the two-second control timeout, and the helper lives.
    let started = Instant::now();
    chain.parameters(0).unwrap();
    assert!(started.elapsed() > Duration::from_secs(2));
    until(
        &mut session,
        |notice| *notice == Notice::nested(0),
        "not told the dialog ended",
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_session_starts_with_the_current_state_and_a_later_one_ends_it() {
    let _serial = support::editor_test();
    let mut chain = spawn_remote(&[editor()]);
    chain
        .set_remote_region(region(100.0, 100.0, 300.0, 200.0))
        .unwrap();
    let opened = chain.open_remote_editor(0).unwrap();
    let mut first = chain.remote_session().unwrap();
    until(
        &mut first,
        |notice| matches!(notice, Notice::Windows { editor, .. } if *editor == opened.editor),
        "the first session was not told the shown editor's windows",
    );
    let mut second = chain.remote_session().unwrap();
    until(
        &mut second,
        |notice| matches!(notice, Notice::Windows { editor, .. } if *editor == opened.editor),
        "the second session was not told the shown editor's windows",
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match first.recv(Duration::from_millis(100)) {
            Err(Error::RemoteSessionEnded) => break,
            Ok(_) => assert!(Instant::now() < deadline, "the first session did not end"),
            Err(error) => panic!("{error:?}"),
        }
    }
}

#[test]
#[ignore = "needs helper and delay fixtures, a desktop session and PLUGHOST_TEST_DISPLAY set to another display than the main one"]
fn a_dialog_opened_on_another_display_moves_into_the_region() {
    let _serial = support::editor_test();
    assert_ne!(test_display(), unsafe { CGMainDisplayID() });
    // The editor's dialog opens at the main display's corner, where the application does not
    // read; the helper moves it into the region: to the editor's right where the region has
    // room, else below it, else over it one step (24 points) down and to the right.
    for (width, height, place) in [
        (600.0, 300.0, "to the right of"),
        (400.0, 300.0, "below"),
        (300.0, 200.0, "over"),
    ] {
        let mut chain = spawn_remote(&[editor()]);
        let mut session = chain.remote_session().unwrap();
        chain
            .set_remote_region(region(100.0, 100.0, width, height))
            .unwrap();
        chain.open_remote_editor(0).unwrap();
        chain.set_parameter_text(0, 17, "6").unwrap();
        until(
            &mut session,
            |notice| {
                let Notice::Windows { windows, .. } = notice else {
                    return false;
                };
                let Some(editor) = windows
                    .iter()
                    .find(|window| window.kind == WindowKind::Editor)
                    .map(|window| window.frame)
                else {
                    return false;
                };
                let expected = match place {
                    "to the right of" => (editor.x + editor.width + EDITOR_INSET, editor.y),
                    "below" => (editor.x, editor.y + editor.height + EDITOR_INSET),
                    _ => (editor.x + 24.0, editor.y + 24.0),
                };
                windows.iter().any(|window| {
                    window.kind == WindowKind::Dialog
                        && (window.frame.x, window.frame.y) == expected
                        && !window.outside
                })
            },
            &format!("the dialog was not moved {place} the editor"),
        );
    }
}

/// What the editor's view received, as the editor variant reports it.
const CLICKS: u64 = 19;
const DRAGS: u64 = 20;
const WHEEL: u64 = 21;
const KEYS: u64 = 22;
const DOWN_X: u64 = 23;
const DOWN_Y: u64 = 24;
const CHARACTER: u64 = 25;
const PRESSED: u64 = 26;
const WARPED_DRAG: u64 = 27;
const ASKED_X: u64 = 28;
const ASKED_Y: u64 = 29;
const ASKED_BUTTONS: u64 = 30;
const ASKED_SHIFT: u64 = 31;
const ENTERED: u64 = 32;
const MOVED: u64 = 33;
const EXITED: u64 = 34;

fn pointer(editor: EditorId, kind: PointerKind, x: f64, y: f64, clicks: u32) -> Command {
    Command::Pointer(PointerInput::new(
        editor,
        kind,
        Button::Left,
        Point::new(x, y),
        clicks,
        Modifiers::empty(),
    ))
}

/// A click at `(x, y)` of the region, the `clicks`th of a run.
fn click(sender: &Sender, editor: EditorId, x: f64, y: f64, clicks: u32) {
    sender
        .send(pointer(editor, PointerKind::Down, x, y, clicks))
        .unwrap();
    sender
        .send(pointer(editor, PointerKind::Up, x, y, clicks))
        .unwrap();
}

fn key(sender: &Sender, editor: EditorId, key_code: u16, character: &str) {
    for down in [true, false] {
        sender
            .send(Command::Key(KeyInput::new(
                editor,
                down,
                key_code,
                character,
                character,
                false,
                Modifiers::empty(),
            )))
            .unwrap();
    }
}

/// A remote chain showing one editor in a region at `(100, 100)`, with its session.
fn shown_editor() -> (Chain, Session, EditorId) {
    let mut chain = spawn_remote(&[editor()]);
    let session = chain.remote_session().unwrap();
    chain
        .set_remote_region(region(100.0, 100.0, 300.0, 200.0))
        .unwrap();
    let opened = chain.open_remote_editor(0).unwrap();
    (chain, session, opened.editor)
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn pointer_wheel_and_key_input_reach_the_shown_editor() {
    let _serial = support::editor_test();
    let (mut chain, session, editor) = shown_editor();
    let sender = session.sender();
    // The editor's view starts 20 points in from the region's corner.
    click(&sender, editor, 50.0, 60.0, 1);
    click(&sender, editor, 50.0, 60.0, 2);
    wait(
        &mut chain,
        |chain| value(chain, CLICKS) == 3.0,
        "the double click did not reach the editor",
    );
    assert_eq!(
        (value(&mut chain, DOWN_X), value(&mut chain, DOWN_Y)),
        (30.0, 40.0)
    );
    sender
        .send(pointer(editor, PointerKind::Down, 60.0, 60.0, 1))
        .unwrap();
    sender
        .send(pointer(editor, PointerKind::Move, 80.0, 70.0, 0))
        .unwrap();
    sender
        .send(pointer(editor, PointerKind::Move, 100.0, 80.0, 0))
        .unwrap();
    wait(
        &mut chain,
        |chain| value(chain, DRAGS) >= 1.0 && value(chain, PRESSED) == 1.0,
        "the drag did not reach the editor",
    );
    sender
        .send(pointer(editor, PointerKind::Up, 100.0, 80.0, 1))
        .unwrap();
    wait(
        &mut chain,
        |chain| value(chain, PRESSED) == 0.0,
        "the button did not come up",
    );
    // Turning the wheel towards the user scrolls the content down: AppKit's negative delta.
    sender
        .send(Command::Wheel(WheelInput::new(
            editor,
            Point::new(100.0, 80.0),
            0.0,
            3.0,
            WheelUnit::Line,
            Modifiers::empty(),
        )))
        .unwrap();
    wait(
        &mut chain,
        |chain| value(chain, WHEEL) == -3.0,
        "the wheel did not reach the editor",
    );
    // Keys go to the window of the last click.
    key(&sender, editor, 0, "a");
    wait(
        &mut chain,
        |chain| value(chain, KEYS) == 1.0 && value(chain, CHARACTER) == f64::from(u32::from('a')),
        "the key did not reach the editor",
    );
    sender
        .send(Command::Text(TextInput::new(editor, "xyz")))
        .unwrap();
    wait(
        &mut chain,
        |chain| value(chain, KEYS) == 4.0 && value(chain, CHARACTER) == f64::from(u32::from('z')),
        "the text did not reach the editor",
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn input_reaches_a_dialog_the_editor_runs_from_a_key() {
    let _serial = support::editor_test();
    let (mut chain, mut session, editor) = shown_editor();
    let sender = session.sender();
    click(&sender, editor, 50.0, 60.0, 1);
    // The key opens a dialog that a click in it and then a key end.
    key(&sender, editor, 46, "m");
    let (mut nested, mut dialog) = (false, None);
    until(
        &mut session,
        |notice| {
            nested |= *notice == Notice::nested(1);
            if let Notice::Windows { windows, .. } = notice {
                dialog = windows
                    .iter()
                    .find(|window| window.kind == WindowKind::Dialog)
                    .map(|window| window.frame);
            }
            nested && dialog.is_some()
        },
        "not told the dialog's loop and window",
    );
    let frame = dialog.unwrap();
    click(
        &sender,
        editor,
        frame.x + frame.width / 2.0,
        frame.y + frame.height / 2.0,
        1,
    );
    key(&sender, editor, 36, "\r");
    until(
        &mut session,
        |notice| *notice == Notice::nested(0),
        "the click and the key did not end the dialog",
    );
    // The editor takes input again once the dialog ended.
    click(&sender, editor, 50.0, 60.0, 1);
    wait(
        &mut chain,
        |chain| value(chain, CLICKS) == 2.0,
        "the editor took no input after the dialog",
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn input_for_another_editor_is_dropped() {
    let _serial = support::editor_test();
    let (mut chain, session, editor) = shown_editor();
    let sender = session.sender();
    click(&sender, EditorId(editor.0 + 1), 50.0, 60.0, 5);
    click(&sender, editor, 50.0, 60.0, 1);
    wait(
        &mut chain,
        |chain| value(chain, CLICKS) >= 1.0,
        "the click did not reach the editor",
    );
    assert_eq!(value(&mut chain, CLICKS), 1.0);
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_session_ending_releases_the_held_button() {
    let _serial = support::editor_test();
    let (mut chain, session, editor) = shown_editor();
    let sender = session.sender();
    sender
        .send(pointer(editor, PointerKind::Down, 50.0, 60.0, 1))
        .unwrap();
    wait(
        &mut chain,
        |chain| value(chain, PRESSED) == 1.0,
        "the button did not go down",
    );
    drop(session);
    wait(
        &mut chain,
        |chain| value(chain, PRESSED) == 0.0,
        "the button stayed down after the session ended",
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_later_session_releases_the_button_the_one_before_held() {
    let _serial = support::editor_test();
    let (mut chain, first, editor) = shown_editor();
    first
        .sender()
        .send(pointer(editor, PointerKind::Down, 50.0, 60.0, 1))
        .unwrap();
    wait(
        &mut chain,
        |chain| value(chain, PRESSED) == 1.0,
        "the button did not go down",
    );
    let _second = chain.remote_session().unwrap();
    wait(
        &mut chain,
        |chain| value(chain, PRESSED) == 0.0,
        "the button stayed down after a later session started",
    );
    drop(first);
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_plugin_asking_for_the_mouse_is_told_the_sessions() {
    let _serial = support::editor_test();
    let (mut chain, session, editor) = shown_editor();
    let sender = session.sender();
    sender
        .send(Command::Pointer(PointerInput::new(
            editor,
            PointerKind::Down,
            Button::Left,
            Point::new(50.0, 60.0),
            1,
            Modifiers::SHIFT,
        )))
        .unwrap();
    wait(
        &mut chain,
        |chain| value(chain, PRESSED) == 1.0,
        "the button did not go down",
    );
    assert_eq!(
        (
            value(&mut chain, ASKED_X),
            value(&mut chain, ASKED_Y),
            value(&mut chain, ASKED_BUTTONS),
            value(&mut chain, ASKED_SHIFT),
        ),
        (30.0, 40.0, 1.0, 1.0)
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn relative_moves_go_on_from_where_the_plugin_warped_the_mouse() {
    let _serial = support::editor_test();
    let (mut chain, session, editor) = shown_editor();
    let sender = session.sender();
    // The view's centre: the fixture warps there after each drag with Command, as a JUCE
    // slider's velocity mode does.
    sender
        .send(Command::Pointer(PointerInput::new(
            editor,
            PointerKind::Down,
            Button::Left,
            Point::new(140.0, 80.0),
            1,
            Modifiers::COMMAND,
        )))
        .unwrap();
    wait(
        &mut chain,
        |chain| value(chain, PRESSED) == 1.0,
        "the button did not go down",
    );
    let real = cursor();
    // At once, so the later moves come before the plugin took the first.
    for _ in 0..5 {
        sender
            .send(Command::Motion(MotionInput::new(
                editor,
                0.0,
                10.0,
                Modifiers::COMMAND,
            )))
            .unwrap();
    }
    wait(
        &mut chain,
        |chain| value(chain, WARPED_DRAG) == 50.0,
        "the drags did not go on from the warped mouse",
    );
    // The warps moved the helper's mouse, not the machine's cursor.
    assert_eq!(cursor(), real);
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn moves_over_the_editor_reach_its_tracking_area() {
    let _serial = support::editor_test();
    // Without accessibility access for the helper's parent, the machine's cursor over the region
    // would hover the editor too.
    let (x, y, width, height) = global(100.0, 100.0, 300.0, 200.0);
    let (cursor_x, cursor_y) = cursor();
    assert!(
        cursor_x < x || cursor_x >= x + width || cursor_y < y || cursor_y >= y + height,
        "move the machine's cursor off the region first"
    );
    let (mut chain, session, editor) = shown_editor();
    let sender = session.sender();
    sender
        .send(pointer(editor, PointerKind::Move, 50.0, 60.0, 0))
        .unwrap();
    wait(
        &mut chain,
        |chain| value(chain, ENTERED) == 1.0 && value(chain, MOVED) >= 1.0,
        "the move did not enter the view",
    );
    // Relative moves without a button hover too: out past the view's right edge, 240 points
    // from where it starts 20 points into the region.
    sender
        .send(Command::Motion(MotionInput::new(
            editor,
            220.0,
            0.0,
            Modifiers::empty(),
        )))
        .unwrap();
    wait(
        &mut chain,
        |chain| value(chain, EXITED) == 1.0,
        "the move did not leave the view",
    );
    assert_eq!(value(&mut chain, ENTERED), 1.0);
}

fn kinds(windows: &[Window]) -> Vec<WindowKind> {
    windows.iter().map(|window| window.kind).collect()
}

fn order_until(session: &mut Session, expected: &[WindowKind], what: &str) {
    until(
        session,
        |notice| matches!(notice, Notice::Windows { windows, .. } if kinds(windows) == expected),
        what,
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_window_the_plugin_opens_behind_comes_forward_and_a_click_raises_the_window_under_it() {
    let _serial = support::editor_test();
    let (mut chain, mut session, editor) = shown_editor();
    let sender = session.sender();
    // The plugin opens a window behind the editor, over its corner: nothing on the machine
    // raises it, so the helper brings it to the front.
    chain.set_parameter_text(0, 17, "27").unwrap();
    order_until(
        &mut session,
        &[WindowKind::Dialog, WindowKind::Editor],
        "the window opened behind the editor did not come to the front",
    );
    // A click where only the editor shows raises the editor, as a click on a window of an
    // active application does, and one where only the window shows raises it again.
    click(&sender, editor, 250.0, 60.0, 1);
    order_until(
        &mut session,
        &[WindowKind::Editor, WindowKind::Dialog],
        "the click did not raise the editor",
    );
    click(&sender, editor, 100.0, 150.0, 1);
    order_until(
        &mut session,
        &[WindowKind::Dialog, WindowKind::Editor],
        "the click did not raise the window",
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_plugin_showing_activating_or_running_a_dialog_leaves_the_helper_behind() {
    let _serial = support::editor_test();
    let (mut chain, mut session, editor) = shown_editor();
    let pid = chain.helper_monitor().process_id() as i32;
    let behind = |what: &str| {
        // Activation takes effect a little later.
        std::thread::sleep(Duration::from_millis(300));
        assert_ne!(
            frontmost(),
            Some(pid),
            "{what} brought the helper to the front"
        );
    };
    // Hidden, then shown again the way that activates.
    act(&mut chain, 1);
    act(&mut chain, 8);
    behind("unhide:");
    act(&mut chain, 9);
    behind("activateFromApplication:options:");
    let sender = session.sender();
    click(&sender, editor, 50.0, 60.0, 1);
    key(&sender, editor, 46, "m");
    until(
        &mut session,
        |notice| *notice == Notice::nested(1),
        "the dialog did not run",
    );
    behind("runModalForWindow:");
}

/// Reads notices until a cursor notice, which it returns.
fn next_cursor(session: &mut Session, what: &str) -> (CursorShape, bool) {
    match until(
        session,
        |notice| matches!(notice, Notice::Cursor { .. }),
        what,
    ) {
        Notice::Cursor { shape, hidden, .. } => (shape, hidden),
        _ => unreachable!(),
    }
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn the_session_is_told_the_cursor_the_plugin_sets_and_hides() {
    let _serial = support::editor_test();
    let (mut chain, mut session, editor) = shown_editor();
    let sender = session.sender();
    assert_eq!(
        next_cursor(&mut session, "not told the cursor"),
        (CursorShape::Arrow, false)
    );
    // The view sets its cursor when the pointer enters it, as FabFilter's and VSTGUI's do.
    sender
        .send(pointer(editor, PointerKind::Move, 50.0, 60.0, 0))
        .unwrap();
    assert_eq!(
        next_cursor(&mut session, "not told the entered view's cursor"),
        (CursorShape::PointingHand, false)
    );
    act(&mut chain, 10);
    assert_eq!(
        next_cursor(&mut session, "not told the I-beam"),
        (CursorShape::IBeam, false)
    );
    for (hide, show, what) in [(13, 14, "NSCursor"), (15, 16, "Core Graphics")] {
        act(&mut chain, hide);
        assert_eq!(
            next_cursor(&mut session, what),
            (CursorShape::IBeam, true),
            "{what}"
        );
        act(&mut chain, show);
        assert_eq!(
            next_cursor(&mut session, what),
            (CursorShape::IBeam, false),
            "{what}"
        );
    }
    act(&mut chain, 17);
    assert_eq!(
        next_cursor(&mut session, "not told the cursor hid until moved"),
        (CursorShape::IBeam, true)
    );
    sender
        .send(pointer(editor, PointerKind::Move, 60.0, 60.0, 0))
        .unwrap();
    assert_eq!(
        next_cursor(&mut session, "not told the cursor showed on a move"),
        (CursorShape::IBeam, false)
    );
    act(&mut chain, 11);
    let image = match next_cursor(&mut session, "not told the image cursor") {
        (CursorShape::Image(image), false) => image,
        other => panic!("expected a visible image cursor, got {other:?}"),
    };
    assert_eq!(
        (image.width, image.height, image.hot_spot),
        (16.0, 16.0, Point::new(3.0, 4.0))
    );
    // A PNG of 16 by 16 pixels.
    assert_eq!(&image.png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&image.png[16..24], &[0, 0, 0, 16, 0, 0, 0, 16]);
    // A cursor with nothing to see, as JUCE hides the cursor.
    act(&mut chain, 12);
    assert!(next_cursor(&mut session, "not told the blank cursor").1);
    // A later session is told the cursor at once.
    let mut later = chain.remote_session().unwrap();
    assert!(next_cursor(&mut later, "the later session was not told the cursor").1);
}

const MENU_CHOICE: u64 = 35;
const MENUS_CLOSED: u64 = 36;

/// Pops up the fixture's menu and returns it with its items once the session is told. Only
/// the edit that asks for it: a later request would wait for the menu's loop.
fn pop_up_menu(chain: &mut Chain, session: &mut Session) -> (MenuId, Vec<MenuItem>) {
    chain.set_parameter_text(0, 17, "18").unwrap();
    match until(
        session,
        |notice| matches!(notice, Notice::MenuOpened { .. }),
        "not told the menu opened",
    ) {
        Notice::MenuOpened { menu, items, .. } => (menu, items),
        _ => unreachable!(),
    }
}

fn menu_closed(session: &mut Session, menu: MenuId, what: &str) {
    until(session, |notice| *notice == Notice::menu_closed(menu), what);
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_menu_the_plugin_opens_is_told_and_its_item_chosen_by_index() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    let (menu, items) = pop_up_menu(&mut chain, &mut session);
    // The hidden item at index 2 is left out; the others keep their indexes.
    let summary: Vec<_> = items
        .iter()
        .map(|item| {
            (
                item.index,
                item.title.as_str(),
                item.separator,
                item.submenu.len(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (0, "First", false, 0),
            (1, "", true, 0),
            (3, "More", false, 2),
            (4, "Save", false, 0)
        ]
    );
    let more = &items[2].submenu;
    assert_eq!(
        (more[0].title.as_str(), more[0].enabled, more[0].state),
        ("Deep", true, MenuItemState::On)
    );
    assert_eq!(
        (more[1].title.as_str(), more[1].enabled, more[1].state),
        ("Off", false, MenuItemState::Off)
    );
    let sender = session.sender();
    // A disabled item is not chosen, and the menu stays open.
    sender
        .send(Command::ChooseMenuItem(MenuChoice::new(menu, vec![3, 1])))
        .unwrap();
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let notice = session.recv(Duration::from_millis(50)).unwrap();
        assert_ne!(
            notice,
            Some(Notice::menu_closed(menu)),
            "the disabled item closed the menu"
        );
    }
    sender
        .send(Command::ChooseMenuItem(MenuChoice::new(menu, vec![3, 0])))
        .unwrap();
    menu_closed(&mut session, menu, "the choice did not close the menu");
    assert_eq!(
        (
            value(&mut chain, MENU_CHOICE),
            value(&mut chain, MENUS_CLOSED)
        ),
        (3.0, 1.0)
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_chosen_item_runs_once_its_menu_closed() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    let (menu, _) = pop_up_menu(&mut chain, &mut session);
    let sender = session.sender();
    sender
        .send(Command::ChooseMenuItem(MenuChoice::new(menu, vec![4])))
        .unwrap();
    // The item runs a save panel: the menu closes before the panel is asked for, as AppKit runs
    // a chosen item once its menu is down, rather than staying up while the panel waits.
    let mut closed = false;
    let panel = loop {
        match until(
            &mut session,
            |notice| {
                matches!(
                    notice,
                    Notice::MenuClosed { .. } | Notice::PanelRequested { .. }
                )
            },
            "neither the menu closed nor the panel was asked for",
        ) {
            Notice::MenuClosed { menu: closing, .. } if closing == menu => closed = true,
            Notice::PanelRequested { panel, .. } => {
                assert!(closed, "the panel was asked for while the menu was up");
                break panel;
            }
            _ => {}
        }
    };
    answer(&sender, panel, None);
    panel_finished(&mut session, panel, "the cancelled panel did not finish");
    assert_eq!(
        (
            value(&mut chain, PANEL_RESULT),
            value(&mut chain, MENUS_CLOSED)
        ),
        (2.0, 1.0)
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_later_session_is_told_the_open_menu_and_can_close_it() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    let (menu, items) = pop_up_menu(&mut chain, &mut session);
    let mut later = chain.remote_session().unwrap();
    until(
        &mut later,
        |notice| *notice == Notice::menu_opened(menu, items.clone()),
        "the later session was not told the open menu",
    );
    // A session replaced by a later one leaves its menu to the later one.
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let notice = later.recv(Duration::from_millis(50)).unwrap();
        assert_ne!(
            notice,
            Some(Notice::menu_closed(menu)),
            "the replacement closed the menu"
        );
    }
    later.sender().send(Command::CloseMenu(menu)).unwrap();
    menu_closed(&mut later, menu, "the menu did not close");
    assert_eq!(
        (
            value(&mut chain, MENU_CHOICE),
            value(&mut chain, MENUS_CLOSED)
        ),
        (0.0, 1.0)
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_session_ending_closes_its_menus() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    pop_up_menu(&mut chain, &mut session);
    let ended = Instant::now();
    drop(session);
    // The request waits for the menu's loop, which the session's end ends, well before the
    // menu timeout would.
    assert_eq!(value(&mut chain, MENUS_CLOSED), 1.0);
    assert!(ended.elapsed() < Duration::from_secs(10));
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_session_starts_while_a_dialog_the_plugin_runs_is_open() {
    let _serial = support::editor_test();
    let (mut chain, mut session, editor) = shown_editor();
    let sender = session.sender();
    click(&sender, editor, 50.0, 60.0, 1);
    // The key opens a dialog that a click in it and then a key end; the session that opened
    // it goes, as a browser that reloads does.
    key(&sender, editor, 46, "m");
    until(
        &mut session,
        |notice| *notice == Notice::nested(1),
        "the dialog did not run",
    );
    drop(sender);
    drop(session);
    let started = Instant::now();
    let mut later = chain.remote_session().unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    let mut dialog = None;
    until(
        &mut later,
        |notice| {
            if let Notice::Windows { windows, .. } = notice {
                dialog = windows
                    .iter()
                    .find(|window| window.kind == WindowKind::Dialog)
                    .map(|window| window.frame);
            }
            dialog.is_some()
        },
        "the later session was not told the dialog",
    );
    let frame = dialog.unwrap();
    let sender = later.sender();
    click(
        &sender,
        editor,
        frame.x + frame.width / 2.0,
        frame.y + frame.height / 2.0,
        1,
    );
    key(&sender, editor, 36, "\r");
    until(
        &mut later,
        |notice| *notice == Notice::nested(0),
        "the later session could not end the dialog",
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_menu_open_past_the_menu_timeout_closes() {
    let _serial = support::editor_test();
    let mut timeouts = Timeouts::default();
    timeouts.menu = Duration::from_secs(1);
    let mut chain = spawn_remote_with(&[editor()], timeouts);
    let mut session = chain.remote_session().unwrap();
    chain
        .set_remote_region(region(100.0, 100.0, 300.0, 200.0))
        .unwrap();
    chain.open_remote_editor(0).unwrap();
    let started = Instant::now();
    let (menu, _) = pop_up_menu(&mut chain, &mut session);
    menu_closed(&mut session, menu, "the menu did not time out");
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(value(&mut chain, MENU_CHOICE), 0.0);
}

const PANEL_RESULT: u64 = 37;
const PANEL_FILES: u64 = 38;

/// Has the editor ask for a panel with `action`, only the edit that asks: a modal panel holds
/// later requests. Returns the panel, its kind and whether it takes several files.
fn ask_panel(chain: &mut Chain, session: &mut Session, action: &str) -> (PanelId, PanelKind, bool) {
    chain.set_parameter_text(0, 17, action).unwrap();
    match until(
        session,
        |notice| matches!(notice, Notice::PanelRequested { .. }),
        "not told the panel's request",
    ) {
        Notice::PanelRequested { panel, request, .. } => {
            assert_eq!(
                request.directory.as_deref(),
                Some(std::path::Path::new("/tmp/plughost-test-panels"))
            );
            assert_eq!(request.types, ["fxp"]);
            (panel, request.kind, request.multiple)
        }
        _ => unreachable!(),
    }
}

fn answer(sender: &Sender, panel: PanelId, paths: Option<Vec<std::path::PathBuf>>) {
    sender
        .send(Command::AnswerPanel(PanelAnswer::new(panel, paths)))
        .unwrap();
}

fn panel_finished(session: &mut Session, panel: PanelId, what: &str) {
    until(
        session,
        |notice| *notice == Notice::panel_finished(panel),
        what,
    );
}

/// No `PanelFinished` for `panel` comes within half a second.
fn still_waiting(session: &mut Session, panel: PanelId, what: &str) {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let notice = session.recv(Duration::from_millis(50)).unwrap();
        assert_ne!(notice, Some(Notice::panel_finished(panel)), "{what}");
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("plughost-panel-{}-{name}", std::process::id()))
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_save_panel_the_plugin_runs_is_answered_by_the_session() {
    let _serial = support::editor_test();
    let (mut chain, mut session, editor) = shown_editor();
    let sender = session.sender();
    chain.set_parameter_text(0, 17, "19").unwrap();
    let request = until(
        &mut session,
        |notice| matches!(notice, Notice::PanelRequested { .. }),
        "not told the panel's request",
    );
    let Notice::PanelRequested { panel, request, .. } = request else {
        unreachable!()
    };
    assert_eq!(
        (
            request.kind,
            request.name.as_str(),
            request.types,
            request.multiple,
            request.directories,
            request.message.as_str()
        ),
        (
            PanelKind::Save,
            "Untitled",
            vec!["fxp".to_owned()],
            false,
            false,
            "Save the preset"
        )
    );
    assert_eq!(
        request.directory.as_deref(),
        Some(std::path::Path::new("/tmp/plughost-test-panels"))
    );
    // The plugin waits in the panel's modal loop, which drops input to its editor.
    click(&sender, editor, 50.0, 60.0, 1);
    let path = scratch("saved.fxp");
    answer(&sender, panel, Some(vec![path.clone()]));
    panel_finished(
        &mut session,
        panel,
        "the panel did not return to the plugin",
    );
    assert_eq!(
        (
            value(&mut chain, PANEL_RESULT),
            value(&mut chain, PANEL_FILES)
        ),
        (1.0, 1.0)
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"saved");
    let _ = std::fs::remove_file(&path);
    assert_eq!(value(&mut chain, CLICKS), 0.0);
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn an_open_panel_with_a_handler_takes_several_files() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    let sender = session.sender();
    let (panel, kind, multiple) = ask_panel(&mut chain, &mut session, "20");
    assert_eq!((kind, multiple), (PanelKind::Open, true));
    // An open takes one file at least.
    answer(&sender, panel, Some(Vec::new()));
    still_waiting(&mut session, panel, "an empty answer opened nothing");
    answer(
        &sender,
        panel,
        Some(vec![scratch("a.fxp"), scratch("b.fxp")]),
    );
    panel_finished(&mut session, panel, "the handler was not called");
    assert_eq!(
        (
            value(&mut chain, PANEL_RESULT),
            value(&mut chain, PANEL_FILES)
        ),
        (3.0, 2.0)
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_save_panel_sheet_is_cancelled_by_the_session() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    let sender = session.sender();
    let (panel, kind, _) = ask_panel(&mut chain, &mut session, "21");
    assert_eq!(kind, PanelKind::Save);
    // A save takes one path.
    answer(
        &sender,
        panel,
        Some(vec![scratch("a.fxp"), scratch("b.fxp")]),
    );
    still_waiting(&mut session, panel, "a save took two paths");
    answer(&sender, panel, None);
    panel_finished(&mut session, panel, "the sheet's handler was not called");
    assert_eq!(value(&mut chain, PANEL_RESULT), 6.0);
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_session_ending_cancels_its_waiting_panel() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    ask_panel(&mut chain, &mut session, "19");
    let ended = Instant::now();
    drop(session);
    // The request waits for the panel's modal loop, which the cancel ends.
    assert_eq!(value(&mut chain, PANEL_RESULT), 2.0);
    assert!(ended.elapsed() < Duration::from_secs(10));
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_later_session_is_told_the_waiting_panel_and_answers_it() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    let (panel, _, _) = ask_panel(&mut chain, &mut session, "20");
    // A later session replaces this one, without this one ending first.
    let mut later = chain.remote_session().unwrap();
    drop(session);
    until(
        &mut later,
        |notice| matches!(notice, Notice::PanelRequested { panel: asked, .. } if *asked == panel),
        "the later session was not told the waiting panel",
    );
    answer(&later.sender(), panel, Some(vec![scratch("a.fxp")]));
    panel_finished(
        &mut later,
        panel,
        "the later session's answer did not reach the plugin",
    );
    assert_eq!(
        (
            value(&mut chain, PANEL_RESULT),
            value(&mut chain, PANEL_FILES)
        ),
        (3.0, 1.0)
    );
}

const COPIED: u64 = 39;

fn clipboard() -> Vec<u8> {
    std::process::Command::new("pbpaste")
        .output()
        .unwrap()
        .stdout
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn what_a_plugin_copies_stays_off_the_machines_clipboard() {
    let _serial = support::editor_test();
    let (mut chain, _session, _) = shown_editor();
    let before = clipboard();
    act(&mut chain, 22);
    // The editor copies after the plugin call, when the helper's run loop next runs.
    wait(
        &mut chain,
        |chain| value(chain, COPIED) == 1.0,
        "what the plugin copied did not read back",
    );
    assert_eq!(clipboard(), before);
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn urls_and_files_a_plugin_opens_or_shows_are_told_instead() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    act(&mut chain, 23);
    until(
        &mut session,
        |notice| *notice == Notice::open_url("plughost-test://open/page"),
        "not told the page the plugin opened",
    );
    act(&mut chain, 24);
    until(
        &mut session,
        |notice| {
            *notice
                == Notice::reveal_files(vec![std::path::PathBuf::from(
                    "/tmp/plughost-test-panels/shown.fxp",
                )])
        },
        "not told the file the plugin showed",
    );
    act(&mut chain, 25);
    until(
        &mut session,
        |notice| *notice == Notice::open_url("plughost-test://launch-services"),
        "not told the page the plugin opened through Launch Services",
    );
}

/// The editor focus the fixture in `slot` reports: 3 once its view took a key the host sent.
fn focus(chain: &mut Chain, slot: usize) -> f64 {
    const EDITOR_FOCUS: u64 = 15;
    let (_, value) = chain
        .parameters(slot)
        .unwrap()
        .into_iter()
        .find(|(info, _)| info.id == EDITOR_FOCUS)
        .unwrap();
    chain
        .parameter_to_plain(slot, EDITOR_FOCUS, value.unwrap())
        .unwrap()
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn keys_go_to_the_shown_editor_once_the_clicked_one_is_hidden() {
    const TOOK_KEY: f64 = 3.0;
    let _serial = support::editor_test();
    let mut chain = spawn_remote(&[editor(), editor()]);
    let session = chain.remote_session().unwrap();
    let sender = session.sender();
    chain
        .set_remote_region(region(100.0, 100.0, 300.0, 200.0))
        .unwrap();
    let first = chain.open_remote_editor(0).unwrap().editor;
    click(&sender, first, 50.0, 60.0, 1);
    wait(
        &mut chain,
        |chain| value(chain, CLICKS) == 1.0,
        "the click did not reach the first editor",
    );
    let second = chain.open_remote_editor(1).unwrap().editor;
    key(&sender, second, 0, "a");
    wait(
        &mut chain,
        |chain| focus(chain, 1) == TOOK_KEY,
        "the key did not reach the shown editor",
    );
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_panel_the_plugin_closes_is_given_up() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    let (panel, kind, _) = ask_panel(&mut chain, &mut session, "26");
    assert_eq!(kind, PanelKind::Open);
    panel_finished(&mut session, panel, "the closed panel did not finish");
    // A later session is not told it, and the session's end does not call its handler.
    let mut later = chain.remote_session().unwrap();
    drop(session);
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let notice = later.recv(Duration::from_millis(50)).unwrap();
        assert!(
            !matches!(notice, Some(Notice::PanelRequested { .. })),
            "the later session was told the closed panel"
        );
    }
    drop(later);
    assert_eq!(value(&mut chain, PANEL_RESULT), 0.0);
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_waiting_panel_is_given_up_when_its_editor_closes_or_its_instance_is_replaced() {
    let _serial = support::editor_test();
    let (mut chain, mut session, _) = shown_editor();
    let (panel, _, _) = ask_panel(&mut chain, &mut session, "20");
    chain.close_editor(0).unwrap();
    panel_finished(
        &mut session,
        panel,
        "the closed editor's panel was not given up",
    );
    chain.open_remote_editor(0).unwrap();
    let (panel, _, _) = ask_panel(&mut chain, &mut session, "20");
    let state = chain
        .save_state(0, plughost::StatePurpose::Project)
        .unwrap();
    chain
        .restore_state(0, &state, plughost::StatePurpose::Project)
        .unwrap();
    panel_finished(
        &mut session,
        panel,
        "the replaced instance's panel was not given up",
    );
    // The session's end calls no handler of them.
    drop(session);
    assert_eq!(value(&mut chain, PANEL_RESULT), 0.0);
}

/// Apple's AUDelay, a version 2 unit, which runs the run loop for a while in a save after an edit
/// so its listeners take the edit.
fn audelay() -> PluginRef {
    PluginRef::audio_unit("6175667864656C796170706C")
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_wait_of_the_host_in_a_request_is_no_nested_loop_and_runs_no_panel_handler() {
    // AUDelay's save runs the run loop for 100 ms after an edit, for its listeners.
    const SAVE_WAIT: Duration = Duration::from_millis(100);
    let _serial = support::editor_test();
    let mut chain = spawn_remote(&[editor(), audelay()]);
    let mut session = chain.remote_session().unwrap();
    chain
        .set_remote_region(region(100.0, 100.0, 300.0, 200.0))
        .unwrap();
    chain.open_remote_editor(0).unwrap();
    let sender = session.sender();
    let (panel, _, _) = ask_panel(&mut chain, &mut session, "20");
    let notices = std::thread::spawn(move || {
        let mut notices = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(notice) = session.recv(Duration::from_millis(20)).unwrap() {
                notices.push((notice, Instant::now()));
            }
        }
        notices
    });
    chain.set_parameter(1, 0, 0.3).unwrap();
    let started = Instant::now();
    // The answer comes while the save waits.
    let answering = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        answer(&sender, panel, Some(vec![scratch("a.fxp")]));
    });
    chain
        .save_state(1, plughost::StatePurpose::Project)
        .unwrap();
    answering.join().unwrap();
    let notices = notices.join().unwrap();
    assert!(
        !notices
            .iter()
            .any(|(notice, _)| matches!(notice, Notice::Nested { depth, .. } if *depth > 0)),
        "the save's wait was told as a nested loop: {notices:?}"
    );
    let finished = notices
        .iter()
        .find(|(notice, _)| *notice == Notice::panel_finished(panel))
        .map(|(_, at)| *at)
        .expect("the panel was not answered");
    assert!(
        finished.duration_since(started) >= SAVE_WAIT,
        "the panel's handler ran {:?} into the save",
        finished.duration_since(started)
    );
    assert_eq!(value(&mut chain, PANEL_RESULT), 3.0);
}

#[test]
#[ignore = "needs helper and delay fixtures, a desktop session and accessibility access"]
fn the_machines_mouse_over_the_region_does_not_reach_the_editor() {
    let _serial = support::editor_test();
    let (x, y, width, height) = global(100.0, 100.0, 300.0, 200.0);
    let (back_x, back_y) = cursor();
    let (mut chain, _session, _) = shown_editor();
    // The window server lists the editor's window once it shows.
    let pid = chain.helper_monitor().process_id();
    wait(
        &mut chain,
        |_| !on_screen(pid).is_empty(),
        "the editor's window did not show",
    );
    // Into the editor's view, 20 points into the region, across it and out again.
    move_machine_cursor(x - 50.0, y - 50.0);
    for (dx, dy) in [
        (80.0, 60.0),
        (120.0, 70.0),
        (200.0, 80.0),
        (width + 50.0, height + 50.0),
    ] {
        move_machine_cursor(x + dx, y + dy);
        std::thread::sleep(Duration::from_millis(50));
    }
    move_machine_cursor(back_x, back_y);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        (
            value(&mut chain, ENTERED),
            value(&mut chain, MOVED),
            value(&mut chain, EXITED)
        ),
        (0.0, 0.0, 0.0)
    );
}
