use super::*;

#[test]
fn malformed_output_events_are_rejected_before_copying_audio() {
    for events in [
        vec![Event::note_on(8, 0, 60, 100)],
        vec![Event::note_on(7, 0, 60, 100), Event::note_off(0, 0, 60, 0)],
        vec![Event::new(0, EventData::NoteOn(Note::new(0, 60, f64::NAN)))],
        vec![Event::new(0, EventData::NoteOn(Note::new(16, 60, 0.5)))],
        vec![Event::new(0, EventData::NoteOn(Note::new(0, 128, 0.5)))],
        vec![Event::new(
            0,
            EventData::NoteOn(Note::new(0, 60, 0.5).with_id(u32::MAX)),
        )],
    ] {
        let (mut owner, mut peer) = pair(SampleFormat::F32);
        let submission = owner
            .write_input_f32(1, 8, &[&[0.0; 8][..]; 2], &[], &[])
            .unwrap();
        peer.write_output_f32(submission, &[&[0.25; 8][..]; 2], &events)
            .unwrap();
        let mut audio = vec![vec![42.0; 8]; 2];
        assert!(
            owner
                .read_output_f32(
                    submission,
                    &mut views(&mut audio),
                    events.len(),
                    crate::MAX_EVENT_PORTS,
                    &mut Vec::with_capacity(MAX_BLOCK_EVENTS)
                )
                .is_err(),
            "{events:?}"
        );
        assert_eq!(audio, vec![vec![42.0; 8]; 2]);
    }
}

#[test]
fn overlapping_sysex_ranges_still_share_one_block_budget() {
    let (mut owner, mut peer) = pair(SampleFormat::F32);
    let submission = owner
        .write_input_f32(1, 8, &[&[0.0; 8][..]; 2], &[], &[])
        .unwrap();
    let mut bytes = vec![0x7D; MAX_BLOCK_SYSEX_BYTES];
    bytes[0] = 0xF0;
    bytes[MAX_BLOCK_SYSEX_BYTES - 1] = 0xF7;
    peer.write_output_f32(submission, &[&[0.25; 8][..]; 2], &[Event::sysex(0, bytes)])
        .unwrap();
    // A corrupted peer repeats the first record, reusing the same valid arena range. The
    // decoded allocation budget must count each occurrence, not only the highest arena offset.
    let start = peer.layout.output_events.slots;
    for offset in (0..EVENT_BYTES).step_by(8) {
        peer.store(start + EVENT_BYTES + offset, peer.load(start + offset));
    }
    let mut audio = vec![vec![42.0; 8]; 2];
    assert!(
        owner
            .read_output_f32(
                submission,
                &mut views(&mut audio),
                2,
                crate::MAX_EVENT_PORTS,
                &mut Vec::with_capacity(MAX_BLOCK_EVENTS)
            )
            .is_err()
    );
    assert_eq!(audio, vec![vec![42.0; 8]; 2]);
}

fn descriptor(format: SampleFormat) -> Descriptor {
    Descriptor {
        generation: 9,
        config: SlotConfig {
            sample_format: format,
            max_frames: 8,
            input_channels: 2,
            output_channels: 2,
        },
    }
}

fn pair(format: SampleFormat) -> (SharedAudio, SharedAudio) {
    let owner = SharedAudio::new(descriptor(format)).unwrap();
    let peer = peer(&owner, owner.descriptor());
    (owner, peer)
}

fn peer(owner: &SharedAudio, descriptor: Descriptor) -> SharedAudio {
    let handle = owner.memory.handle().try_clone().unwrap();
    // SAFETY: the owner's own shared memory, for its descriptor.
    unsafe { SharedAudio::open(handle, descriptor) }.unwrap()
}

fn automation() -> Vec<AutomationEvent> {
    vec![0.25, 0.75]
        .into_iter()
        .map(|value| AutomationEvent {
            slot: 1,
            change: crate::ParameterChange {
                id: u64::MAX,
                offset: 0,
                value,
            },
        })
        .collect()
}

fn views<T>(channels: &mut [Vec<T>]) -> Vec<&mut [T]> {
    channels.iter_mut().map(Vec::as_mut_slice).collect()
}

