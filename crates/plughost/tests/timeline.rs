use plughost::*;
use plughost_core::render::Process;
use plughost_core::{ProcessConfig, SampleFormat};
use plughost_formats::{Error as NativeError, HostedPlugin};

mod support;

use support::{delay as reference, spawn};

fn options() -> RenderOptions {
    RenderOptions {
        tail: TailPolicy::Reported,
        max_tail_seconds: 0.0,
    }
}
fn chain(format: PluginFormat, block: usize) -> Chain {
    layout_chain(format, block, Layout::Stereo)
}
fn layout_chain(format: PluginFormat, block: usize, layout: Layout) -> Chain {
    let mut chain = spawn(&[reference(format)]);
    let config = chain
        .main_bus_config(48_000.0, block, layout, &[layout])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    chain
}
struct Native {
    plugin: Box<dyn HostedPlugin>,
    config: ProcessConfig,
}
impl Native {
    fn new(format: PluginFormat, block: usize) -> Self {
        let mut plugin =
            plughost_formats::load(&reference(format), &HostIdentity::default()).unwrap();
        let config = ProcessConfig {
            sample_rate: 48_000.0,
            max_block_size: block,
            sample_format: SampleFormat::F32,
            input: Layout::Stereo,
            output: Layout::Stereo,
            mode: ProcessMode::Offline,
        };
        plugin.prepare(&config).unwrap();
        Self { plugin, config }
    }
}
impl Process<f32> for Native {
    type Error = NativeError;
    fn sample_rate(&self) -> f64 {
        self.config.sample_rate
    }
    fn max_block_size(&self) -> usize {
        self.config.max_block_size
    }
    fn input_channels(&self) -> usize {
        self.config.input.channels()
    }
    fn output_channels(&self) -> usize {
        self.config.output.channels()
    }
    fn latency(&self) -> Result<u32, NativeError> {
        self.plugin.latency()
    }
    fn tail(&self) -> Result<Tail, NativeError> {
        self.plugin.tail()
    }
    fn validate_automation(&mut self, events: &[AutomationEvent]) -> Result<(), NativeError> {
        let parameters = self.plugin.parameters();
        for event in events {
            if event.slot != 0 {
                return Err(NativeError::Input(InputError::Slot));
            }
            parameters
                .iter()
                .find(|(p, _)| p.id == event.change.id)
                .ok_or(NativeError::Input(InputError::UnknownParameter {
                    id: event.change.id,
                }))?
                .0
                .automation_value(event.change.value)
                .map_err(NativeError::Input)?;
        }
        Ok(())
    }
    fn process(
        &mut self,
        context: &BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[AutomationEvent],
        midi: &[Event],
        produced: &mut Vec<Event>,
    ) -> Result<(), NativeError> {
        let changes: Vec<_> = automation.iter().map(|point| point.change).collect();
        self.plugin
            .processor()
            .process_audio_f32(context, input, output, &changes, midi, produced)
    }
}
fn time(sample: i64, beat: f64, tempo: f64, playing: bool) -> Transport {
    Transport {
        sample_position: sample,
        beat_position: Some(beat),
        bar_position: Some(BarPosition {
            start: (beat / 4.0).floor() * 4.0,
            number: (beat / 4.0).floor() as i32,
        }),
        tempo: Some(tempo),
        time_signature: Some(TimeSignature {
            numerator: 4,
            denominator: 4,
        }),
        playing,
        loop_region: None,
    }
}
fn near(actual: &[Vec<f32>], expected: &[Vec<f32>]) {
    assert_eq!(actual.len(), expected.len());
    for (a, b) in actual.iter().zip(expected) {
        assert_eq!(a.len(), b.len());
        for (index, (a, b)) in a.iter().zip(b).enumerate() {
            assert!((a - b).abs() < 2e-5, "sample {index}: {a} != {b}");
        }
    }
}

