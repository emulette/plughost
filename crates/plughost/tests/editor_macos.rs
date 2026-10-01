//! Editor windows of a real helper on the macOS desktop: keyboard focus, editors that show, hide
//! and close themselves, and Audio Unit views. Build the helper and the test plugins first (see
//! `support`).
#![cfg(target_os = "macos")]

use std::ptr::NonNull;
use std::time::{Duration, Instant};

use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use plughost::{BlockContext, Chain, Event, Layout, PluginFormat, PluginRef};

mod support;

use support::{delay_variant, fixture, prepare, spawn};

/// 'aufx' 'dely' 'appl'
const AU_DELAY: &str = "6175667864656C796170706C";
/// The synth fixture's editor window is the only helper window this wide.
const SYNTH_EDITOR_WIDTH: f64 = 200.0;

fn audio_unit(class_id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::AudioUnit,
        bundle: None,
        class_id: class_id.to_owned(),
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const CFArray;
}

type Info = CFDictionary<CFString, CFType>;

/// A window of the helper: its size and whether it is on screen.
#[derive(Debug)]
struct Window {
    width: f64,
    height: f64,
    on_screen: bool,
}

fn number(info: &Info, key: &'static str) -> Option<f64> {
    info.get(&CFString::from_static_str(key))?
        .downcast::<CFNumber>()
        .ok()?
        .as_f64()
}

/// The windows the window server lists for process `pid`, including hidden ones.
fn windows(pid: u32) -> Vec<Window> {
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

/// Whether the synth's floating editor window exists, and if so whether it is on screen.
fn synth_editor(pid: u32) -> Option<bool> {
    windows(pid)
        .into_iter()
        .find(|window| window.width == SYNTH_EDITOR_WIDTH)
        .map(|window| window.on_screen)
}

/// Waits until `check` holds, with the chain's editor state polled so the helper serves requests.
fn wait(chain: &mut Chain, mut check: impl FnMut(&mut Chain) -> bool, what: &str) {
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

/// Waits until the chain reports `open` and the synth's window is in `window`.
fn expect(chain: &mut Chain, open: bool, window: Option<bool>) {
    let pid = chain.helper_monitor().process_id();
    wait(
        chain,
        |chain| (chain.editor_open(0).unwrap(), synth_editor(pid)) == (open, window),
        &format!("expected editor open {open} and window {window:?}"),
    );
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
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn an_opened_editor_window_gives_the_editor_keyboard_focus() {
    // The editor variant reports 1 once its view is its window's first responder, and 2 once a
    // key it sends to its window reaches it.
    const FOCUS: u64 = 15;
    let mut chain = spawn(&[delay_variant(PluginFormat::Clap, "editor", 0xB)]);
    chain.open_editor(0).unwrap();
    wait(
        &mut chain,
        |chain| {
            let (_, value) = chain
                .parameters(0)
                .unwrap()
                .into_iter()
                .find(|(info, _)| info.id == FOCUS)
                .unwrap();
            chain.parameter_to_plain(0, FOCUS, value).unwrap() == 2.0
        },
        "keys never reached the editor's view",
    );
    chain.close_editor(0).unwrap();
}

#[test]
#[ignore = "needs helper and synth fixtures and a desktop session"]
fn a_floating_editor_follows_its_own_requests_and_reopens() {
    let mut chain = spawn(&[fixture(
        PluginFormat::Clap,
        "plughost-test-synth",
        "com.studio.plughost.test-synth",
    )]);
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::None, &[Layout::Stereo])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    let pid = chain.helper_monitor().process_id();
    assert_eq!(synth_editor(pid), None);
    chain.open_editor(0).unwrap();
    expect(&mut chain, true, Some(true));
    // The host's own window stays hidden behind a floating editor.
    assert!(
        windows(pid)
            .iter()
            .all(|window| window.width == SYNTH_EDITOR_WIDTH || !window.on_screen)
    );
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
#[ignore = "needs scripts/build-helper.sh and a desktop session"]
fn an_audio_unit_view_shows_in_a_window_that_fits_it_and_reopens() {
    let mut chain = spawn(&[audio_unit(AU_DELAY)]);
    prepare(&mut chain);
    let pid = chain.helper_monitor().process_id();
    let mut sizes = Vec::new();
    for _ in 0..2 {
        chain.open_editor(0).unwrap();
        wait(
            &mut chain,
            |chain| {
                chain.editor_open(0).unwrap() && windows(pid).iter().any(|window| window.on_screen)
            },
            "the Audio Unit's editor window never showed",
        );
        let shown: Vec<_> = windows(pid)
            .into_iter()
            .filter(|window| window.on_screen)
            .map(|window| (window.width, window.height))
            .collect();
        assert_eq!(shown.len(), 1, "{shown:?}");
        sizes.push(shown[0]);
        chain.close_editor(0).unwrap();
        wait(
            &mut chain,
            |chain| {
                !chain.editor_open(0).unwrap()
                    && windows(pid).iter().all(|window| !window.on_screen)
            },
            "the Audio Unit's editor window stayed on screen",
        );
    }
    // The window takes the view's size, not the 400 × 300 content it is created with, and a
    // reopened view is the same.
    assert_ne!(sizes[0].0, 400.0, "{sizes:?}");
    assert_eq!(sizes[0], sizes[1]);
}
