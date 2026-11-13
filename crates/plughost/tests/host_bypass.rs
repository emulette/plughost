//! Host bypass through the helper: the plugin goes on processing, and its slot plays its main
//! input, delayed by the slot's latency, in place of its main output. The delay fixtures delay by
//! 480 samples at a gain other than 1 here, so that their output and the delayed input differ.
use plughost::*;
mod support;

use support::{delay, prepare, routing, spawn, synth};

/// The delay fixture's latency.
const LATENCY: usize = 480;

fn options() -> RenderOptions {
    RenderOptions::new(TailPolicy::Reported, 0.0)
}

/// A different signal on each of `channels` channels.
fn signal(channels: usize, frames: usize) -> Vec<Vec<f32>> {
    (0..channels)
        .map(|channel| {
            (0..frames)
                .map(|frame| ((frame as f32) * 0.013 + channel as f32).sin() * 0.5)
                .collect()
        })
        .collect()
}

fn rendered(chain: &mut Chain, input: &[Vec<f32>]) -> Rendered<f32> {
    let channels: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    render(chain, &channels, input[0].len(), &Events::new(), &options()).unwrap()
}

/// The delay fixture in `format`, at half gain, in a chain prepared in stereo.
fn half_delay(format: PluginFormat) -> Chain {
    let mut chain = spawn(&[delay(format)]);
    chain.set_parameter(0, 0, 0.5).unwrap();
    prepare(&mut chain);
    chain
}

/// Processes one stereo block and returns its output.
fn block(chain: &mut Chain, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let frames = input[0].len();
    let mut output = vec![vec![0.0; frames]; 2];
    let [left, right] = &mut output[..] else {
        unreachable!()
    };
    chain
        .process_audio_f32(
            &BlockContext::new(frames),
            &[&input[0], &input[1]],
            &mut [left, right],
            &[],
            &Events::new(),
            &mut Events::for_block(),
        )
        .unwrap();
    output
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn host_bypass_plays_the_input_at_the_slot_latency_from_the_next_block_on() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = half_delay(format);
        // The fixture has no native bypass.
        assert_eq!(chain.bypass(0).unwrap().support, Support::Unsupported);
        let input = signal(2, 4096);
        assert_ne!(rendered(&mut chain, &input).channels, input, "{format:?}");
        assert!(!chain.host_bypass(0).unwrap());
        chain.set_host_bypass(0, true).unwrap();
        assert!(chain.host_bypass(0).unwrap());
        let output = rendered(&mut chain, &input);
        assert_eq!(output.latency as usize, LATENCY, "{format:?}");
        assert_eq!(output.channels, input, "{format:?}");
        assert_eq!(chain.latency() as usize, LATENCY);

        // Offline, a switch applies from the first frame of the next block, and the delayed
        // input it plays was gathered while host bypass was off.
        chain.set_host_bypass(0, false).unwrap();
        chain.reset().unwrap();
        let first: Vec<Vec<f32>> = input
            .iter()
            .map(|channel| channel[..512].to_vec())
            .collect();
        let second: Vec<Vec<f32>> = input
            .iter()
            .map(|channel| channel[512..1024].to_vec())
            .collect();
        let wet = block(&mut chain, &first);
        assert!(wet[0][..LATENCY].iter().all(|&sample| sample == 0.0));
        assert_ne!(wet[0][LATENCY..], input[0][..512 - LATENCY]);
        chain.set_host_bypass(0, true).unwrap();
        let dry = block(&mut chain, &second);
        for (output, input) in dry.iter().zip(&input) {
            assert_eq!(
                output[..],
                input[512 - LATENCY..1024 - LATENCY],
                "{format:?}"
            );
        }
    }
}