#[test]
#[ignore = "needs helper and test plugins"]
fn native_and_helper_transport_automation_and_audio_less_clock_match() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let probe = if format == PluginFormat::Vst3 { 4 } else { 7 };
        let clock = if format == PluginFormat::Vst3 { 5 } else { 8 };
        let mut stopped = time(317, 2.103, 18_000.0, false);
        stopped.time_signature = Some(TimeSignature {
            numerator: 3,
            denominator: 4,
        });
        let mut looped = time(-137, 1.103, 9_000.0, true);
        looped.loop_region = Some(LoopRegion {
            start: 1.103,
            end: 2.103,
            sample_start: -137,
            sample_end: 183,
        });
        let timeline = [
            TransportChange {
                offset: 0,
                transport: Some(time(0, 0.103, 18_000.0, true)),
            },
            TransportChange {
                offset: 317,
                transport: Some(stopped),
            },
            TransportChange {
                offset: 733,
                transport: Some(looped),
            },
            TransportChange {
                offset: 1053,
                transport: Some(looped),
            },
        ];
        let points = [AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: 0,
                offset: 813,
                value: 0.5,
            },
        }];
        let ramps = [AutomationRamp {
            slot: 0,
            id: 0,
            start: 997,
            end: 1013,
            from: 0.5,
            to: 1.0,
        }];
        let schedule = RenderSchedule {
            transport: &timeline,
            automation: &points,
            ramps: &ramps,
        };
        let input = vec![0.0; 1300];
        let mut previous: Option<Vec<Vec<f32>>> = None;
        for block in [127, 512] {
            let mut native = Native::new(format, block);
            native.plugin.set_parameter(probe, 1.0).unwrap();
            let expected = render_with_schedule(
                &mut native,
                &[&input, &input],
                input.len(),
                &[],
                &options(),
                &schedule,
            )
            .unwrap();
            let mut isolated = chain(format, block);
            isolated.set_parameter(0, probe, 1.0).unwrap();
            let actual = render_with_schedule(
                &mut isolated,
                &[&input, &input],
                input.len(),
                &[],
                &options(),
                &schedule,
            )
            .unwrap();
            near(&actual.channels, &expected.channels);
            if let Some(previous) = &previous {
                near(&actual.channels, previous);
            }
            // Audible beat output, and an independently encoded seekable vs steady clock.
            assert_eq!(actual.channels[0][0], 1.0);
            assert!(actual.channels[0].contains(&0.0));
            assert!((actual.channels[1][0] - 0.0).abs() < 1e-5);
            // Gain at raw sample 813 changes output from aligned sample 333 onward.
            assert!(
                (actual.channels[1][318] - (317.0 / 10_000.0 + 318.0 / 100_000_000.0)).abs() < 2e-5
            );
            previous = Some(actual.channels);
            let changes = isolated.take_changes().unwrap();
            assert_eq!(changes.snapshot[0].unwrap().latency, 480);
            assert!(isolated.take_changes().unwrap().changes.is_empty());
        }
        // Neither a main input nor output is needed to advance the prepared processing clock.
        let mut isolated = layout_chain(format, 512, Layout::None);
        for _ in 0..3 {
            isolated
                .process_audio_f32(
                    &BlockContext::new(137),
                    &[],
                    &mut [],
                    &[],
                    &[],
                    &mut Vec::new(),
                )
                .unwrap();
        }
        let elapsed = isolated
            .parameters(0)
            .unwrap()
            .into_iter()
            .find(|(p, _)| p.id == clock)
            .unwrap()
            .1;
        assert!((elapsed * 1_000_000.0 - 411.0).abs() < 1e-6);
    }
}

#[test]
#[ignore = "needs helper and test plugins"]
fn segmented_latency_alignment_and_cancel_recovery_match_whole_render() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let source: Vec<_> = (0..1500).map(|index| (index % 31) as f32 / 31.0).collect();
        let mut whole = chain(format, 127);
        let expected = render(
            &mut whole,
            &[&source, &source],
            source.len(),
            &[],
            &options(),
        )
        .unwrap();
        let mut segmented = chain(format, 127);
        let mut session = RenderSession::new(&mut segmented).unwrap();
        let mut channels = vec![vec![], vec![]];
        let mut consume = |block: Delivery<'_, f32>| {
            for (out, samples) in channels.iter_mut().zip(block.audio) {
                out.extend_from_slice(samples);
            }
        };
        for segment in source.chunks(113) {
            session
                .push(
                    RenderInput {
                        audio: &[segment, segment],
                        frames: segment.len(),
                        events: &[],
                        schedule: RenderSchedule::default(),
                    },
                    &mut consume,
                    || false,
                )
                .unwrap();
        }
        session.finish(&options(), &mut consume, || false).unwrap();
        assert_eq!(session.state(), SessionState::Finished);
        near(&channels, &expected.channels);
        segmented.reset().unwrap();
        let blocks = std::cell::Cell::new(0);
        let result = render_stream(
            &mut segmented,
            RenderInput {
                audio: &[&source, &source],
                frames: source.len(),
                events: &[],
                schedule: RenderSchedule::default(),
            },
            &options(),
            |_| blocks.set(blocks.get() + 1),
            || blocks.get() == 2,
        )
        .unwrap();
        assert_eq!(result.status, RenderStatus::Cancelled);
        assert!(result.processed_frames < source.len());
        segmented.reset().unwrap();
        let recovered = render(
            &mut segmented,
            &[&source, &source],
            source.len(),
            &[],
            &options(),
        )
        .unwrap();
        near(&recovered.channels, &expected.channels);
    }
}

