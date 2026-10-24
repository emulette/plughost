use super::*;
use crate::{Event, MAX_BLOCK_EVENTS, SampleFormat};

fn descriptor(lanes: usize) -> RingDescriptor {
    RingDescriptor {
        control: 7,
        first_lane: 100,
        lanes,
        config: SlotConfig {
            sample_format: SampleFormat::F32,
            max_frames: 8,
            input_channels: 2,
            output_channels: 2,
        },
    }
}

/// The application's ring and the worker's view of the same mappings.
fn pair(lanes: usize) -> (Ring, Ring) {
    let owner = Ring::new(descriptor(lanes)).unwrap();
    let control = owner.control_memory().handle().try_clone().unwrap();
    let handles = owner
        .lane_audio()
        .iter()
        .map(|lane| lane.memory.handle().try_clone().unwrap())
        .collect();
    #[cfg(windows)]
    let doorbell = Doorbell {
        event: owner.doorbell().event.try_clone().unwrap(),
    };
    #[cfg(not(windows))]
    let doorbell = Doorbell::new().unwrap();
    // SAFETY: the owner's own mappings, in order.
    let worker = unsafe { Ring::open(owner.descriptor(), control, handles, doorbell) }.unwrap();
    (owner, worker)
}

fn views(output: &mut [Vec<f32>]) -> Vec<&mut [f32]> {
    output.iter_mut().map(Vec::as_mut_slice).collect()
}

fn take(worker: &mut Ring, generation: u64, sequence: u64) -> (Quantum, Vec<Vec<f32>>, Events) {
    let mut input: Vec<Vec<f32>> = (0..2).map(|_| Vec::with_capacity(8)).collect();
    let mut automation = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut events = Events::for_block();
    let quantum = worker
        .take_f32(
            generation,
            sequence,
            &mut input,
            &mut automation,
            &mut events,
        )
        .unwrap();
    (quantum, input, events)
}

#[test]
fn a_quantum_travels_to_the_worker_and_back_with_its_events() {
    let (mut owner, mut worker) = pair(3);
    owner.start(4, 1000);
    assert_eq!(worker.started(), (4, 1000));
    let mut context = BlockContext::new(8);
    context.transport = Some(Transport::new(4800, true));
    let bell = worker.bell_count();
    let note = Event::note_on(2, 0, 60, 100);
    owner
        .submit_f32(
            4,
            1,
            &context,
            &[&[0.5; 8], &[0.25; 8]],
            &[],
            &Events::from_iter([note]),
        )
        .unwrap();
    assert_eq!(owner.submitted(), 1);
    assert_ne!(worker.bell_count(), bell);
    let (quantum, input, events) = take(&mut worker, 4, 1);
    assert_eq!(quantum.context, context);
    assert_eq!(input, [vec![0.5; 8], vec![0.25; 8]]);
    assert_eq!(events, [note]);
    let mut output = vec![vec![0.0; 8]; 2];
    let mut produced = Events::for_block();
    assert_eq!(
        owner
            .read_f32(4, 1, &mut views(&mut output), 1, &mut produced)
            .unwrap(),
        None
    );
    let sysex = Event::sysex(3, &[0xF0, 0x7D, 0xF7]);
    worker
        .complete_f32(
            &quantum,
            &[&[1.0; 8], &[2.0; 8]],
            &Events::from_iter([note, sysex]),
            32,
            Tail::Samples(64),
        )
        .unwrap();
    assert_eq!(owner.completed(), 1);
    // System exclusive events are copied into the reader's reserved room.
    assert!(matches!(
        owner
            .read_f32(4, 1, &mut views(&mut output), 1, &mut produced)
            .unwrap(),
        Some((
            Outcome::Done {
                latency: 32,
                tail: Tail::Samples(64),
                ..
            },
            0
        ))
    ));
    assert_eq!(output, [vec![1.0; 8], vec![2.0; 8]]);
    assert_eq!(produced, [note, sysex]);
    // Another generation's reader does not take it.
    assert_eq!(
        owner
            .read_f32(5, 1, &mut views(&mut output), 1, &mut produced)
            .unwrap(),
        None
    );
}

