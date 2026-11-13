use super::*;
use plughost_core::{AudioBusConfig, AudioInputRoute, Layout, SampleFormat, SlotAudioConfig};

fn config(previous: bool) -> RoutedChainConfig {
    let bus = AudioBusConfig::new(0, Layout::Mono, true);
    let route = AudioInputRoute::new(bus, AudioSource::External { bus: 0 });
    let mut second = route;
    second.bus.id = 1;
    if previous {
        second.source = AudioSource::Previous { bus: 0 };
    }
    RoutedChainConfig::new(
        48_000.0,
        512,
        SampleFormat::F64,
        vec![Layout::Mono],
        vec![
            SlotAudioConfig::new(vec![route], vec![bus]),
            SlotAudioConfig::new(vec![route, second], vec![bus]),
        ],
    )
}

fn native(latency: u32, tail: u32) -> PluginTiming {
    PluginTiming::new(latency, Tail::Samples(tail))
}

/// The bank of a chain without dry paths.
fn bank<S: Sample>(
    config: &RoutedChainConfig,
    timing: &AudioTiming,
) -> Result<DelayBank<S>, Failure> {
    let slots = config.slots.len();
    DelayBank::new(config, timing, &vec![None; slots], &vec![false; slots])
}

fn replacement<S: Sample>(
    config: &RoutedChainConfig,
    current: &AudioTiming,
    replaced: usize,
    timing: &AudioTiming,
) -> Result<DelayReplacement<S>, Failure> {
    let slots = config.slots.len();
    let none = vec![None; slots];
    let off = vec![false; slots];
    DelayReplacement::new(config, current, replaced, timing, &none, &off, &off)
}

#[test]
fn unconnected_previous_plugins_do_not_add_latency_or_tail() {
    let config = config(false);
    let native = [native(480, 1000), native(12, 24)];
    let timing = AudioTiming::new(&config, &native).unwrap();
    assert_eq!(timing.latency(), 12);
    assert_eq!(
        timing.tail(&native, &[false; 2]).unwrap(),
        Tail::Samples(24)
    );
}

#[test]
fn connected_paths_align_external_audio_and_accumulate_tail() {
    let config = config(true);
    let native = [native(3, 9), native(2, 4)];
    let timing = AudioTiming::new(&config, &native).unwrap();
    assert_eq!(timing.latency(), 5);
    assert_eq!(
        timing.tail(&native, &[false; 2]).unwrap(),
        Tail::Samples(13)
    );
    let mut delays = bank::<f64>(&config, &timing).unwrap();
    let mut first = vec![vec![1.0 + f64::EPSILON, 2.0]];
    delays.apply(1, 0, &mut first);
    assert_eq!(first, vec![vec![0.0, 0.0]]);
    let mut next = vec![vec![3.0, 4.0, 5.0]];
    delays.apply(1, 0, &mut next);
    assert_eq!(next, vec![vec![0.0, 1.0 + f64::EPSILON, 2.0]]);
    let mut previous = vec![vec![4.0]];
    delays.apply(1, 1, &mut previous);
    assert_eq!(previous, vec![vec![4.0]]);
}

#[test]
fn silence_and_disabled_connections_do_not_connect_a_discarded_path() {
    let mut config = config(true);
    config.slots[1].inputs[1].source = AudioSource::Silence;
    let native = [native(480, 1000), native(12, 24)];
    let timing = AudioTiming::new(&config, &native).unwrap();
    assert_eq!(timing.latency(), 12);
    config.slots[1].inputs[1].source = AudioSource::Previous { bus: 0 };
    config.slots[1].inputs[1].bus.active = false;
    let timing = AudioTiming::new(&config, &native).unwrap();
    assert_eq!(timing.latency(), 12);
}

#[test]
fn timing_changes_require_reprepare_and_latency_overflow_is_explicit() {
    let config = config(true);
    let native = [native(3, 9), native(2, 4)];
    let timing = AudioTiming::new(&config, &native).unwrap();
    let mut changed = native;
    changed[0].latency = 4;
    let (slot, failure) = timing.tail(&changed, &[false; 2]).unwrap_err();
    assert_eq!((slot, failure.kind), (0, FailureKind::RestartRequired));
    changed = native;
    changed[1].restart_required = true;
    let (slot, failure) = timing.tail(&changed, &[false; 2]).unwrap_err();
    assert_eq!((slot, failure.kind), (1, FailureKind::RestartRequired));
    assert_eq!(
        timing.current_tail(&changed, &[false; 2]),
        Tail::Samples(13)
    );
    changed[1].restart_required = false;
    changed[0].latency = u32::MAX;
    assert_eq!(
        AudioTiming::new(&config, &changed).unwrap_err().kind,
        FailureKind::Configuration
    );
}