#[test]
fn fixed_slots_preserve_precision_event_order_and_variable_frames() {
    let (mut owner, mut peer) = pair(SampleFormat::F64);
    let mut audio = vec![vec![0.0; 8]; 2];
    let mut returned = audio.clone();
    let mut edits = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut notes = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let expected_edits = automation();
    let note = Note::new(15, 127, 1.0 / 3.0);
    let expression = NoteExpression::new(3, 64, ExpressionKind::Tuning, -0.1);
    let expected_notes = [
        Event::note_on(0, 0, 64, 100),
        Event::sysex(0, vec![0xF0, 0x7D, 0x12, 0x34, 0xF7]).on_port(3),
        Event::new(0, EventData::NoteOn(note.with_id(crate::MAX_NOTE_ID))).on_port(2),
        Event::new(0, EventData::Expression(expression.with_id(0))),
        Event::new(0, EventData::Expression(expression)).on_port(15),
        Event::note_off(0, 0, 64, 32).on_port(1),
        Event::new(0, EventData::NoteOff(note)),
        Event::sysex(0, vec![0xF0, 0xF7]),
    ];
    let mut produced = Vec::with_capacity(MAX_BLOCK_EVENTS);
    for (index, frames) in [8, 3, 0, 1, 8].into_iter().enumerate() {
        let input = vec![1.0 + f64::EPSILON; frames];
        let a = if frames == 0 {
            &[][..]
        } else {
            &expected_edits
        };
        let m = if frames == 0 {
            &[][..]
        } else {
            &expected_notes
        };
        let submission = owner
            .write_input_f64(index as u64 + 1, frames, &[&input, &input], a, m)
            .unwrap();
        peer.read_input_f64(submission, &mut audio, &mut edits, &mut notes)
            .unwrap();
        assert_eq!(audio, [input.clone(), input.clone()]);
        assert_eq!(edits, a);
        assert_eq!(notes, m);
        for channel in &mut audio {
            for sample in channel {
                *sample = -*sample;
            }
        }
        // The output events are the input events in reverse, echoed back.
        let echoed: Vec<Event> = notes.iter().rev().cloned().collect();
        peer.write_output_f64(submission, &[&audio[0], &audio[1]], &echoed)
            .unwrap();
        returned
            .iter_mut()
            .for_each(|channel| channel.resize(frames, 0.0));
        owner
            .read_output_f64(
                submission,
                &mut views(&mut returned),
                echoed.len(),
                crate::MAX_EVENT_PORTS,
                &mut produced,
            )
            .unwrap();
        assert_eq!(returned, audio);
        assert_eq!(produced, echoed);
    }
}