#[test]
#[ignore = "needs helper and test plugins"]
fn slot_automation_rejection_does_not_advance_earlier_plugins() {
    let plugins = [reference(PluginFormat::Vst3), reference(PluginFormat::Clap)];
    let mut chain = spawn(&plugins);
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::Stereo, &[Layout::Stereo; 2])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    let source = [1.0; 512];
    let (mut left, mut right) = ([0.0; 512], [0.0; 512]);
    let invalid = [AutomationEvent {
        slot: 1,
        change: ParameterChange {
            id: 100,
            offset: 0,
            value: 0.5,
        },
    }];
    let error = chain
        .process_audio_f32(
            &BlockContext::new(512),
            &[&source, &source],
            &mut [&mut left, &mut right],
            &invalid,
            &[],
            &mut Vec::new(),
        )
        .unwrap_err();
    assert_eq!(error.kind(), FailureKind::InvalidInput);
    assert_eq!(
        chain
            .parameters(0)
            .unwrap()
            .into_iter()
            .find(|(p, _)| p.id == 5)
            .unwrap()
            .1,
        0.0
    );
    let points = [AutomationEvent {
        slot: 1,
        change: ParameterChange {
            id: 0,
            offset: 1000,
            value: 0.5,
        },
    }];
    let source = vec![1.0f32; 1200];
    let rendered = render_with_schedule(
        &mut chain,
        &[&source, &source],
        source.len(),
        &[],
        &options(),
        &RenderSchedule {
            automation: &points,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rendered.latency, 960);
    assert!(rendered.channels[0][..40].iter().all(|v| *v == 1.0));
    assert!(rendered.channels[0][40..].iter().all(|v| *v == 0.5));
}

#[test]
#[ignore = "needs helper and test plugins"]
fn timing_callbacks_coalesce_and_restart_requests_never_loop_automatically() {
    let mut chain = chain(PluginFormat::Clap, 512);
    let input = [0.0; 32];
    let (mut left, mut right) = ([0.0; 32], [0.0; 32]);
    chain.take_changes().unwrap();
    // More changes than queue capacity, without consuming. They coalesce into this slot's
    // latest timing; none of these requests can be blocked by an event consumer.
    for index in 0..130 {
        let automation = [AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: 9,
                offset: 0,
                value: (index % 33) as f64 / 32.0,
            },
        }];
        chain
            .process_audio_f32(
                &BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &automation,
                &[],
                &mut Vec::new(),
            )
            .unwrap();
    }
    let changes = chain.take_changes().unwrap();
    assert_eq!(changes.changes.len(), 1);
    assert!(!changes.resync_required);
    assert_eq!(changes.snapshot[0].unwrap().tail, Tail::Samples(30));
    assert_eq!(chain.tail(), Tail::Samples(30));
    let automation = [AutomationEvent {
        slot: 0,
        change: ParameterChange {
            id: 9,
            offset: 0,
            value: 0.5,
        },
    }];
    // The reported tail sets this render's length, so its change invalidates the plan.
    let error = render_with_schedule(
        &mut chain,
        &[&input, &input],
        32,
        &[],
        &RenderOptions {
            max_tail_seconds: 1.0,
            ..options()
        },
        &RenderSchedule {
            automation: &automation,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind(), FailureKind::Configuration);
    assert_eq!(
        chain.take_changes().unwrap().snapshot[0].unwrap().tail,
        Tail::Samples(16)
    );
    chain.set_parameter(0, 0, 0.375).unwrap();
    for mode in [0.5, 1.0] {
        let automation = [AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: 10,
                offset: 0,
                value: mode,
            },
        }];
        assert_eq!(
            chain
                .process_audio_f32(
                    &BlockContext::new(32),
                    &[&input, &input],
                    &mut [&mut left, &mut right],
                    &automation,
                    &[],
                    &mut Vec::new(),
                )
                .unwrap_err()
                .kind(),
            FailureKind::RestartRequired
        );
        assert!(
            chain.take_changes().unwrap().snapshot[0]
                .unwrap()
                .restart_required
        );
        // A pending restart must never cause the helper to reconfigure on its own.
        assert_eq!(
            chain
                .process_audio_f32(
                    &BlockContext::new(32),
                    &[&input, &input],
                    &mut [&mut left, &mut right],
                    &[],
                    &[],
                    &mut Vec::new(),
                )
                .unwrap_err()
                .kind(),
            FailureKind::RestartRequired
        );
        assert_eq!(
            chain
                .parameters(0)
                .unwrap()
                .iter()
                .find(|(info, _)| info.id == 10)
                .unwrap()
                .1,
            mode
        );
        chain.reprepare().unwrap();
        let snapshot = chain.take_changes().unwrap().snapshot[0].unwrap();
        // Atomic reprepare restores serialized plugin state into a candidate. This fixture saves
        // gain, but deliberately excludes its runtime-only restart/tail probes from that state.
        assert_eq!(snapshot.latency, 480);
        assert!(!snapshot.restart_required);
        let parameters = chain.parameters(0).unwrap();
        assert_eq!(
            parameters.iter().find(|(info, _)| info.id == 0).unwrap().1,
            0.375
        );
        assert_eq!(
            parameters.iter().find(|(info, _)| info.id == 10).unwrap().1,
            0.0
        );
        chain
            .process_audio_f32(
                &BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
    }
    chain.reset().unwrap();
    assert!(
        !chain.take_changes().unwrap().snapshot[0]
            .unwrap()
            .restart_required
    );
    chain
        .process_audio_f32(
            &BlockContext::new(32),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
}

