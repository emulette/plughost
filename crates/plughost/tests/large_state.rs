use std::time::Duration;

use plughost::{Error, FailureKind, HostIdentity, PluginFormat, Timeouts};

mod support;

use support::{delay, delay_variant, prepare, spawn, spawn_with};

#[test]
#[ignore = "needs scripts/build-helper and scripts/build-test-plugins (.ps1 or .sh)"]
fn large_native_state_crosses_ipc_and_oversized_native_output_is_not_accepted() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let plugin = delay_variant(format, "large-state", 8);
        let mut chain = spawn(&[plugin]);
        prepare(&mut chain);
        chain.set_parameter(0, 0, 0.25).unwrap();
        let state = chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap();
        assert_eq!(state.component.len(), (8 << 20) + 8);
        assert_eq!(&state.component[..8], &0.25f64.to_le_bytes());
        for (index, chunk) in state.component[8..].chunks_exact(65536).enumerate() {
            assert!(chunk.iter().all(|&byte| byte == index as u8));
        }
        chain.set_parameter(0, 0, 0.75).unwrap();
        chain
            .restore_state(0, &state, plughost::StatePurpose::Project)
            .unwrap();
        assert_eq!(
            chain
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap(),
            state
        );

        // This native fixture ignores failed writes and reports success after attempting 129 MiB.
        // The owning format must still reject the incomplete state, and the helper stays usable.
        chain.set_parameter(0, 0, 0.0).unwrap();
        match chain
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap_err()
        {
            Error::Operation { slot, failure } => {
                assert_eq!(slot, Some(0));
                assert_eq!(failure.kind, FailureKind::State);
            }
            error => panic!("unexpected large state failure: {error:?}"),
        }
        chain.set_parameter(0, 0, 0.25).unwrap();
        assert_eq!(
            chain
                .save_state(0, plughost::StatePurpose::Project)
                .unwrap(),
            state
        );
    }
}

#[test]
#[ignore = "needs scripts/build-helper and scripts/build-test-plugins (.ps1 or .sh)"]
fn recovery_restores_states_that_together_exceed_one_message() {
    let plugins = [0.25, 0.5, 0.75].map(|_| delay(PluginFormat::Clap));
    let mut chain = spawn(&plugins);
    let states: Vec<_> = [0.25, 0.5, 0.75]
        .into_iter()
        .enumerate()
        .map(|(slot, gain)| {
            chain.set_parameter(slot, 0, gain).unwrap();
            let mut state = chain
                .save_state(slot, plughost::StatePurpose::Project)
                .unwrap();
            // The delay reads its gain from the first bytes and ignores the rest.
            state.component.resize(100 << 20, 0);
            state
        })
        .collect();
    let mut recovered = chain.recover(&states).unwrap();
    for (slot, gain) in [0.25, 0.5, 0.75].into_iter().enumerate() {
        let (_, value) = recovered.parameters(slot).unwrap()[0].clone();
        assert_eq!(value, gain);
    }
}

#[test]
#[ignore = "needs scripts/build-helper and scripts/build-test-plugins (.ps1 or .sh)"]
fn native_state_save_timeout_terminates_the_helper_and_explicit_recovery_replaces_it() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let plugin = delay_variant(format, "large-state", 8);
        let mut chain = spawn_with(&[plugin], &HostIdentity::default(), {
            let mut timeouts = Timeouts::default();
            timeouts.control = Duration::from_millis(500);
            timeouts
        });
        prepare(&mut chain);
        chain.set_parameter(0, 0, 0.125).unwrap();
        assert!(matches!(
            chain.save_state(0, plughost::StatePurpose::Project),
            Err(Error::TimedOut { slot: Some(0) })
        ));
        let input = [1.0f32; 512];
        let mut left = [42.0f32; 512];
        let mut right = [42.0f32; 512];
        assert!(
            chain
                .process_audio_f32(
                    &plughost::BlockContext::new(512),
                    &[&input, &input],
                    &mut [&mut left, &mut right],
                    &[],
                    &[],
                    &mut Vec::new(),
                )
                .is_err()
        );
        assert_eq!(left, [42.0; 512]);
        assert_eq!(right, [42.0; 512]);
        chain = chain.recover(&[]).unwrap();
        chain
            .process_audio_f32(
                &plughost::BlockContext::new(512),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(left[480], 1.0);
        assert_eq!(right[480], 1.0);
    }
}