#[test]
fn submissions_outside_the_generation_or_slot_bounds_are_rejected() {
    let (mut owner, mut peer) = pair(SampleFormat::F32);
    let mut audio = vec![vec![0.0; 8]; 2];
    let mut edits = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut notes = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let submission = owner
        .write_input_f32(1, 1, &[&[0.25], &[0.5]], &automation(), &[])
        .unwrap();
    for bad in [
        Submission {
            generation: 8,
            ..submission
        },
        Submission {
            frames: 9,
            ..submission
        },
        Submission {
            automation: usize::MAX,
            ..submission
        },
    ] {
        assert!(
            peer.read_input_f32(bad, &mut audio, &mut edits, &mut notes)
                .is_err()
        );
    }
    peer.read_input_f32(submission, &mut audio, &mut edits, &mut notes)
        .unwrap();
    peer.write_output_f32(submission, &[&audio[0], &audio[1]], &[])
        .unwrap();
    for bad in [
        Submission {
            generation: 8,
            ..submission
        },
        Submission {
            frames: 9,
            ..submission
        },
    ] {
        assert!(
            owner
                .read_output_f32(
                    bad,
                    &mut views(&mut audio),
                    0,
                    crate::MAX_EVENT_PORTS,
                    &mut Vec::new()
                )
                .is_err()
        );
    }
    owner
        .read_output_f32(
            submission,
            &mut views(&mut audio),
            0,
            crate::MAX_EVENT_PORTS,
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(audio, [vec![0.25], vec![0.5]]);
}

#[test]
fn invalid_shape_precision_and_event_budget_do_not_replace_valid_input() {
    let (mut owner, mut peer) = pair(SampleFormat::F32);
    let submission = owner
        .write_input_f32(1, 1, &[&[0.25], &[0.5]], &[], &[])
        .unwrap();
    assert!(
        owner
            .write_input_f64(2, 1, &[&[1.0], &[1.0]], &[], &[])
            .is_err()
    );
    assert!(
        owner
            .write_input_f32(2, 2, &[&[1.0], &[1.0]], &[], &[])
            .is_err()
    );
    let too_many = vec![Event::note_on(0, 0, 60, 127); MAX_BLOCK_EVENTS];
    assert!(
        owner
            .write_input_f32(2, 1, &[&[1.0], &[1.0]], &automation(), &too_many)
            .is_err()
    );
    let mut audio = vec![vec![0.0; 8]; 2];
    peer.read_input_f32(submission, &mut audio, &mut Vec::new(), &mut Vec::new())
        .unwrap();
    assert_eq!(audio, [vec![0.25], vec![0.5]]);
    for config in [
        SlotConfig {
            max_frames: 0,
            ..owner.descriptor.config
        },
        SlotConfig {
            input_channels: usize::MAX,
            ..owner.descriptor.config
        },
        SlotConfig {
            max_frames: i32::MAX as usize,
            ..owner.descriptor.config
        },
    ] {
        assert!(
            SharedAudio::new(Descriptor {
                generation: 1,
                config
            })
            .is_err()
        );
    }
}

#[test]
fn insufficient_scratch_is_rejected_before_dsp() {
    let (mut owner, mut peer) = pair(SampleFormat::F32);
    let submission = owner
        .write_input_f32(
            1,
            1,
            &[&[1.0], &[1.0]],
            &[],
            &[Event::note_on(0, 0, 60, 127)],
        )
        .unwrap();
    let mut audio = vec![vec![0.0; 8]; 2];
    let mut edits = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut notes = Vec::with_capacity(MAX_BLOCK_EVENTS);
    assert!(
        peer.read_input_f32(submission, &mut [], &mut edits, &mut notes)
            .is_err()
    );
    assert!(
        peer.read_input_f32(submission, &mut audio, &mut edits, &mut Vec::new())
            .is_err()
    );
    peer.read_input_f32(submission, &mut audio, &mut edits, &mut notes)
        .unwrap();
    assert_eq!(notes, [Event::note_on(0, 0, 60, 127)]);
}

#[test]
fn anonymous_mapping_survives_peer_drop() {
    let (mut owner, mut peer) = pair(SampleFormat::F32);
    let submission = owner
        .write_input_f32(1, 1, &[&[0.25], &[0.5]], &[], &[])
        .unwrap();
    let mut audio = vec![vec![0.0; 8]; 2];
    peer.read_input_f32(submission, &mut audio, &mut Vec::new(), &mut Vec::new())
        .unwrap();
    peer.write_output_f32(submission, &[&audio[0], &audio[1]], &[])
        .unwrap();
    drop(peer);
    owner
        .read_output_f32(
            submission,
            &mut views(&mut audio),
            0,
            crate::MAX_EVENT_PORTS,
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(audio, [vec![0.25], vec![0.5]]);
}

#[test]
fn zero_audio_input_carries_the_full_event_budget_and_preserves_float_bits() {
    let descriptor = Descriptor {
        generation: 1,
        config: SlotConfig {
            input_channels: 0,
            output_channels: 1,
            ..descriptor(SampleFormat::F32).config
        },
    };
    let mut owner = SharedAudio::new(descriptor).unwrap();
    let mut peer = peer(&owner, descriptor);
    let notes = vec![Event::note_off(0, 0, 60, 0); MAX_BLOCK_EVENTS - 1];
    let changes = automation();
    let submission = owner
        .write_input_f32(1, 2, &[], &changes[..1], &notes)
        .unwrap();
    let mut edits = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut received = Vec::with_capacity(MAX_BLOCK_EVENTS);
    peer.read_input_f32(submission, &mut [], &mut edits, &mut received)
        .unwrap();
    assert_eq!(edits, changes[..1]);
    assert_eq!(received, notes);
    let bits = [0xffc00123, 0x80000000];
    let output = bits.map(f32::from_bits);
    peer.write_output_f32(submission, &[&output], &[]).unwrap();
    let mut result = vec![vec![0.0; 2]];
    owner
        .read_output_f32(
            submission,
            &mut views(&mut result),
            0,
            crate::MAX_EVENT_PORTS,
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(
        result[0]
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>(),
        bits
    );
}

/// A mapped file that a peer truncates kills the process that reads the mapping.
#[cfg(unix)]
#[test]
fn a_peer_cannot_shrink_the_shared_memory() {
    let (mut owner, mut peer) = pair(SampleFormat::F32);
    let handle = owner.memory.handle().try_clone().unwrap();
    assert!(std::fs::File::from(handle).set_len(0).is_err());
    let submission = owner
        .write_input_f32(1, 8, &[&[0.25; 8], &[0.5; 8]], &[], &[])
        .unwrap();
    let mut audio = vec![vec![0.0; 8]; 2];
    peer.read_input_f32(submission, &mut audio, &mut Vec::new(), &mut Vec::new())
        .unwrap();
    assert_eq!(audio, [vec![0.25; 8], vec![0.5; 8]]);
}