#[test]
#[ignore = "needs helper and the delay and routing fixtures (.ps1 or .sh build scripts)"]
fn host_bypass_keeps_the_chain_latency_and_the_alignment_after_the_slot() {
    // The delay, bypassed at half gain, and the delay at unity gain both deliver the input 480
    // samples late, to which the routing fixture's external key must stay aligned.
    let config = RoutedChainConfig::new(
        48_000.0,
        512,
        SampleFormat::F32,
        vec![Layout::Stereo, Layout::Mono],
        vec![
            SlotAudioConfig::new(
                vec![AudioInputRoute::new(
                    AudioBusConfig::new(0, Layout::Stereo, true),
                    AudioSource::External { bus: 0 },
                )],
                vec![AudioBusConfig::new(0, Layout::Stereo, true)],
            ),
            SlotAudioConfig::new(
                vec![
                    AudioInputRoute::new(
                        AudioBusConfig::new(1, Layout::Stereo, true),
                        AudioSource::Previous { bus: 0 },
                    ),
                    AudioInputRoute::new(
                        AudioBusConfig::new(0, Layout::Mono, true),
                        AudioSource::External { bus: 1 },
                    ),
                ],
                vec![AudioBusConfig::new(1, Layout::Stereo, true)],
            ),
        ],
    );
    let mut input = signal(2, 4096);
    input.push(
        signal(3, 4096)
            .remove(2)
            .iter()
            .map(|key| key * 0.5)
            .collect(),
    );
    let mut reference = spawn(&[delay(PluginFormat::Vst3), routing(PluginFormat::Vst3)]);
    reference.prepare_audio(&config).unwrap();
    let expected = rendered(&mut reference, &input);
    let mut chain = spawn(&[delay(PluginFormat::Vst3), routing(PluginFormat::Vst3)]);
    chain.set_parameter(0, 0, 0.5).unwrap();
    chain.prepare_audio(&config).unwrap();
    assert_ne!(rendered(&mut chain, &input).channels, expected.channels);
    chain.set_host_bypass(0, true).unwrap();
    let output = rendered(&mut chain, &input);
    assert_eq!(output.latency, expected.latency);
    assert_eq!(output.channels, expected.channels);

    // Two delays, the first bypassed: the input comes out 960 samples late, as without bypass.
    let mut chain = spawn(&[delay(PluginFormat::Clap), delay(PluginFormat::Vst3)]);
    prepare(&mut chain);
    chain.set_parameter(0, 0, 0.5).unwrap();
    chain.set_host_bypass(0, true).unwrap();
    let input = signal(2, 4096);
    let output = rendered(&mut chain, &input);
    assert_eq!(output.latency as usize, 2 * LATENCY);
    assert_eq!(output.channels, input);
}

#[test]
#[ignore = "needs helper and the routing fixtures (.ps1 or .sh build scripts)"]
fn host_bypass_silences_auxiliary_outputs_while_the_sidechain_feeds_the_plugin() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let (key, main, auxiliary, out) = match format {
            PluginFormat::Vst3 => (0, 1, 0, 1),
            _ => (20, 21, 30, 31),
        };
        let mut setup = SlotAudioConfig::new(
            vec![
                AudioInputRoute::new(
                    AudioBusConfig::new(key, Layout::Mono, true),
                    AudioSource::External { bus: 0 },
                ),
                AudioInputRoute::new(
                    AudioBusConfig::new(main, Layout::Stereo, true),
                    AudioSource::External { bus: 1 },
                ),
            ],
            vec![
                AudioBusConfig::new(auxiliary, Layout::Stereo, true),
                AudioBusConfig::new(out, Layout::Stereo, true),
            ],
        );
        setup.configuration = (format == PluginFormat::Clap).then_some(101);
        let config = RoutedChainConfig::new(
            48_000.0,
            256,
            SampleFormat::F32,
            vec![Layout::Mono, Layout::Stereo],
            vec![setup],
        );
        let mut chain = spawn(&[routing(format)]);
        chain.prepare_audio(&config).unwrap();
        let mut input = vec![vec![0.25; 1024]];
        input.extend(signal(2, 1024));
        // Active, the auxiliary output carries the main input's left channel and the key.
        let active = rendered(&mut chain, &input).channels;
        chain.set_host_bypass(0, true).unwrap();
        let output = rendered(&mut chain, &input).channels;
        // The fixture puts its main bus first in CLAP and last in VST3.
        let (main_channels, auxiliary_channels) = match format {
            PluginFormat::Vst3 => (2..4, 0..2),
            _ => (0..2, 2..4),
        };
        assert!(
            active[auxiliary_channels.clone()]
                .iter()
                .any(|channel| channel.iter().any(|&sample| sample != 0.0))
        );
        assert_eq!(output[main_channels], input[1..], "{format:?}");
        for channel in &output[auxiliary_channels] {
            assert!(channel.iter().all(|&sample| sample == 0.0), "{format:?}");
        }
    }
}

