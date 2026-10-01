use super::super::audio_timing::{AudioTiming, DelayBank};
use super::*;
use plughost_core::{AudioBusConfig, AudioInputRoute, Layout, ProcessMode, SlotAudioConfig};

fn bus(id: u64, layout: Layout) -> AudioBusConfig {
    AudioBusConfig {
        id,
        layout,
        active: true,
    }
}
fn config() -> RoutedChainConfig {
    RoutedChainConfig {
        event_inputs: 0,
        sample_rate: 48_000.0,
        max_block_size: 8,
        sample_format: SampleFormat::F64,
        mode: ProcessMode::Offline,
        inputs: vec![Layout::Mono],
        slots: vec![SlotAudioConfig {
            events: Default::default(),
            configuration: None,
            inputs: vec![
                AudioInputRoute {
                    bus: bus(7, Layout::Stereo),
                    source: AudioSource::External { bus: 0 },
                    adaptation: ChannelAdaptation::MonoToStereo,
                },
                AudioInputRoute {
                    bus: bus(19, Layout::Mono),
                    source: AudioSource::Silence,
                    adaptation: ChannelAdaptation::Exact,
                },
            ],
            outputs: vec![bus(4, Layout::Stereo)],
        }],
    }
}
fn timing() -> PluginTiming {
    PluginTiming::new(0, plughost_core::render::Tail::Samples(0))
}

#[test]
fn short_and_long_blocks_clear_silence_and_outputs_and_preserve_f64_routing() {
    let config = config();
    config.validate(1).unwrap();
    let mut prepared = Routed::<f64>::new(&config, &[timing()]);
    let plan = AudioTiming::new(&config, &[timing()]).unwrap();
    let mut delays = DelayBank::new(&config, &plan).unwrap();
    let value = 1.0 + f64::EPSILON;
    for frames in [8, 1, 5, 0, 8] {
        let external = [vec![value; frames]];
        let slot = &mut prepared.slots[0];
        slot.begin(0, frames, &[]);
        slot.process(
            &external,
            &[],
            frames,
            &mut delays,
            0,
            |input, output, _| {
                assert_eq!(input.len(), 3);
                assert_eq!(input[0], external[0]);
                assert_eq!(input[1], external[0]);
                assert!(input[2].iter().all(|s| *s == 0.0));
                assert!(output.iter().all(|c| c.iter().all(|s| *s == 0.0)));
                for channel in output {
                    channel.copy_from_slice(input[0]);
                }
            },
        );
        assert_eq!(slot.output, vec![vec![value; frames]; 2]);
        // The next block must overwrite all scratch contents, including silent routes.
        for channel in &mut slot.input {
            channel.fill(99.0);
        }
    }
}

#[test]
fn prepared_audio_storage_and_channel_views_need_no_processing_allocations() {
    let config = config();
    let mut prepared = Routed::<f64>::new(&config, &[timing()]);
    let plan = AudioTiming::new(&config, &[timing()]).unwrap();
    let mut delays = DelayBank::new(&config, &plan).unwrap();
    let inputs: Vec<_> = [8, 1, 5, 0, 8]
        .into_iter()
        .map(|frames| vec![vec![1.0; frames]])
        .collect();
    let counts = allocation::measure(|| {
        for input in &inputs {
            let frames = input[0].len();
            let slot = &mut prepared.slots[0];
            slot.begin(0, frames, &[]);
            slot.process(input, &[], frames, &mut delays, 0, |input, output, _| {
                output[0].copy_from_slice(input[0]);
            });
        }
    });
    assert_eq!(counts, [0, 0, 0]);
    assert_eq!(prepared.slots[0].output[0], vec![1.0; 8]);
}

#[test]
fn undelayed_exact_routes_pass_their_source_and_delayed_routes_are_aligned() {
    let mono = |id| bus(id, Layout::Mono);
    let route = |id, source| AudioInputRoute {
        bus: mono(id),
        source,
        adaptation: ChannelAdaptation::Exact,
    };
    let config = RoutedChainConfig {
        inputs: vec![Layout::Mono],
        slots: vec![
            SlotAudioConfig {
                events: Default::default(),
                configuration: None,
                inputs: vec![route(0, AudioSource::External { bus: 0 })],
                outputs: vec![mono(4)],
            },
            SlotAudioConfig {
                events: Default::default(),
                configuration: None,
                inputs: vec![
                    route(7, AudioSource::Previous { bus: 4 }),
                    route(19, AudioSource::External { bus: 0 }),
                ],
                outputs: vec![mono(4)],
            },
        ],
        ..config()
    };
    config.validate(2).unwrap();
    let mut upstream = timing();
    upstream.latency = 3;
    let timings = [upstream, timing()];
    let mut prepared = Routed::<f64>::new(&config, &timings);
    let plan = AudioTiming::new(&config, &timings).unwrap();
    let mut delays = DelayBank::new(&config, &plan).unwrap();
    let external = [vec![1.0, 2.0, 3.0, 4.0, 5.0]];
    let (first, second) = prepared.slots.split_at_mut(1);
    first[0].begin(0, 5, &[]);
    first[0].process(&external, &[], 5, &mut delays, 0, |input, output, _| {
        assert!(std::ptr::eq(input[0], external[0].as_slice()));
        output[0].copy_from_slice(&[6.0, 7.0, 8.0, 9.0, 10.0]);
    });
    second[0].begin(1, 5, &[]);
    second[0].process(
        &external,
        &first[0].output,
        5,
        &mut delays,
        1,
        |input, _, _| {
            assert!(std::ptr::eq(input[0], first[0].output[0].as_slice()));
            assert_eq!(input[1], [0.0, 0.0, 0.0, 1.0, 2.0]);
        },
    );
}

#[test]
fn dense_automation_keeps_duplicate_order_without_initial_growth() {
    let mut prepared = Routed::<f64>::new(&config(), &[timing()]);
    let slots = &mut prepared.slots;
    let dense: Vec<_> = (0..plughost_core::MAX_BLOCK_EVENTS)
        .map(|index| AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: 7,
                offset: 0,
                value: index as f64 / plughost_core::MAX_BLOCK_EVENTS as f64,
            },
        })
        .collect();
    let counts = allocation::measure(|| {
        slots[0].begin(0, 1, &[]);
        assert!(slots[0].changes.is_empty());
        slots[0].begin(0, 5, &dense);
    });
    assert_eq!(counts, [0, 0, 0]);
    assert_eq!(slots[0].changes.len(), dense.len());
    for (actual, expected) in slots[0].changes.iter().zip(&dense) {
        assert_eq!(*actual, expected.change);
    }
}

mod allocation {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    thread_local! {
        static COUNTS: Cell<Option<[usize; 3]>> = const { Cell::new(None) };
    }
    struct Allocator;
    #[global_allocator]
    static ALLOCATOR: Allocator = Allocator;
    fn count(index: usize) {
        let _ = COUNTS.try_with(|counts| {
            if let Some(mut value) = counts.get() {
                value[index] += 1;
                counts.set(Some(value));
            }
        });
    }
    // SAFETY: all allocation operations are delegated to System with unchanged arguments.
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            count(0);
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            count(0);
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            count(2);
            unsafe { System.dealloc(pointer, layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            count(1);
            unsafe { System.realloc(pointer, layout, size) }
        }
    }
    pub fn measure(call: impl FnOnce()) -> [usize; 3] {
        struct Stop;
        impl Drop for Stop {
            fn drop(&mut self) {
                COUNTS.set(None);
            }
        }
        COUNTS.set(Some([0; 3]));
        let _stop = Stop;
        call();
        COUNTS.get().unwrap()
    }
}
