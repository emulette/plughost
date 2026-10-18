//! Editor windows of a real helper on the desktop: keyboard focus, window sizes the system
//! corrects, and editors that show, hide and close themselves. Build the helper and the test plugins first (see `support`).
#![cfg(target_os = "windows")]

use std::time::{Duration, Instant};

use plughost::{BlockContext, Chain, Event, Layout, PluginFormat};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOT, GUITHREADINFO, GetAncestor, GetClassNameW, GetClientRect,
    GetGUIThreadInfo, GetWindowThreadProcessId, IsWindowVisible,
};

mod support;

use support::{delay_variant, fixture, prepare, spawn};

fn class_name(hwnd: windows_sys::Win32::Foundation::HWND) -> String {
    let mut name = [0u16; 128];
    let length = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
    String::from_utf16_lossy(&name[..length.max(0) as usize])
}

/// Whether the synth's floating editor window exists, and if so whether it is visible.
fn editor_window(pid: u32) -> Option<bool> {
    window(pid, "PlughostTestSynthEditor").map(|hwnd| unsafe { IsWindowVisible(hwnd) } != 0)
}

fn window(pid: u32, class: &str) -> Option<HWND> {
    struct Search<'a> {
        pid: u32,
        class: &'a str,
        found: HWND,
    }
    unsafe extern "system" fn visit(hwnd: HWND, data: isize) -> i32 {
        // SAFETY: EnumWindows calls synchronously with this function's live Search.
        let search = unsafe { &mut *(data as *mut Search<'_>) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == search.pid && class_name(hwnd) == search.class {
            search.found = hwnd;
            0
        } else {
            1
        }
    }
    let mut search = Search {
        pid,
        class,
        found: std::ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(visit), std::ptr::from_mut(&mut search) as isize) };
    (!search.found.is_null()).then_some(search.found)
}

