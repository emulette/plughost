//! Slots inserted, moved, replaced and removed in a running chain: the helper and the other
//! slots' plugins stay, and a prepared chain is prepared again for the slots it has.

use plughost::{ChainSlot, Error, FailureKind, InputError, PluginFormat, StatePurpose};

mod support;

use support::{block, delay, prepare, routing, spawn};

/// The delay fixture's gain parameter.
const GAIN: u64 = 0;

/// The slot's normalized gain.
fn gain(chain: &mut plughost::Chain, slot: usize) -> f64 {
    chain
        .parameters(slot)
        .unwrap()
        .into_iter()
        .find(|(info, _)| info.id == GAIN)
        .and_then(|(_, value)| value)
        .unwrap()
}

/// The last sample of four blocks of constant 0.25 through the prepared chain: past the 480
/// samples each delay fixture is late.
fn output(chain: &mut plughost::Chain) -> f32 {
    (0..4)
        .map(|_| *block(chain, 512, 0.25).last().unwrap())
        .last()
        .unwrap()
}

#[test]
#[ignore = "needs helper and native delay and routing fixtures"]
fn slots_change_in_the_running_helper_and_the_chain_is_prepared_again_for_them() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = spawn(&[delay(format)]);
        let helper = chain.helper_monitor().process_id();
        chain.set_parameter(0, GAIN, 0.25).unwrap();
        prepare(&mut chain);
        // What one slot at this gain multiplies the input by.
        let factor = output(&mut chain) / 0.25;
        assert!(factor > 0.0 && factor != 1.0, "{format:?}");

        // A slot inserted after it leaves the chain unprepared; prepared again, both process.
        chain
            .insert_slot(1, &ChainSlot::from(delay(format)))
            .unwrap();
        assert_eq!(chain.plugins().len(), 2);
        assert!(
            matches!(
                chain.process_audio_f32(
                    &plughost::BlockContext::new(512),
                    &[&[0.0; 512], &[0.0; 512]],
                    &mut [&mut [0.0; 512], &mut [0.0; 512]],
                    &[],
                    &plughost::Events::new(),
                    &mut plughost::Events::for_block(),
                ),
                Err(Error::NotPrepared)
            ),
            "{format:?}: the changed chain stayed prepared"
        );
        chain.set_parameter(1, GAIN, 0.25).unwrap();
        prepare(&mut chain);
        assert_eq!(output(&mut chain), 0.25 * factor * factor, "{format:?}");

        // A moved slot takes its plugin, with its gain, along.
        chain.set_parameter(1, GAIN, 0.75).unwrap();
        chain.move_slot(1, 0).unwrap();
        assert_eq!((gain(&mut chain, 0), gain(&mut chain, 1)), (0.75, 0.25));

        // A replacement takes the state it is given; one of another class leaves the slot.
        let state = chain.save_state(1, StatePurpose::Project).unwrap();
        chain
            .replace_slot(0, &ChainSlot::from(delay(format)).with_state(state))
            .unwrap();
        assert_eq!(gain(&mut chain, 0), 0.25);
        let foreign = spawn(&[routing(format)])
            .save_state(0, StatePurpose::Project)
            .unwrap();
        let refused = chain
            .replace_slot(0, &ChainSlot::from(delay(format)).with_state(foreign))
            .unwrap_err();
        assert_eq!(refused.kind(), FailureKind::StateMismatch, "{format:?}");
        assert_eq!(gain(&mut chain, 0), 0.25);

        // A position past the end and the last slot are refused.
        assert!(matches!(
            chain.insert_slot(3, &ChainSlot::from(delay(format))),
            Err(Error::Input {
                error: InputError::Slot,
                ..
            })
        ));
        chain.remove_slot(1).unwrap();
        assert!(matches!(
            chain.remove_slot(0),
            Err(Error::Input {
                error: InputError::EmptyChain,
                ..
            })
        ));
        assert_eq!(chain.plugins().len(), 1);
        prepare(&mut chain);
        assert_eq!(output(&mut chain), 0.25 * factor, "{format:?}");
        assert_eq!(chain.helper_monitor().process_id(), helper, "{format:?}");
    }
}
