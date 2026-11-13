//! Native bypass through a VST3 controller that takes every `setParamNormalized` value but
//! answers `kResultFalse`, as many commercial plugins do, and through the instances the helper
//! puts in a slot's place. The `routing-strict` bundle of the routing fixture is such a plugin; it
//! counts the writes to its bypass parameter.
use plughost::*;
mod support;

use support::{fixture, prepare, spawn};

fn strict() -> PluginRef {
    fixture(
        PluginFormat::Vst3,
        "plughost-test-routing-strict",
        "706C7567686F7374526F7574696E6701",
    )
}

/// How many times the host wrote the slot's bypass parameter.
fn writes(chain: &mut Chain, slot: usize) -> u32 {
    let (_, value) = chain
        .parameters(slot)
        .unwrap()
        .into_iter()
        .find(|(info, _)| info.title == "Bypass writes")
        .unwrap();
    (value.unwrap() * 100.0).round() as u32
}

/// Renders a constant stereo signal and returns its first sample on the left: the input when
/// the fixture is bypassed, the input times its gain otherwise.
fn level(chain: &mut Chain) -> f32 {
    chain.reset().unwrap();
    let input = vec![0.5f32; 64];
    let rendered = render(
        chain,
        &[&input, &input],
        64,
        &Events::new(),
        &RenderOptions::new(TailPolicy::Reported, 0.0),
    )
    .unwrap();
    rendered.channels[0][0]
}

#[test]
#[ignore = "needs helper and the routing-strict fixture (.ps1 or .sh build scripts)"]
fn native_bypass_reaches_a_controller_that_answers_every_value_with_false() {
    let mut chain = spawn(&[strict()]);
    // The gain tells the processed signal from the bypassed one.
    chain.set_parameter(0, 0, 0.5).unwrap();
    prepare(&mut chain);
    assert_eq!(level(&mut chain), 0.25);
    // Preparing again copies the bypass to a fresh instance, as restoring a state does.
    prepare(&mut chain);
    let state = chain.save_state(0, StatePurpose::Project).unwrap();
    chain
        .restore_state(0, &state, StatePurpose::Project)
        .unwrap();
    chain.set_bypass(0, true).unwrap();
    assert_eq!(chain.bypass(0).unwrap().enabled, Some(true));
    assert_eq!(level(&mut chain), 0.5);
    prepare(&mut chain);
    let bypassed = chain.save_state(0, StatePurpose::Project).unwrap();
    chain
        .restore_state(0, &bypassed, StatePurpose::Project)
        .unwrap();
    chain.reprepare_slot(0).unwrap();
    assert_eq!(chain.bypass(0).unwrap().enabled, Some(true));
    assert_eq!(level(&mut chain), 0.5);
    chain.set_bypass(0, false).unwrap();
    assert_eq!(level(&mut chain), 0.25);
    for enabled in [false, true] {
        let slot = ChainSlot::new(strict()).with_bypass(enabled);
        let mut spawned = Chain::spawn(
            &support::helper(),
            &[slot],
            &SpawnOptions::new(HostIdentity::default(), Timeouts::default()),
        )
        .unwrap();
        assert_eq!(spawned.bypass(0).unwrap().enabled, Some(enabled));
    }
}

#[test]
#[ignore = "needs helper and the routing-strict fixture (.ps1 or .sh build scripts)"]
fn a_fresh_instance_takes_the_slot_bypass_only_when_it_has_another() {
    let mut chain = spawn(&[strict()]);
    prepare(&mut chain);
    let active = chain.save_state(0, StatePurpose::Project).unwrap();
    // The fresh instances of a re-preparation, a restored state and a re-prepared slot have the
    // slot's bypass already, from the state or their defaults.
    prepare(&mut chain);
    assert_eq!(writes(&mut chain, 0), 0);
    chain
        .restore_state(0, &active, StatePurpose::Project)
        .unwrap();
    assert_eq!(writes(&mut chain, 0), 0);
    chain.reprepare_slot(0).unwrap();
    assert_eq!(writes(&mut chain, 0), 0);
    chain.set_bypass(0, true).unwrap();
    assert!(writes(&mut chain, 0) > 0);
    prepare(&mut chain);
    assert_eq!(writes(&mut chain, 0), 0);
    assert_eq!(chain.bypass(0).unwrap().enabled, Some(true));
    // A fresh instance for a state still takes the slot's bypass first, which the state then
    // sets: this one holds bypass off.
    chain
        .restore_state(0, &active, StatePurpose::Project)
        .unwrap();
    assert!(writes(&mut chain, 0) > 0);
    assert_eq!(chain.bypass(0).unwrap().enabled, Some(false));
    // A slot's explicit bypass is written when the instance does not have it.
    chain
        .insert_slot(1, &ChainSlot::new(strict()).with_bypass(false))
        .unwrap();
    assert_eq!(writes(&mut chain, 1), 0);
    chain
        .replace_slot(1, &ChainSlot::new(strict()).with_bypass(true))
        .unwrap();
    assert!(writes(&mut chain, 1) > 0);
    assert_eq!(chain.bypass(1).unwrap().enabled, Some(true));
}
