//! Editor windows of a real helper on the macOS desktop: keyboard focus, editors that show, hide
//! and close themselves, and Audio Unit views. Build the helper and the test plugins first (see
//! `support`).
#![cfg(target_os = "macos")]

use std::time::Duration;

use plughost::{BlockContext, Chain, Event, Events, Layout, PluginFormat, PluginRef, StatePurpose};

mod support;

use support::desktop::{Window, act, wait, windows};
use support::{delay_variant, fixture, prepare, spawn};

/// 'aufx' 'dely' 'appl'
const AU_DELAY: &str = "6175667864656C796170706C";
/// The synth fixture's editor window is the only helper window this wide.
const SYNTH_EDITOR_WIDTH: f64 = 200.0;

fn audio_unit(class_id: &str) -> PluginRef {
    PluginRef::audio_unit(class_id)
}

/// Whether the synth's floating editor window exists, and if so whether it is on screen.
fn synth_editor(pid: u32) -> Option<bool> {
    windows(pid)
        .into_iter()
        .find(|window| window.width == SYNTH_EDITOR_WIDTH)
        .map(|window| window.on_screen)
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
            &Events::from_iter([Event::note_on(0, 0, key, 127)]),
            &mut Events::for_block(),
        )
        .unwrap();
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn an_opened_editor_window_gives_the_editor_keyboard_focus() {
    let _serial = support::editor_test();
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
            chain.parameter_to_plain(0, FOCUS, value.unwrap()).unwrap() == 2.0
        },
        "keys never reached the editor's view",
    );
    chain.close_editor(0).unwrap();
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_window_size_the_editor_refuses_returns_to_the_size_it_accepted() {
    let _serial = support::editor_test();
    // The editor variant drags its window 60 points past its 240 by 120 size, which it refuses,
    // and counts the sizes it refused.
    const REFUSED_SIZES: u64 = 16;
    let refused = |chain: &mut Chain| {
        let (_, value) = chain
            .parameters(0)
            .unwrap()
            .into_iter()
            .find(|(info, _)| info.id == REFUSED_SIZES)
            .unwrap();
        chain
            .parameter_to_plain(0, REFUSED_SIZES, value.unwrap())
            .unwrap()
    };
    let mut chain = spawn(&[delay_variant(PluginFormat::Clap, "editor", 0xB)]);
    let pid = chain.helper_monitor().process_id();
    chain.open_editor(0).unwrap();
    wait(
        &mut chain,
        |chain| refused(chain) >= 1.0,
        "the editor was never offered the dragged size",
    );
    wait(
        &mut chain,
        |_| {
            windows(pid)
                .iter()
                .any(|window| window.on_screen && window.width == 240.0)
        },
        "the window kept the refused size",
    );
    // Restored, the window is not offered the refused size again.
    let count = refused(&mut chain);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(refused(&mut chain), count);
    chain.close_editor(0).unwrap();
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
#[ignore = "needs helper and synth fixtures and a desktop session"]
fn a_hidden_floating_editor_stays_hidden_when_the_chain_is_prepared_again() {
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
    chain.open_editor(0).unwrap();
    expect(&mut chain, true, Some(true));
    request(&mut chain, 0);
    expect(&mut chain, true, Some(false));
    // The synth cannot save its state, so preparing again reopens its editor in place.
    chain.prepare_audio(&config).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    expect(&mut chain, true, Some(false));
    chain.close_editor(0).unwrap();
}

#[test]
#[ignore = "needs scripts/build-helper.sh and a desktop session"]
fn an_editor_keeps_its_window_when_its_state_is_restored() {
    let _serial = support::editor_test();
    let mut chain = spawn(&[audio_unit(AU_DELAY)]);
    prepare(&mut chain);
    let pid = chain.helper_monitor().process_id();
    let shown = || -> Vec<Window> {
        windows(pid)
            .into_iter()
            .filter(|window| window.on_screen)
            .collect()
    };
    chain.open_editor(0).unwrap();
    wait(
        &mut chain,
        |_| !shown().is_empty(),
        "the Audio Unit's editor window never showed",
    );
    let before = shown();
    let state = chain.save_state(0, StatePurpose::Project).unwrap();
    chain
        .restore_state(0, &state, StatePurpose::Project)
        .unwrap();
    assert!(chain.editor_open(0).unwrap());
    // The new instance's editor is in the same window, at the same place and size.
    assert_eq!(shown(), before);
    chain.close_editor(0).unwrap();
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn a_hidden_helper_keeps_its_editors_open() {
    let _serial = support::editor_test();
    let mut chain = spawn(&[delay_variant(PluginFormat::Clap, "editor", 0xB)]);
    let pid = chain.helper_monitor().process_id();
    let shown = || windows(pid).iter().any(|window| window.on_screen);
    chain.open_editor(0).unwrap();
    wait(&mut chain, |_| shown(), "the editor window never showed");
    act(&mut chain, 1);
    wait(
        &mut chain,
        |_| !shown(),
        "the hidden helper's window stayed on screen",
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(chain.editor_open(0).unwrap());
    act(&mut chain, 2);
    wait(
        &mut chain,
        |_| shown(),
        "the editor window did not come back",
    );
    assert!(chain.editor_open(0).unwrap());
    chain.close_editor(0).unwrap();
}

#[test]
#[ignore = "needs helper and delay fixtures and a desktop session"]
fn an_editor_whose_window_the_user_closes_is_closed() {
    let _serial = support::editor_test();
    let mut chain = spawn(&[delay_variant(PluginFormat::Clap, "editor", 0xB)]);
    let pid = chain.helper_monitor().process_id();
    chain.open_editor(0).unwrap();
    wait(
        &mut chain,
        |_| windows(pid).iter().any(|window| window.on_screen),
        "the editor window never showed",
    );
    act(&mut chain, 3);
    wait(
        &mut chain,
        |chain| {
            !chain.editor_open(0).unwrap() && windows(pid).iter().all(|window| !window.on_screen)
        },
        "the closed window's editor stayed open",
    );
}

#[test]
#[ignore = "needs scripts/build-helper.sh and a desktop session"]
fn an_audio_unit_view_shows_in_a_window_that_fits_it_and_reopens() {
    let _serial = support::editor_test();
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