#[test]
fn output_events_beyond_the_readers_room_are_counted_instead() {
    let (mut owner, mut worker) = pair(3);
    owner.start(4, 1000);
    assert_eq!(worker.started(), (4, 1000));
    owner
        .submit_f32(
            4,
            1,
            &BlockContext::new(8),
            &[&[0.5; 8], &[0.25; 8]],
            &[],
            &Events::new(),
        )
        .unwrap();
    let (quantum, _, _) = take(&mut worker, 4, 1);
    let sent = Events::from_iter([
        Event::note_on(2, 0, 60, 100),
        Event::sysex(3, &[0xF0, 0x7D, 0xF7]),
    ]);
    worker
        .complete_f32(
            &quantum,
            &[&[1.0; 8], &[2.0; 8]],
            &sent,
            32,
            Tail::Samples(64),
        )
        .unwrap();
    // A reader with no room reserved takes the audio and counts both events, the system
    // exclusive one included.
    let mut output = vec![vec![0.0; 8]; 2];
    let mut produced = Events::new();
    assert!(matches!(
        owner
            .read_f32(4, 1, &mut views(&mut output), 1, &mut produced)
            .unwrap(),
        Some((Outcome::Done { .. }, 2))
    ));
    assert_eq!(output, [vec![1.0; 8], vec![2.0; 8]]);
    assert!(produced.is_empty());
}

#[test]
fn a_lane_is_reused_only_after_its_previous_quantum_completed() {
    let (mut owner, mut worker) = pair(3);
    owner.start(1, 1000);
    let context = BlockContext::new(8);
    for sequence in 1..=3 {
        assert!(owner.lane_free(sequence));
        owner
            .submit_f32(
                1,
                sequence,
                &context,
                &[&[0.0f32; 8][..]; 2],
                &[],
                &Events::new(),
            )
            .unwrap();
    }
    assert!(!owner.lane_free(4));
    let (quantum, _, _) = take(&mut worker, 1, 1);
    worker
        .complete_f32(
            &quantum,
            &[&[0.0f32; 8][..]; 2],
            &Events::new(),
            0,
            Tail::Samples(0),
        )
        .unwrap();
    assert!(owner.lane_free(4));
    assert!(!owner.lane_free(5));
}

#[test]
fn a_failed_quantum_reports_its_slot_and_kind() {
    let (mut owner, mut worker) = pair(4);
    owner.start(2, 1000);
    owner
        .submit_f32(
            2,
            1,
            &BlockContext::new(8),
            &[&[0.0f32; 8][..]; 2],
            &[],
            &Events::new(),
        )
        .unwrap();
    let (quantum, _, _) = take(&mut worker, 2, 1);
    worker
        .fail(
            2,
            1,
            Some(quantum.submitted_at),
            Some(3),
            FailureKind::Processing,
        )
        .unwrap();
    let mut output = vec![vec![0.0; 8]; 2];
    assert_eq!(
        owner
            .read_f32(2, 1, &mut views(&mut output), 1, &mut Events::for_block())
            .unwrap(),
        Some((
            Outcome::Failed {
                slot: Some(3),
                kind: FailureKind::Processing
            },
            0
        ))
    );
}

#[test]
fn completions_after_their_deadline_are_counted_late() {
    let (mut owner, mut worker) = pair(3);
    owner.start(1, 0);
    owner
        .submit_f32(
            1,
            1,
            &BlockContext::new(8),
            &[&[0.0f32; 8][..]; 2],
            &[],
            &Events::new(),
        )
        .unwrap();
    let (quantum, _, _) = take(&mut worker, 1, 1);
    std::thread::sleep(std::time::Duration::from_millis(1));
    worker
        .complete_f32(
            &quantum,
            &[&[0.0f32; 8][..]; 2],
            &Events::new(),
            0,
            Tail::Samples(0),
        )
        .unwrap();
    assert_eq!(owner.worker().0, 1);
}

#[test]
fn a_quantum_the_worker_could_not_take_is_not_counted_late() {
    let (mut owner, mut worker) = pair(3);
    owner.start(1, 0);
    owner
        .submit_f32(
            1,
            1,
            &BlockContext::new(8),
            &[&[0.0f32; 8][..]; 2],
            &[],
            &Events::new(),
        )
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1));
    worker
        .fail(1, 1, None, None, FailureKind::Protocol)
        .unwrap();
    assert_eq!(owner.worker().0, 0);
}