/// Waits until the chain reports `open` and the window is in `window`.
fn expect(chain: &mut Chain, open: bool, window: Option<bool>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = (
            chain.editor_open(0).unwrap(),
            editor_window(chain.helper_monitor().process_id()),
        );
        if state == (open, window) {
            return;
        }
        assert!(Instant::now() < deadline, "{state:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A note on the synth's editor keys: 0 hides, 1 shows, 2 closes.
fn request(chain: &mut Chain, key: u8) {
    let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
    chain
        .process_audio_f32(
            &BlockContext::new(64),
            &[],
            &mut [&mut left, &mut right],
            &[],
            &[Event::note_on(0, 0, key, 127)],
            &mut Vec::new(),
        )
        .unwrap();
}

#[test]
#[ignore = "needs helper and synth fixtures and a desktop session"]
fn a_floating_editor_follows_its_own_requests_and_reopens() {
    let _serial = support::editor_test();
    let mut chain = spawn(&[fixture(
        PluginFormat::Clap,
        "plughost-test-synth",
        "com.studio.plughost.test-synth",
    )]);
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::None, &[Layout::Stereo])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    assert_eq!(editor_window(chain.helper_monitor().process_id()), None);
    chain.open_editor(0).unwrap();
    expect(&mut chain, true, Some(true));
    request(&mut chain, 0);
    expect(&mut chain, true, Some(false));
    request(&mut chain, 1);
    expect(&mut chain, true, Some(true));
    // The editor reports its window closed; the host detaches and destroys it.
    request(&mut chain, 2);
    expect(&mut chain, false, None);
    chain.open_editor(0).unwrap();
    expect(&mut chain, true, Some(true));
    chain.close_editor(0).unwrap();
    expect(&mut chain, false, None);
}

#[test]
#[ignore = "needs helper and SDK sample fixtures and a desktop session"]
fn an_activated_editor_window_gives_the_editor_keyboard_focus() {
    let _serial = support::editor_test();
    let mut chain = spawn(&[fixture(
        PluginFormat::Vst3,
        "again",
        "84E8DE5F92554F5396FAE4133C935A18",
    )]);
    prepare(&mut chain);
    chain.open_editor(0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid = chain.helper_monitor().process_id();
    let (window, focus) = loop {
        chain.editor_open(0).unwrap();
        if let Some(window) = window(pid, "PlughostEditorWindow") {
            let thread = unsafe { GetWindowThreadProcessId(window, std::ptr::null_mut()) };
            let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
            info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
            let found = unsafe { GetGUIThreadInfo(thread, &mut info) } != 0;
            if found && info.hwndActive == window && !info.hwndFocus.is_null() {
                break (window, info.hwndFocus);
            }
        }
        assert!(
            Instant::now() < deadline,
            "the editor window never became active"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    // Keys reach the plugin's view inside the window, not the host's frame around it.
    assert_ne!(focus, window);
    assert_eq!(unsafe { GetAncestor(focus, GA_ROOT) }, window);
    chain.close_editor(0).unwrap();
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_window_the_system_widens_past_the_editor_is_not_offered_to_it() {
    let _serial = support::editor_test();
    // The editor variant is 40 pixels wide, narrower than the system lets a window be. It claims
    // to be resizable, accepts only its own size and counts the sizes it refused.
    const WIDTH: i32 = 40;
    const REFUSED_SIZES: u64 = 16;
    let mut chain = spawn(&[delay_variant(PluginFormat::Clap, "editor", 0xB)]);
    chain.open_editor(0).unwrap();
    let pid = chain.helper_monitor().process_id();
    let deadline = Instant::now() + Duration::from_secs(5);
    let width = loop {
        if let Some(window) = window(pid, "PlughostEditorWindow") {
            let mut rect = RECT::default();
            if unsafe { GetClientRect(window, &mut rect) } != 0 && rect.right > 0 {
                break rect.right - rect.left;
            }
        }
        assert!(Instant::now() < deadline, "the editor window never opened");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(width > WIDTH, "the window is {width} pixels wide");
    // The helper checks the window's size each time it serves its editors.
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        assert!(chain.editor_open(0).unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
    let (_, value) = chain
        .parameters(0)
        .unwrap()
        .into_iter()
        .find(|(info, _)| info.id == REFUSED_SIZES)
        .unwrap();
    assert_eq!(
        chain.parameter_to_plain(0, REFUSED_SIZES, value).unwrap(),
        0.0
    );
    chain.close_editor(0).unwrap();
}

#[test]
#[ignore = "needs helper and SDK sample fixtures and a desktop session"]
fn edits_and_saves_with_the_editor_open_survive_until_a_crash_and_recovery_reopens_it() {
    let _serial = support::editor_test();
    const GAIN: u64 = 0;
    let mut chain = spawn(&[fixture(
        PluginFormat::Vst3,
        "again",
        "84E8DE5F92554F5396FAE4133C935A18",
    )]);
    prepare(&mut chain);
    chain.open_editor(0).unwrap();
    let input = [0.5f32; 512];
    let mut snapshot = None;
    for block in 0..40 {
        // Edits, processing and saves interleave while the editor shows the plugin.
        chain
            .set_parameter(0, GAIN, f64::from(block % 8) / 8.0)
            .unwrap();
        let (mut left, mut right) = ([0.0f32; 512], [0.0f32; 512]);
        chain
            .process_audio_f32(
                &BlockContext::new(512),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        if block % 10 == 9 {
            snapshot = Some(
                chain
                    .save_state(0, plughost::StatePurpose::Project)
                    .unwrap(),
            );
        }
        assert!(chain.editor_open(0).unwrap());
    }
    let snapshot = snapshot.unwrap();
    // The helper dies with the editor open, the way a crashing plugin takes it down.
    let pid = chain.helper_monitor().process_id();
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        assert!(!process.is_null());
        assert_ne!(TerminateProcess(process, 17), 0);
        CloseHandle(process);
    }
    assert!(
        chain
            .helper_monitor()
            .wait_for_exit(Duration::from_secs(5))
            .is_some()
    );
    assert!(matches!(
        chain.editor_open(0),
        Err(plughost::Error::Crashed { .. })
    ));
    let mut recovered = chain.recover(std::slice::from_ref(&snapshot)).unwrap();
    // Editors start closed after recovery; the application reopens them.
    assert!(!recovered.editor_open(0).unwrap());
    assert_eq!(
        recovered
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap(),
        snapshot
    );
    recovered.open_editor(0).unwrap();
    assert!(recovered.editor_open(0).unwrap());
    recovered.close_editor(0).unwrap();
}