#[test]
fn declared_delays_reject_excessive_storage() {
    let config = config(true);
    let native = [native(u32::MAX, 0), native(0, 0)];
    let timing = AudioTiming::new(&config, &native).unwrap();
    assert_eq!(
        bank::<f32>(&config, &timing).err().unwrap().kind,
        FailureKind::Configuration
    );
    let timing = AudioTiming::new(&config, &[self::native(3, 0), self::native(0, 0)]).unwrap();
    let mut delays = bank::<f32>(&config, &timing).unwrap();
    let mut samples = vec![vec![1.0f32; 16]];
    delays.apply(1, 0, &mut samples);
    assert_eq!(&samples[0][..3], &[0.0; 3]);
    assert_eq!(&samples[0][3..], &[1.0; 13]);
}

#[test]
fn replacement_preserves_own_inputs_and_disconnected_paths_but_retimes_dependents() {
    let mut config = config(true);
    config.slots.push(config.slots[0].clone());
    config.slots.push(config.slots[1].clone());
    let timing = AudioTiming::new(
        &config,
        &[native(3, 0), native(0, 0), native(5, 0), native(0, 0)],
    )
    .unwrap();
    let mut delays = bank::<f64>(&config, &timing).unwrap();
    delays.apply(1, 0, &mut [vec![1.0, 2.0]]);
    delays.apply(3, 0, &mut [vec![3.0, 4.0]]);
    // Replacing slot 1 leaves its input history intact: it belongs to slot 0's path.
    delays.replace_slot(&mut replacement(&config, &timing, 1, &timing).unwrap());
    let mut own = [vec![0.0; 3]];
    delays.apply(1, 0, &mut own);
    assert_eq!(own, [vec![0.0, 1.0, 2.0]]);

    // Slot 0's replacement changes latency. Only its connected dependent is rebuilt.
    let changed = AudioTiming::new(
        &config,
        &[native(7, 0), native(0, 0), native(5, 0), native(0, 0)],
    )
    .unwrap();
    delays.replace_slot(&mut replacement(&config, &timing, 0, &changed).unwrap());
    let mut dependent = [vec![9.0; 8]];
    delays.apply(1, 0, &mut dependent);
    assert_eq!(dependent, [vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 9.0]]);
    let mut independent = [vec![0.0; 5]];
    delays.apply(3, 0, &mut independent);
    assert_eq!(independent, [vec![0.0, 0.0, 0.0, 3.0, 4.0]]);
}

#[test]
fn failed_delay_replacement_preserves_pending_audio_and_checks_the_whole_bank() {
    let config = config(true);
    let plan = AudioTiming::new(&config, &[native(3, 0), native(0, 0)]).unwrap();
    let mut bank = bank::<f64>(&config, &plan).unwrap();
    bank.apply(1, 0, &mut [vec![1.0, 2.0]]);
    let huge = AudioTiming::new(&config, &[native(u32::MAX, 0), native(0, 0)]).unwrap();
    assert_eq!(
        replacement::<f64>(&config, &plan, 0, &huge)
            .err()
            .unwrap()
            .kind,
        FailureKind::Configuration
    );
    let mut next = [vec![3.0, 4.0, 5.0]];
    bank.apply(1, 0, &mut next);
    assert_eq!(next, [vec![0.0, 1.0, 2.0]]);

    let samples = plughost_core::MAX_ALIGNMENT_BYTES / size_of::<f64>();
    assert!(add_budget::<f64>(0, [(1, samples)].into_iter()).is_ok());
    assert_eq!(
        add_budget::<f64>(0, [(1, samples), (1, 1)].into_iter())
            .unwrap_err()
            .kind,
        FailureKind::Configuration
    );
    assert_eq!(
        add_budget::<f64>(0, [(usize::MAX, 2)].into_iter())
            .unwrap_err()
            .kind,
        FailureKind::Configuration
    );
}