#[test]
#[ignore = "needs helper and test plugins"]
fn absent_time_stays_absent_and_discrete_automation_is_quantized() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let input = [1.0; 512];
        let point = AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: 0,
                offset: 490,
                value: 0.5,
            },
        };
        let mut native = Native::new(format, 512);
        let mut block_chain = chain(format, 512);
        let mut direct = vec![vec![0.0; 512]; 2];
        let mut isolated = direct.clone();
        native
            .process(
                &BlockContext::new(512),
                &[&input, &input],
                &mut direct.iter_mut().map(Vec::as_mut_slice).collect::<Vec<_>>(),
                &[point],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        block_chain
            .process_audio_f32(
                &BlockContext::new(512),
                &[&input, &input],
                &mut isolated
                    .iter_mut()
                    .map(Vec::as_mut_slice)
                    .collect::<Vec<_>>(),
                &[point],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(direct, isolated);
        assert!(direct[0][..480].iter().all(|sample| *sample == 0.0));
        assert!(direct[0][480..490].iter().all(|sample| *sample == 1.0));
        assert!(direct[0][490..].iter().all(|sample| *sample == 0.5));
        let probe = if format == PluginFormat::Vst3 { 4 } else { 7 };
        let mut chain = chain(format, 512);
        let source = vec![0.25f32; 600];
        let automation = [AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: probe,
                offset: 0,
                value: 0.51,
            },
        }];
        let output = render_with_schedule(
            &mut chain,
            &[&source, &source],
            source.len(),
            &[],
            &options(),
            &RenderSchedule {
                automation: &automation,
                ..Default::default()
            },
        )
        .unwrap();
        // Without a transport, VST3 still gets a process context, one without musical time.
        let no_musical_time = if format == PluginFormat::Vst3 {
            -2.0
        } else {
            -1.0
        };
        assert!(
            output
                .channels
                .iter()
                .flatten()
                .all(|sample| *sample == no_musical_time)
        );
        assert_eq!(
            chain
                .parameters(0)
                .unwrap()
                .iter()
                .find(|(p, _)| p.id == probe)
                .unwrap()
                .1,
            1.0
        );
        // Quantized back to zero, the ordinary delay reads the actual audio input again.
        let automation = [AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: probe,
                offset: 0,
                value: 0.49,
            },
        }];
        let output = render_with_schedule(
            &mut chain,
            &[&source, &source],
            source.len(),
            &[],
            &options(),
            &RenderSchedule {
                automation: &automation,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            output
                .channels
                .iter()
                .flatten()
                .all(|sample| *sample == 0.25)
        );
    }
}
