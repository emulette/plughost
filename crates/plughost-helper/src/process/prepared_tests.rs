use super::super::audio_timing::{AudioTiming, DelayBank};
use super::*;
use plughost_core::{AudioBusConfig, AudioInputRoute, Layout, SlotAudioConfig};

fn bus(id: u64, layout: Layout) -> AudioBusConfig {
    AudioBusConfig::new(id, layout, true)
}
fn config() -> RoutedChainConfig {
    RoutedChainConfig::new(
        48_000.0,
        8,
        SampleFormat::F64,
        vec![Layout::Mono],
        vec![SlotAudioConfig::new(
            vec![
                AudioInputRoute::new(bus(7, Layout::Stereo), AudioSource::External { bus: 0 })
                    .with_adaptation(ChannelAdaptation::MonoToStereo),
                AudioInputRoute::new(bus(19, Layout::Mono), AudioSource::Silence),
            ],
            vec![bus(4, Layout::Stereo)],
        )],
    )
}
fn timing() -> PluginTiming {
    PluginTiming::new(0, plughost_core::render::Tail::Samples(0))
}

#[test]
fn short_and_long_blocks_clear_silence_and_outputs_and_preserve_f64_routing() {
    let config = config();
    config.validate(1).unwrap();
    let mut prepared = Routed::<f64>::new(&config, &[timing()], &[None]);
    let plan = AudioTiming::new(&config, &[timing()]).unwrap();
    let mut delays = DelayBank::new(
        &config,
        &plan,
        &vec![None; config.slots.len()],
        &vec![false; config.slots.len()],
    )
    .unwrap();
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
    let mut prepared = Routed::<f64>::new(&config, &[timing()], &[None]);
    let plan = AudioTiming::new(&config, &[timing()]).unwrap();
    let mut delays = DelayBank::new(
        &config,
        &plan,
        &vec![None; config.slots.len()],
        &vec![false; config.slots.len()],
    )
    .unwrap();
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
    let route = |id, source| AudioInputRoute::new(mono(id), source);
    let config = {
        let mut value = config();
        value.inputs = vec![Layout::Mono];
        value.slots = vec![
            SlotAudioConfig::new(
                vec![route(0, AudioSource::External { bus: 0 })],
                vec![mono(4)],
            ),
            SlotAudioConfig::new(
                vec![
                    route(7, AudioSource::Previous { bus: 4 }),
                    route(19, AudioSource::External { bus: 0 }),
                ],
                vec![mono(4)],
            ),
        ];
        value
    };
    config.validate(2).unwrap();
    let mut upstream = timing();
    upstream.latency = 3;
    let timings = [upstream, timing()];
    let mut prepared = Routed::<f64>::new(&config, &timings, &[None, None]);
    let plan = AudioTiming::new(&config, &timings).unwrap();
    let mut delays = DelayBank::new(
        &config,
        &plan,
        &vec![None; config.slots.len()],
        &vec![false; config.slots.len()],
    )
    .unwrap();
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
    let mut prepared = Routed::<f64>::new(&config(), &[timing()], &[None]);
    let slots = &mut prepared.slots;
    let dense: Vec<_> = (0..plughost_core::MAX_BLOCK_EVENTS)
        .map(|index| {
            AutomationEvent::new(
                0,
                ParameterChange::new(7, 0, index as f64 / plughost_core::MAX_BLOCK_EVENTS as f64),
            )
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

fn info(id: u64, direction: AudioDirection, role: AudioBusRole) -> AudioBusInfo {
    AudioBusInfo::new(id, 0, "", direction, role, 0)
}

#[test]
fn dry_paths_take_the_first_main_buses_and_refuse_other_layouts() {
    use AudioBusRole::{Auxiliary, Main};
    use AudioDirection::{Input, Output};
    // As the VST3 routing fixture: an auxiliary bus before the main one in each direction.
    let buses = [
        info(0, Input, Auxiliary),
        info(1, Input, Main),
        info(0, Output, Auxiliary),
        info(1, Output, Main),
        info(2, Output, Main),
    ];
    let slot = |main_in: Layout, main_out: Layout| {
        SlotAudioConfig::new(
            vec![
                AudioInputRoute::new(bus(0, Layout::Mono), AudioSource::Silence),
                AudioInputRoute::new(bus(1, main_in), AudioSource::External { bus: 0 }),
            ],
            vec![
                bus(0, Layout::Stereo),
                bus(1, main_out),
                bus(2, Layout::Stereo),
            ],
        )
    };
    let path = |main_in, main_out| DryPath::new(&slot(main_in, main_out), &buses);
    assert_eq!(
        path(Layout::Stereo, Layout::Stereo),
        Some(DryPath {
            input: 1,
            output: 2..4,
            adaptation: ChannelAdaptation::Exact,
        })
    );
    assert_eq!(
        path(Layout::Surround51, Layout::Surround51).unwrap().output,
        2..8
    );
    assert_eq!(
        path(Layout::Mono, Layout::Stereo).unwrap().adaptation,
        ChannelAdaptation::MonoToStereo
    );
    assert_eq!(
        path(Layout::Stereo, Layout::Mono),
        Some(DryPath {
            input: 1,
            output: 2..3,
            adaptation: ChannelAdaptation::StereoToMono,
        })
    );
    assert_eq!(path(Layout::Surround51, Layout::Stereo), None);
    assert_eq!(path(Layout::Mono, Layout::Surround51), None);
    // An inactive auxiliary output takes no channels; an inactive main bus leaves no path.
    let mut setup = slot(Layout::Stereo, Layout::Stereo);
    setup.outputs[0].active = false;
    assert_eq!(DryPath::new(&setup, &buses).unwrap().output, 0..2);
    setup.inputs[1].bus.active = false;
    assert_eq!(DryPath::new(&setup, &buses), None);
    // An instrument has no main input.
    let instrument = [info(0, Output, Main)];
    let setup = SlotAudioConfig::new(Vec::new(), vec![bus(0, Layout::Stereo)]);
    assert_eq!(DryPath::new(&setup, &instrument), None);
}

/// One slot with a mono external main input and a stereo main output after a stereo auxiliary
/// output, its dry path, and a bank that delays it by `latency`.
fn bypassable(latency: u32) -> (RoutedChainConfig, Routed<f64>, DelayBank<f64>) {
    let config = RoutedChainConfig::new(
        48_000.0,
        8,
        SampleFormat::F64,
        vec![Layout::Mono],
        vec![SlotAudioConfig::new(
            vec![AudioInputRoute::new(
                bus(1, Layout::Mono),
                AudioSource::External { bus: 0 },
            )],
            vec![bus(0, Layout::Stereo), bus(1, Layout::Stereo)],
        )],
    );
    config.validate(1).unwrap();
    let dry = [Some(DryPath {
        input: 0,
        output: 2..4,
        adaptation: ChannelAdaptation::MonoToStereo,
    })];
    let mut native = timing();
    native.latency = latency;
    let prepared = Routed::new(&config, &[native], &dry);
    let plan = AudioTiming::new(&config, &[native]).unwrap();
    let delays = DelayBank::new(&config, &plan, &dry, &[false]).unwrap();
    (config, prepared, delays)
}

/// Processes `input` through the plugin, which writes `wet` on every output, and then host
/// bypass at `mix`, moving by `step`.
fn bypass_block(
    prepared: &mut Routed<f64>,
    delays: &mut DelayBank<f64>,
    input: &[f64],
    wet: f64,
    mix: &mut f32,
    enabled: bool,
    step: f32,
) -> Vec<Vec<f64>> {
    let frames = input.len();
    let external = [input.to_vec()];
    let slot = &mut prepared.slots[0];
    slot.begin(0, frames, &[]);
    slot.process(&external, &[], frames, delays, 0, |_, output, _| {
        for channel in output {
            channel.fill(wet);
        }
    });
    slot.bypass(
        &external,
        &[],
        frames,
        delays,
        0,
        Fade { enabled, mix, step },
    );
    slot.output.clone()
}

#[test]
fn host_bypass_plays_the_input_delayed_by_the_latency_fed_while_it_was_off() {
    let (_, mut prepared, mut delays) = bypassable(3);
    let mut mix = 0.0;
    // Off, the plugin's output stays; the dry line takes the input all the same.
    let output = bypass_block(
        &mut prepared,
        &mut delays,
        &[1.0, 2.0],
        0.5,
        &mut mix,
        false,
        1.0,
    );
    assert_eq!(output, vec![vec![0.5; 2]; 4]);
    // On, settled: the mono input on both main channels, three frames late, and silent
    // auxiliary outputs, even when the plugin's output is not finite.
    mix = 1.0;
    let output = bypass_block(
        &mut prepared,
        &mut delays,
        &[3.0, 4.0, 5.0, 6.0],
        f64::NAN,
        &mut mix,
        true,
        1.0,
    );
    let main = vec![0.0, 1.0, 2.0, 3.0];
    assert_eq!(output, [vec![0.0; 4], vec![0.0; 4], main.clone(), main]);
    assert_eq!(mix, 1.0);
}

#[test]
fn host_bypass_crossfades_linearly_and_reaches_its_ends_exactly() {
    let (_, mut prepared, mut delays) = bypassable(0);
    let mut mix = 0.0;
    let input = [1.0; 6];
    let output = bypass_block(
        &mut prepared,
        &mut delays,
        &input,
        0.5,
        &mut mix,
        true,
        0.25,
    );
    let main = vec![0.625, 0.75, 0.875, 1.0, 1.0, 1.0];
    let auxiliary = vec![0.375, 0.25, 0.125, 0.0, 0.0, 0.0];
    assert_eq!(output, [auxiliary.clone(), auxiliary, main.clone(), main]);
    assert_eq!(mix, 1.0);
    // Back, across blocks: the crossfade goes on where the last block left it.
    let output = bypass_block(
        &mut prepared,
        &mut delays,
        &input[..2],
        0.5,
        &mut mix,
        false,
        0.25,
    );
    assert_eq!(output[2], [0.875, 0.75]);
    assert_eq!(mix, 0.5);
    let output = bypass_block(
        &mut prepared,
        &mut delays,
        &input[..3],
        0.5,
        &mut mix,
        false,
        0.25,
    );
    assert_eq!(output[2], [0.625, 0.5, 0.5]);
    assert_eq!(output[0], [0.375, 0.5, 0.5]);
    assert_eq!(mix, 0.0);
}

#[test]
fn a_stereo_main_input_is_averaged_into_a_mono_main_output() {
    let config = RoutedChainConfig::new(
        48_000.0,
        4,
        SampleFormat::F64,
        vec![Layout::Stereo],
        vec![SlotAudioConfig::new(
            vec![AudioInputRoute::new(
                bus(0, Layout::Stereo),
                AudioSource::External { bus: 0 },
            )],
            vec![bus(0, Layout::Mono)],
        )],
    );
    let dry = [Some(DryPath {
        input: 0,
        output: 0..1,
        adaptation: ChannelAdaptation::StereoToMono,
    })];
    let mut prepared = Routed::<f64>::new(&config, &[timing()], &dry);
    let plan = AudioTiming::new(&config, &[timing()]).unwrap();
    let mut delays = DelayBank::new(&config, &plan, &dry, &[true]).unwrap();
    let external = [vec![1.0, 0.5], vec![0.0, 0.25]];
    let slot = &mut prepared.slots[0];
    slot.begin(0, 2, &[]);
    slot.process(&external, &[], 2, &mut delays, 0, |_, _, _| {});
    slot.bypass(
        &external,
        &[],
        2,
        &mut delays,
        0,
        Fade {
            enabled: true,
            mix: &mut 1.0,
            step: 1.0,
        },
    );
    assert_eq!(slot.output, [vec![0.5, 0.375]]);
}

#[test]
fn host_bypass_needs_no_processing_allocations() {
    let (_, mut prepared, mut delays) = bypassable(5);
    let inputs: Vec<_> = [8, 1, 5, 0, 8]
        .map(|frames| vec![vec![0.25; frames]])
        .into();
    let mut mix = 0.0;
    let counts = allocation::measure(|| {
        for (index, input) in inputs.iter().enumerate() {
            let frames = input[0].len();
            let slot = &mut prepared.slots[0];
            slot.begin(0, frames, &[]);
            slot.process(input, &[], frames, &mut delays, 0, |_, _, _| {});
            slot.bypass(
                input,
                &[],
                frames,
                &mut delays,
                0,
                Fade {
                    enabled: index > 0,
                    mix: &mut mix,
                    step: 0.25,
                },
            );
        }
    });
    assert_eq!(counts, [0, 0, 0]);
    assert_eq!(prepared.slots[0].output[2], vec![0.25; 8]);
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

#[test]
#[ignore = "prepared channel-view benchmark; run in release mode with --nocapture"]
fn prepared_channel_views_benchmark() {
    use std::hint::black_box;
    use std::io::Write;
    use std::time::Instant;
    let config = black_box(config());
    let mut prepared = Routed::<f64>::new(&config, &[timing()], &[None]);
    let plan = AudioTiming::new(&config, &[timing()]).unwrap();
    let mut delays = DelayBank::new(
        &config,
        &plan,
        &vec![None; config.slots.len()],
        &vec![false; config.slots.len()],
    )
    .unwrap();
    const BLOCKS: usize = 1_000_000;
    for frames in [1, 8] {
        let external = vec![vec![1.0 + f64::EPSILON; frames]];
        let started = Instant::now();
        for _ in 0..BLOCKS {
            let slot = black_box(&mut prepared.slots[0]);
            slot.begin(0, frames, &[]);
            slot.process(
                &external,
                &[],
                frames,
                &mut delays,
                0,
                |input, output, _| {
                    let input = black_box(input);
                    let output = black_box(output);
                    for channel in output {
                        channel.copy_from_slice(input[0]);
                    }
                },
            );
        }
        let elapsed = started.elapsed();
        assert_eq!(prepared.slots[0].output, vec![external[0].clone(); 2]);
        writeln!(
            std::io::stdout(),
            "channels=3/2 frames={frames} blocks={BLOCKS} ms={:.3}",
            elapsed.as_secs_f64() * 1000.0
        )
        .unwrap();
    }
}
