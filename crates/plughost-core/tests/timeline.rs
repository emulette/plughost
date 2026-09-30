use plughost_core::render::*;
use plughost_core::{
    AutomationEvent, BlockContext, InputError, MidiEvent, ParameterChange, RenderError, Transport,
};
use std::cell::Cell;

struct Instrument {
    block: usize,
    gain: f64,
    processed: usize,
    contexts: Vec<BlockContext>,
    midi: Vec<MidiEvent>,
}
impl Instrument {
    fn new(block: usize) -> Self {
        Self {
            block,
            gain: 1.0,
            processed: 0,
            contexts: vec![],
            midi: vec![],
        }
    }
}
impl Process<f32> for Instrument {
    type Error = RenderError;
    fn sample_rate(&self) -> f64 {
        100.0
    }
    fn max_block_size(&self) -> usize {
        self.block
    }
    fn input_channels(&self) -> usize {
        0
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn latency(&self) -> Result<u32, RenderError> {
        Ok(0)
    }
    fn tail(&self) -> Result<Tail, RenderError> {
        Ok(Tail::Samples(0))
    }
    fn validate_automation(&mut self, events: &[AutomationEvent]) -> Result<(), RenderError> {
        if events.iter().any(|e| e.slot != 0 || e.change.id != 0) {
            return Err(RenderError::Input(InputError::Automation));
        }
        Ok(())
    }
    fn process(
        &mut self,
        context: &BlockContext,
        _: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[AutomationEvent],
        midi: &[MidiEvent],
        _produced: &mut Vec<MidiEvent>,
    ) -> Result<(), RenderError> {
        self.contexts.push(*context);
        plughost_core::validate_event_count(automation.len(), midi.len())
            .map_err(RenderError::Input)?;
        self.midi.extend(midi.iter().map(|event| MidiEvent {
            offset: event.offset + self.processed,
            ..event.clone()
        }));
        let mut points = automation.iter().peekable();
        for (offset, sample) in output[0].iter_mut().enumerate() {
            while points
                .peek()
                .is_some_and(|point| point.change.offset == offset)
            {
                self.gain = points.next().unwrap().change.value;
            }
            *sample = context.transport.map_or(self.gain as f32, |time| {
                time.advanced(offset, 100.0)
                    .unwrap()
                    .beat_position
                    .unwrap_or(-1.0) as f32
                    * self.gain as f32
            });
        }
        self.processed += context.frames;
        Ok(())
    }
}
fn options() -> RenderOptions {
    RenderOptions {
        tail: TailPolicy::Reported,
        max_tail_seconds: 0.0,
    }
}
fn transport(beat: f64, tempo: f64, playing: bool) -> Transport {
    Transport {
        sample_position: 0,
        beat_position: Some(beat),
        bar_position: None,
        tempo: Some(tempo),
        playing,
        time_signature: None,
        loop_region: None,
    }
}

#[test]
fn tempo_stop_seek_points_and_ramps_are_block_size_independent() {
    let timeline = [
        TransportChange {
            offset: 0,
            transport: Some(transport(0.0, 60.0, true)),
        },
        TransportChange {
            offset: 5,
            transport: Some(transport(0.05, 120.0, true)),
        },
        TransportChange {
            offset: 12,
            transport: Some(transport(0.19, 120.0, false)),
        },
        TransportChange {
            offset: 17,
            transport: Some(transport(-2.0, 60.0, true)),
        },
    ];
    let points = [AutomationEvent {
        slot: 0,
        change: ParameterChange {
            id: 0,
            offset: 8,
            value: 0.5,
        },
    }];
    let ramps = [AutomationRamp {
        slot: 0,
        id: 0,
        start: 19,
        end: 23,
        from: 0.5,
        to: 1.0,
    }];
    let schedule = RenderSchedule {
        transport: &timeline,
        automation: &points,
        ramps: &ramps,
    };
    let mut reference: Option<Vec<f32>> = None;
    for block in [1, 7, 16, 64] {
        let mut processor = Instrument::new(block);
        let output =
            render_with_schedule(&mut processor, &[], 30, &[], &options(), &schedule).unwrap();
        if let Some(expected) = &reference {
            for (a, b) in output.channels[0].iter().zip(expected) {
                assert!((a - b).abs() < 1e-6);
            }
        } else {
            reference = Some(output.channels[0].clone());
        }
        assert_eq!(output.channels[0][12], output.channels[0][16]);
        assert_eq!(output.channels[0][17], -1.0);
        assert!(
            processor
                .contexts
                .iter()
                .all(|context| context.frames <= block)
        );
    }
}

#[test]
fn malformed_schedule_is_rejected_before_processing() {
    let mut processor = Instrument::new(16);
    let automation = [AutomationEvent {
        slot: 1,
        change: ParameterChange {
            id: 0,
            offset: 29,
            value: 1.0,
        },
    }];
    let schedule = RenderSchedule {
        automation: &automation,
        ..Default::default()
    };
    assert!(render_with_schedule(&mut processor, &[], 30, &[], &options(), &schedule).is_err());
    assert_eq!(processor.processed, 0);
    let timeline = [TransportChange {
        offset: 0,
        transport: Some(transport(0.0, f64::NAN, true)),
    }];
    assert!(
        render_with_schedule(
            &mut processor,
            &[],
            30,
            &[],
            &options(),
            &RenderSchedule {
                transport: &timeline,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert_eq!(processor.processed, 0);
}

#[test]
fn streaming_is_bounded_and_cancellation_stops_at_a_block_boundary() {
    let mut processor = Instrument::new(17);
    let mut frames = 0;
    let mut largest = 0;
    let result = render_stream(
        &mut processor,
        RenderInput {
            audio: &[],
            frames: 100_000,
            events: &[],
            schedule: RenderSchedule::default(),
        },
        &options(),
        |block| {
            largest = largest.max(block.audio[0].len());
            frames += block.audio[0].len();
        },
        || false,
    )
    .unwrap();
    assert_eq!(frames, 100_000);
    assert_eq!(largest, 17);
    assert_eq!(result.output_frames, frames);
    let blocks = Cell::new(0);
    let result = render_stream(
        &mut processor,
        RenderInput {
            audio: &[],
            frames: 10_000,
            events: &[],
            schedule: RenderSchedule::default(),
        },
        &options(),
        |_| blocks.set(blocks.get() + 1),
        || blocks.get() == 5,
    )
    .unwrap();
    assert_eq!(result.status, RenderStatus::Cancelled);
    assert_eq!(result.processed_frames, 85);
    assert_eq!(result.output_frames, 85);

    // A ramp is submitted block by block too, so cancelling stops before the next block.
    let mut processor = Instrument::new(64);
    let ramps = [AutomationRamp {
        slot: 0,
        id: 0,
        start: 0,
        end: 1023,
        from: 0.0,
        to: 1.0,
    }];
    let delivered = Cell::new(0);
    let result = render_stream(
        &mut processor,
        RenderInput {
            audio: &[],
            frames: 1024,
            events: &[],
            schedule: RenderSchedule {
                ramps: &ramps,
                ..Default::default()
            },
        },
        &options(),
        |block| delivered.set(delivered.get() + block.audio[0].len()),
        || delivered.get() > 0,
    )
    .unwrap();
    assert_eq!(result.status, RenderStatus::Cancelled);
    assert_eq!(result.processed_frames, 64);
    assert_eq!(delivered.get(), 64);
}

#[test]
fn session_continues_transport_and_rejects_calls_after_finish_or_cancel() {
    let mut processor = Instrument::new(7);
    let mut session = RenderSession::new(&mut processor).unwrap();
    let timeline = [TransportChange {
        offset: 0,
        transport: Some(transport(0.0, 60.0, true)),
    }];
    let mut output = vec![];
    for index in 0..3 {
        session
            .push(
                RenderInput {
                    audio: &[],
                    frames: 10,
                    events: &[],
                    schedule: RenderSchedule {
                        transport: if index == 0 { &timeline } else { &[] },
                        ..Default::default()
                    },
                },
                |block| output.extend_from_slice(block.audio[0]),
                || false,
            )
            .unwrap();
    }
    session
        .finish(&options(), |_| panic!("no tail"), || false)
        .unwrap();
    assert_eq!(session.state(), SessionState::Finished);
    assert!(session.finish(&options(), |_| {}, || false).is_err());
    for (index, sample) in output.iter().enumerate() {
        assert!((sample - index as f32 / 100.0).abs() < 1e-6);
    }
    let mut session = RenderSession::new(&mut processor).unwrap();
    session
        .push(
            RenderInput {
                audio: &[],
                frames: 10,
                events: &[],
                schedule: RenderSchedule::default(),
            },
            |_| panic!("cancel before first block"),
            || true,
        )
        .unwrap();
    assert_eq!(session.state(), SessionState::Cancelled);
    assert!(session.finish(&options(), |_| {}, || false).is_err());
}

#[test]
fn long_ramp_and_midi_are_batched_without_splitting_same_sample_events() {
    let mut processor = Instrument::new(8192);
    let ramps = [AutomationRamp {
        slot: 0,
        id: 0,
        start: 0,
        end: 8191,
        from: 0.0,
        to: 1.0,
    }];
    let midi = [
        MidiEvent::note_on(4095, 0, 60, 100),
        MidiEvent::note_off(4095, 0, 60, 0),
    ];
    let output = render_with_schedule(
        &mut processor,
        &[],
        8192,
        &midi,
        &options(),
        &RenderSchedule {
            ramps: &ramps,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        processor
            .contexts
            .iter()
            .map(|c| c.frames)
            .collect::<Vec<_>>(),
        [4095, 4094, 3]
    );
    assert_eq!(processor.midi, midi);
    for (index, sample) in output.channels[0].iter().enumerate() {
        assert_eq!(*sample, (index as f64 / 8191.0) as f32);
    }
}

#[test]
fn points_hold_values_and_preserve_duplicate_order_inside_one_block() {
    let mut processor = Instrument::new(16);
    let points: Vec<_> = [(3, 0.25), (3, 0.75), (5, 0.5)]
        .into_iter()
        .map(|(offset, value)| AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: 0,
                offset,
                value,
            },
        })
        .collect();
    let output = render_with_schedule(
        &mut processor,
        &[],
        8,
        &[],
        &options(),
        &RenderSchedule {
            automation: &points,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        output.channels[0],
        [1.0, 1.0, 1.0, 0.75, 0.75, 0.5, 0.5, 0.5]
    );
    assert_eq!(processor.contexts.len(), 1);
}

#[test]
fn excessive_same_sample_events_are_rejected_before_any_audio_advances() {
    let mut processor = Instrument::new(8);
    let ramps = [AutomationRamp {
        slot: 0,
        id: 0,
        start: 10,
        end: 20,
        from: 0.0,
        to: 1.0,
    }];
    let midi = vec![MidiEvent::note_off(12, 0, 60, 0); plughost_core::MAX_BLOCK_EVENTS];
    let error = render_with_schedule(
        &mut processor,
        &[],
        24,
        &midi,
        &options(),
        &RenderSchedule {
            ramps: &ramps,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error, RenderError::Input(InputError::EventCapacity));
    assert_eq!(processor.processed, 0);
}
