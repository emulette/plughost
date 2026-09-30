//! Recovering a chain in a new helper from the application's last saved states. Build the helper
//! and the test plugins first (see `support`).

use plughost::{Chain, Error, FailureKind, InputError, PluginFormat, PluginState, StatePurpose};

mod support;

use support::{block, delay, delay_variant, prepare, prepared_delay, routing, spawn};

const GAIN: u64 = 0;

fn gain(chain: &mut Chain) -> f64 {
    chain
        .parameters(0)
        .unwrap()
        .into_iter()
        .find(|(info, _)| info.id == GAIN)
        .unwrap()
        .1
}

fn snapshot(chain: &mut Chain) -> Vec<PluginState> {
    (0..chain.plugins().len())
        .map(|slot| chain.save_state(slot, StatePurpose::Project).unwrap())
        .collect()
}

#[test]
#[ignore = "needs helper and delay fixtures (.ps1 or .sh build scripts)"]
fn recovery_prepares_a_separate_chain_from_the_snapshot() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = prepared_delay(format);
        chain.set_parameter(0, GAIN, 0.25).unwrap();
        let states = snapshot(&mut chain);
        // An edit after the snapshot is not part of the recovered chain.
        chain.set_parameter(0, GAIN, 0.75).unwrap();
        let mut recovered = chain.recover(&states).unwrap();
        assert_ne!(
            recovered.helper_monitor().process_id(),
            chain.helper_monitor().process_id()
        );
        assert_eq!(gain(&mut recovered), 0.25, "{format:?}");
        assert_eq!(recovered.latency(), chain.latency());
        let output = block(&mut recovered, 512, 1.0);
        assert!(output.contains(&0.25), "{format:?}");
        // The original chain keeps running with its own settings until the application drops it.
        block(&mut chain, 512, 1.0);
        assert_eq!(gain(&mut chain), 0.75, "{format:?}");
    }
}

#[test]
#[ignore = "needs helper and delay fixtures (.ps1 or .sh build scripts)"]
fn a_crashed_chain_recovers_from_its_last_snapshot() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = spawn(&[delay(format), delay_variant(format, "crash-in-process", 2)]);
        prepare(&mut chain);
        chain.set_parameter(0, GAIN, 0.25).unwrap();
        let states = snapshot(&mut chain);
        assert!(matches!(
            chain.process_audio_f32(
                &plughost::BlockContext::new(64),
                &[&[0.0; 64], &[0.0; 64]],
                &mut [&mut [0.0; 64], &mut [0.0; 64]],
                &[],
                &[],
                &mut Vec::new(),
            ),
            Err(Error::Crashed { slot: Some(1), .. })
        ));
        let mut recovered = chain.recover(&states).unwrap();
        assert_eq!(gain(&mut recovered), 0.25, "{format:?}");
        assert!(matches!(chain.parameters(0), Err(Error::Crashed { .. })));
    }
}

#[test]
#[ignore = "needs helper, delay and routing fixtures (.ps1 or .sh build scripts)"]
fn recovery_rejects_states_that_do_not_belong_to_the_chain() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = prepared_delay(format);
        let state = chain.save_state(0, StatePurpose::Project).unwrap();
        assert!(matches!(
            chain.recover(&[state.clone(), state]),
            Err(Error::Input {
                slot: None,
                error: InputError::StateCount
            })
        ));
        let mut other = spawn(&[routing(format)]);
        let foreign = other.save_state(0, StatePurpose::Project).unwrap();
        let Err(error) = chain.recover(&[foreign]) else {
            panic!("{format:?}: a foreign state was restored");
        };
        assert_eq!(error.kind(), FailureKind::StateMismatch, "{format:?}");
        assert!(matches!(error, Error::Operation { slot: Some(0), .. }));
        block(&mut chain, 512, 1.0);
    }
}