#[test]
fn a_worker_cannot_take_a_quantum_of_another_generation() {
    let (mut owner, mut worker) = pair(3);
    owner.start(1, 1000);
    owner
        .submit_f32(
            1,
            1,
            &BlockContext::new(8),
            &[&[0.0f32; 8][..]; 2],
            &[],
            &Events::new(),
        )
        .unwrap();
    let mut input: Vec<Vec<f32>> = (0..2).map(|_| Vec::with_capacity(8)).collect();
    assert!(
        worker
            .take_f32(2, 1, &mut input, &mut Vec::new(), &mut Events::for_block())
            .is_err()
    );
    assert!(
        worker
            .take_f32(1, 2, &mut input, &mut Vec::new(), &mut Events::for_block())
            .is_err()
    );
}

#[test]
fn a_new_generation_reads_and_frees_only_its_own_quanta() {
    let (mut owner, mut worker) = pair(3);
    owner.start(1, 1000);
    let context = BlockContext::new(8);
    for sequence in 1..=3 {
        owner
            .submit_f32(
                1,
                sequence,
                &context,
                &[&[0.5f32; 8][..]; 2],
                &[],
                &Events::new(),
            )
            .unwrap();
        let (quantum, _, _) = take(&mut worker, 1, sequence);
        worker
            .complete_f32(
                &quantum,
                &[&[0.5f32; 8][..]; 2],
                &Events::new(),
                0,
                Tail::Samples(0),
            )
            .unwrap();
    }
    owner.start(2, 1000);
    owner
        .submit_f32(2, 1, &context, &[&[0.0f32; 8][..]; 2], &[], &Events::new())
        .unwrap();
    let mut output = vec![vec![0.0; 8]; 2];
    // The lanes still hold the earlier generation's outputs, which are not this one's.
    assert_eq!(
        owner
            .read_f32(2, 1, &mut views(&mut output), 1, &mut Events::for_block())
            .unwrap(),
        None
    );
    assert!(!owner.lane_free(4));
}

#[test]
fn a_resync_numbers_the_adopted_generation_from_one() {
    let (mut owner, mut worker) = pair(3);
    owner.start(1, 1000);
    let context = BlockContext::new(8);
    for sequence in 1..=3 {
        owner
            .submit_f32(
                1,
                sequence,
                &context,
                &[&[0.5f32; 8][..]; 2],
                &[],
                &Events::new(),
            )
            .unwrap();
    }
    let (quantum, _, _) = take(&mut worker, 1, 1);
    worker
        .complete_f32(
            &quantum,
            &[&[0.5f32; 8][..]; 2],
            &Events::new(),
            0,
            Tail::Samples(0),
        )
        .unwrap();
    // The worker goes on; the quanta it has not taken yet are not taken any more.
    owner.control().resync(2);
    assert_eq!(worker.submitted(), 0);
    assert_eq!(worker.adopt(), 2);
    assert_eq!(owner.completed(), 0);
    for sequence in 1..=3 {
        assert!(owner.lane_free(sequence));
        owner
            .submit_f32(
                2,
                sequence,
                &context,
                &[&[0.0f32; 8][..]; 2],
                &[],
                &Events::new(),
            )
            .unwrap();
    }
    // Lanes free up as the new generation's quanta complete, whatever the old one completed.
    assert!(!owner.lane_free(4));
    let mut output = vec![vec![0.0; 8]; 2];
    assert_eq!(
        owner
            .read_f32(2, 1, &mut views(&mut output), 1, &mut Events::for_block())
            .unwrap(),
        None
    );
    let (quantum, input, _) = take(&mut worker, 2, 1);
    assert_eq!(input, [vec![0.0; 8], vec![0.0; 8]]);
    worker
        .complete_f32(
            &quantum,
            &[&[1.0f32; 8][..]; 2],
            &Events::new(),
            0,
            Tail::Samples(0),
        )
        .unwrap();
    assert!(owner.lane_free(4));
    assert!(matches!(
        owner
            .read_f32(2, 1, &mut views(&mut output), 1, &mut Events::for_block())
            .unwrap(),
        Some((Outcome::Done { .. }, 0))
    ));
    assert_eq!(output, [vec![1.0; 8], vec![1.0; 8]]);
}

#[test]
fn rings_outside_the_supported_lane_counts_are_refused() {
    for lanes in [0, 2, MAX_LANES + 1] {
        assert!(Ring::new(descriptor(lanes)).is_err(), "{lanes}");
    }
}

#[test]
fn the_doorbell_wakes_a_waiting_worker() {
    let (owner, worker) = pair(3);
    let control = worker.control();
    let last = control.bell_count();
    let waiter = std::thread::spawn(move || {
        while control.bell_count() == last {
            control.wait(last);
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(20));
    owner.control().ring();
    waiter.join().unwrap();
}