#[test]
fn delayed_channels_match_a_sample_queue_across_ring_wraps() {
    let mut config = config(true);
    config.inputs = vec![Layout::Stereo];
    for slot in &mut config.slots {
        for route in &mut slot.inputs {
            route.bus.layout = Layout::Stereo;
        }
        for bus in &mut slot.outputs {
            bus.layout = Layout::Stereo;
        }
    }
    let timing = AudioTiming::new(&config, &[native(5, 0), native(0, 0)]).unwrap();
    let mut delays = bank::<f64>(&config, &timing).unwrap();
    let mut queues = [[0.0; 5], [0.0; 5]].map(std::collections::VecDeque::from);
    let mut next = 0.0;
    // Block lengths below, at and above the delay wrap the ring at every phase.
    for frames in [1, 3, 7, 2, 11, 4, 5, 9, 1, 13, 6] {
        let mut channels: Vec<Vec<f64>> = (0..2)
            .map(|channel| {
                (0..frames)
                    .map(|i| next + i as f64 + 1000.0 * channel as f64)
                    .collect()
            })
            .collect();
        next += frames as f64;
        let expected: Vec<Vec<f64>> = channels
            .iter()
            .zip(&mut queues)
            .map(|(input, queue)| {
                input
                    .iter()
                    .map(|&sample| {
                        queue.push_back(sample);
                        queue.pop_front().unwrap()
                    })
                    .collect()
            })
            .collect();
        delays.apply(1, 0, &mut channels);
        assert_eq!(channels, expected, "{frames} frames");
    }
}

#[test]
fn a_host_bypassed_slot_adds_no_tail_but_passes_the_incoming_one_on() {
    let config = config(true);
    let native = [native(3, 9), native(2, 4)];
    let timing = AudioTiming::new(&config, &native).unwrap();
    assert_eq!(
        timing.tail(&native, &[true, false]).unwrap(),
        Tail::Samples(4)
    );
    assert_eq!(
        timing.tail(&native, &[false, true]).unwrap(),
        Tail::Samples(9)
    );
    assert_eq!(
        timing.current_tail(&native, &[true, true]),
        Tail::Samples(0)
    );
    let mut infinite = native;
    infinite[1].tail = Tail::Infinite;
    assert_eq!(
        timing.current_tail(&infinite, &[false, true]),
        Tail::Samples(9)
    );
    assert_eq!(
        timing.current_tail(&infinite, &[false, false]),
        Tail::Infinite
    );
    let (plan, tail) = AudioTiming::plan(&config, &native, &[false, true]).unwrap();
    assert_eq!((plan.latency(), tail), (5, Tail::Samples(9)));
}

fn dry(channels: usize) -> Option<DryPath> {
    Some(DryPath {
        input: 0,
        output: 0..channels,
        adaptation: plughost_core::ChannelAdaptation::Exact,
    })
}

#[test]
fn dry_lines_delay_by_their_slot_latency_within_the_budget() {
    let config = config(false);
    let paths = [dry(1), None];
    let timing = AudioTiming::new(&config, &[native(3, 0), native(2, 0)]).unwrap();
    let mut delays = DelayBank::<f64>::new(&config, &timing, &paths, &[false; 2]).unwrap();
    assert!(delays.dry_history(0));
    assert!(!delays.dry_history(1));
    let mut first = [vec![1.0, 2.0]];
    assert!(delays.apply_dry(0, &mut first));
    assert_eq!(first, [vec![0.0, 0.0]]);
    let mut next = [vec![3.0, 4.0, 5.0]];
    assert!(delays.apply_dry(0, &mut next));
    assert_eq!(next, [vec![0.0, 1.0, 2.0]]);
    assert!(!delays.apply_dry(1, &mut next));
    assert_eq!(next, [vec![0.0, 1.0, 2.0]]);
    let alignment = Alignment::new(&config, timing, &paths, &[false; 2]).unwrap();
    assert_eq!(alignment.dry_lines(), [true, false]);

    // A line past the budget is left out while its slot's host bypass is off, and fails the
    // bank while it is on.
    let huge = AudioTiming::new(&config, &[native(u32::MAX, 0), native(2, 0)]).unwrap();
    let alignment = Alignment::new(&config, huge.clone(), &paths, &[false; 2]).unwrap();
    assert_eq!(alignment.dry_lines(), [false, false]);
    assert_eq!(
        Alignment::new(&config, huge, &paths, &[true, false])
            .err()
            .unwrap()
            .kind,
        FailureKind::Configuration
    );
    // A bypassed slot's line comes before an earlier one that is not.
    let samples = (plughost_core::MAX_ALIGNMENT_BYTES / size_of::<f64>()) as u32;
    let both = AudioTiming::new(&config, &[native(samples, 0), native(samples, 0)]).unwrap();
    let paths = [dry(1), dry(1)];
    let alignment = Alignment::new(&config, both, &paths, &[false, true]).unwrap();
    assert_eq!(alignment.dry_lines(), [false, true]);
}