#[test]
#[ignore = "needs helper and the synth fixture (.ps1 or .sh build scripts)"]
fn a_slot_without_a_main_input_refuses_host_bypass() {
    let mut chain = spawn(&[synth(PluginFormat::Clap)]);
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::None, &[Layout::Stereo])
        .unwrap();
    // An unprepared chain takes it, and preparing with it fails for the slot.
    chain.set_host_bypass(0, true).unwrap();
    match chain.prepare_audio(&config).unwrap_err() {
        Error::Operation {
            slot: Some(0),
            failure,
        } => assert_eq!(failure.kind, FailureKind::Configuration),
        other => panic!("{other:?}"),
    }
    chain.set_host_bypass(0, false).unwrap();
    chain.prepare_audio(&config).unwrap();
    match chain.set_host_bypass(0, true).unwrap_err() {
        Error::Operation {
            slot: Some(0),
            failure,
        } => assert_eq!(failure.kind, FailureKind::Configuration),
        other => panic!("{other:?}"),
    }
    assert!(!chain.host_bypass(0).unwrap());
}

/// The left channel of the chain's render of `input`, compared with `input`: 1 when it passes
/// through, the product of the delays' gains otherwise.
fn gain(chain: &mut Chain, input: &[Vec<f32>]) -> f32 {
    rendered(chain, input).channels[0][100] / input[0][100]
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn host_bypass_stays_with_its_slot() {
    let input = signal(2, 2048);
    let slots = [
        ChainSlot::new(delay(PluginFormat::Vst3)).with_host_bypass(true),
        ChainSlot::new(delay(PluginFormat::Clap)),
    ];
    let mut chain = Chain::spawn(
        &support::helper(),
        &slots,
        &SpawnOptions::new(HostIdentity::default(), Timeouts::default()),
    )
    .unwrap();
    assert!(chain.host_bypass(0).unwrap());
    chain.set_parameter(0, 0, 0.5).unwrap();
    chain.set_parameter(1, 0, 0.5).unwrap();
    prepare(&mut chain);
    let half = gain(&mut chain, &input);
    assert!((half - 0.5).abs() < 1e-6, "{half}");
    // Preparing again and replacing the slot's instance keep it.
    prepare(&mut chain);
    let state = chain.save_state(0, StatePurpose::Project).unwrap();
    chain
        .restore_state(0, &state, StatePurpose::Project)
        .unwrap();
    chain.reprepare_slot(0).unwrap();
    assert!(chain.host_bypass(0).unwrap());
    assert_eq!(gain(&mut chain, &input), half);
    // Slot changes move it with its slot.
    chain
        .insert_slot(0, &ChainSlot::new(delay(PluginFormat::Clap)))
        .unwrap();
    chain.move_slot(1, 2).unwrap();
    let flags = |chain: &mut Chain| {
        (0..chain.plugins().len())
            .map(|slot| chain.host_bypass(slot).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(flags(&mut chain), [false, false, true]);
    chain.remove_slot(0).unwrap();
    assert_eq!(flags(&mut chain), [false, true]);
    chain
        .replace_slot(
            0,
            &ChainSlot::new(delay(PluginFormat::Vst3)).with_host_bypass(true),
        )
        .unwrap();
    assert_eq!(flags(&mut chain), [true, true]);
    chain.set_host_bypass(0, false).unwrap();
    // A snapshot holds it, and a recovered chain has it again.
    let snapshot = chain.snapshot().unwrap();
    assert_eq!(
        snapshot
            .slots
            .iter()
            .map(|slot| slot.host_bypass)
            .collect::<Vec<_>>(),
        [false, true]
    );
    prepare(&mut chain);
    let mut recovered = chain.recover(&snapshot.slots).unwrap();
    assert_eq!(flags(&mut recovered), [false, true]);
    assert_eq!(gain(&mut recovered, &input), 1.0);
}

#[test]
#[ignore = "needs helper and the delay fixture (.ps1 or .sh build scripts)"]
fn a_host_bypassed_slot_adds_no_tail() {
    let mut chain = half_delay(PluginFormat::Vst3);
    let tail = chain
        .parameters(0)
        .unwrap()
        .into_iter()
        .find(|(info, _)| info.title == "Tail samples")
        .unwrap()
        .0
        .id;
    // 16 of the parameter's 0 to 32 samples.
    chain.set_parameter(0, tail, 0.5).unwrap();
    assert_eq!(chain.tail(), Tail::Samples(16));
    let input = signal(2, 1024);
    let channels = [input[0].as_slice(), input[1].as_slice()];
    let options = RenderOptions::new(TailPolicy::Reported, 1.0);
    let with_tail =
        |chain: &mut Chain| render(chain, &channels, 1024, &Events::new(), &options).unwrap();
    assert_eq!(with_tail(&mut chain).tail, 16);
    chain.set_host_bypass(0, true).unwrap();
    assert_eq!(chain.tail(), Tail::Samples(0));
    let output = with_tail(&mut chain);
    assert_eq!((output.tail, output.channels), (0, input));
    chain.set_host_bypass(0, false).unwrap();
    assert_eq!(chain.tail(), Tail::Samples(16));
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs helper"]
fn host_bypass_lines_up_where_native_bypass_of_a_latent_audio_unit_does_not() {
    // Apple's AUPeakLimiter, 'aufx' 'lmtr' 'appl', which keeps reporting its latency while its
    // native bypass stops delaying.
    let limiter = PluginRef::audio_unit("617566786C6D74726170706C");
    let mut chain = spawn(&[limiter]);
    prepare(&mut chain);
    let input: Vec<Vec<f32>> = signal(2, 8192)
        .into_iter()
        .map(|channel| channel.into_iter().map(|sample| sample * 0.1).collect())
        .collect();
    chain.set_host_bypass(0, true).unwrap();
    let output = rendered(&mut chain, &input);
    assert!(output.latency > 0);
    assert_eq!(output.channels, input);
    chain.set_host_bypass(0, false).unwrap();
    chain.set_bypass(0, true).unwrap();
    assert_ne!(rendered(&mut chain, &input).channels, input);
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs helper"]
fn host_bypass_of_a_mono_to_stereo_slot_puts_its_input_on_both_sides() {
    // Apple's AUMatrixReverb, 'aufx' 'mrev' 'appl', with a mono input and a stereo output.
    let reverb = PluginRef::audio_unit("617566786D7265766170706C");
    let mut chain = spawn(&[reverb]);
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::Mono, &[Layout::Stereo])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    chain.set_host_bypass(0, true).unwrap();
    let input = signal(1, 4096);
    let output = rendered(&mut chain, &input);
    assert_eq!(output.channels, [input[0].clone(), input[0].clone()]);
}

#[test]
#[ignore = "needs helper and the latency-overflow fixture (.ps1 or .sh build scripts)"]
fn a_latency_too_long_for_the_alignment_budget_refuses_host_bypass_but_not_preparation() {
    // The fixture reports a latency of u32::MAX samples, whose delay no chain can reserve.
    let overflow = support::delay_variant(PluginFormat::Vst3, "latency-overflow", 7);
    let mut chain = spawn(&[overflow]);
    prepare(&mut chain);
    let refused = |result: Result<(), Error>| match result.unwrap_err() {
        Error::Operation { slot, failure } => (slot, failure.kind),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        refused(chain.set_host_bypass(0, true)),
        (Some(0), FailureKind::Configuration)
    );
    assert!(!chain.host_bypass(0).unwrap());
    let mut chain = Chain::spawn(
        &support::helper(),
        &[ChainSlot::new(support::delay_variant(
            PluginFormat::Vst3,
            "latency-overflow",
            7,
        ))
        .with_host_bypass(true)],
        &SpawnOptions::new(HostIdentity::default(), Timeouts::default()),
    )
    .unwrap();
    let config = support::stereo_config(&mut chain);
    assert_eq!(
        refused(chain.prepare_audio(&config).map(|_| ())).1,
        FailureKind::Configuration
    );
}