#[test]
fn a_replacement_keeps_its_dry_line_at_the_same_latency_and_renews_it_at_another() {
    let config = config(false);
    let paths = [dry(1), dry(1)];
    let timing = AudioTiming::new(&config, &[native(3, 0), native(0, 0)]).unwrap();
    let mut delays = DelayBank::<f64>::new(&config, &timing, &paths, &[true, false]).unwrap();
    delays.apply_dry(0, &mut [vec![1.0, 2.0]]);
    let mut same = DelayReplacement::new(
        &config,
        &timing,
        0,
        &timing,
        &paths,
        &[true, false],
        &[true, true],
    )
    .unwrap();
    assert_eq!(same.dry_line(), None);
    delays.replace_slot(&mut same);
    let mut kept = [vec![0.0; 3]];
    delays.apply_dry(0, &mut kept);
    assert_eq!(kept, [vec![0.0, 1.0, 2.0]]);

    let longer = AudioTiming::new(&config, &[native(4, 0), native(0, 0)]).unwrap();
    let mut renewed = DelayReplacement::new(
        &config,
        &timing,
        0,
        &longer,
        &paths,
        &[true, false],
        &[true, true],
    )
    .unwrap();
    assert_eq!(renewed.dry_line(), Some(true));
    delays.replace_slot(&mut renewed);
    let mut fresh = [vec![9.0; 5]];
    delays.apply_dry(0, &mut fresh);
    assert_eq!(fresh, [vec![0.0, 0.0, 0.0, 0.0, 9.0]]);

    // Past the budget, a bypassed slot's replacement fails; another one goes without a line.
    let huge = AudioTiming::new(&config, &[native(u32::MAX, 0), native(0, 0)]).unwrap();
    let replace = |bypassed: &[bool]| {
        DelayReplacement::<f64>::new(&config, &longer, 0, &huge, &paths, bypassed, &[true, true])
    };
    assert_eq!(
        replace(&[true, false]).err().unwrap().kind,
        FailureKind::Configuration
    );
    assert_eq!(replace(&[false, false]).unwrap().dry_line(), Some(false));
}

#[test]
#[ignore = "latency alignment benchmark; run in release mode with --nocapture"]
fn delay_bank_benchmark() {
    use std::hint::black_box;
    use std::io::Write;
    use std::time::Instant;
    const BLOCKS: usize = 100_000;
    for (latency, fingerprints) in [
        (1, [0x972e9f649d3f4325, 0xaa6ba19ffd0b2325]),
        (480, [0xe415cc889d3f4325, 0x451cefebfd0b2325]),
        (8192, [0xb7faacd89d3f4325, 0xa2817b3bfd0b2325]),
    ] {
        for (frames, expected) in [64, 512].into_iter().zip(fingerprints) {
            let config = config(true);
            let timing = AudioTiming::new(&config, &[native(latency, 0), native(0, 0)]).unwrap();
            let mut delays = bank::<f64>(&config, &timing).unwrap();
            let mut channels = vec![vec![0.0f64; frames]];
            let mut checksum = 0xcbf29ce484222325u64;
            let started = Instant::now();
            for index in 0..BLOCKS {
                channels[0].fill(index as f64 + 0.25);
                delays.apply(1, 0, black_box(&mut channels));
                for &sample in &channels[0] {
                    checksum = (checksum ^ sample.to_bits()).wrapping_mul(0x100000001b3);
                }
            }
            let elapsed = started.elapsed();
            assert_eq!(checksum, expected);
            for (offset, &sample) in channels[0].iter().enumerate() {
                let input_at = (BLOCKS - 1) * frames + offset - latency as usize;
                assert_eq!(sample, (input_at / frames) as f64 + 0.25);
            }
            writeln!(
                std::io::stdout(),
                "delay={latency} frames={frames} blocks={BLOCKS} ms={:.3} checksum={checksum:016x}",
                elapsed.as_secs_f64() * 1000.0
            )
            .unwrap();
        }
    }
}
