//! Scanning and chains through real helper processes. Build the helper and the test plugins with
//! `scripts/build-helper.sh` and `scripts/build-test-plugins.sh` (the `.ps1` versions on Windows), then run
//! `cargo test -p plughost -- --ignored`.

use std::path::Path;
use std::time::{Duration, Instant};

use plughost::{
    Chain, Error, Event, HostIdentity, InputError, Layout, PluginFormat, PluginRef, RenderOptions,
    RoutedChainConfig, ScanAction, ScanOutcome, ScanPolicy, Scanner, Source, TailPolicy, Timeouts,
    render,
};

mod support;

use support::{bundle, delay, delay_variant, fixture, helper, plugins, prepare, spawn, spawn_with};

const AGAIN: &str = "84E8DE5F92554F5396FAE4133C935A18";
const AGAIN_GAIN: u64 = 0;
const DELAY_LATENCY: u32 = 480;
const CLAP_GAIN: u64 = 0;
const CLAP_SYNTH: &str = "com.studio.plughost.test-synth";

#[test]
fn empty_chain_is_rejected_before_starting_a_helper() {
    let result = Chain::spawn(
        Path::new("missing-helper.exe"),
        &[],
        &HostIdentity::default(),
        Timeouts::default(),
    );
    assert!(matches!(
        result,
        Err(Error::Input {
            slot: None,
            error: InputError::EmptyChain
        })
    ));
}
/// Apple's AUDelay, 'aufx' 'dely' 'appl', and its wet/dry mix and delay time parameters.
#[cfg(target_os = "macos")]
const AU_DELAY: &str = "6175667864656C796170706C";
#[cfg(target_os = "macos")]
const AU_WET_DRY_MIX: u64 = 0;
#[cfg(target_os = "macos")]
const AU_DELAY_TIME: u64 = 1;

#[cfg(target_os = "macos")]
fn audio_unit(class_id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::AudioUnit,
        bundle: None,
        class_id: class_id.to_owned(),
    }
}

fn again() -> PluginRef {
    fixture(PluginFormat::Vst3, "again", AGAIN)
}

fn cache_path(directory: &tempfile::TempDir) -> std::path::PathBuf {
    directory.path().join("cache.json")
}

/// A scanner that loads only the named test plugin bundles, such as `again.vst3`.
fn scanner_for(cache: &Path, files: &[&str]) -> Scanner {
    let policy = files
        .iter()
        .fold(ScanPolicy::new(ScanAction::Block), |policy, file| {
            policy.bundle(plugins().join(file), ScanAction::Allow)
        });
    Scanner::new(&helper(), cache)
        .directories(vec![plugins()])
        .policy(policy)
        .stall_timeout(Duration::from_secs(1))
}

fn signal(frames: usize) -> Vec<Vec<f32>> {
    (0..2)
        .map(|ch| {
            (0..frames)
                .map(|i| 0.5 * ((i as f32) * 0.011 * (ch + 1) as f32).sin())
                .collect()
        })
        .collect()
}

fn run(chain: &mut Chain, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let slices: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let options = RenderOptions::new(TailPolicy::Reported, 0.0);
    render(chain, &slices, input[0].len(), &[], &options)
        .unwrap()
        .channels
}

fn max_difference(a: &[Vec<f32>], b: &[Vec<f32>], gain: f32) -> f32 {
    a.iter()
        .zip(b)
        .flat_map(|(x, y)| x.iter().zip(y).map(move |(p, q)| (p - gain * q).abs()))
        .fold(0.0, f32::max)
}

fn try_process_block(chain: &mut Chain, input: &[Vec<f32>]) -> Result<Vec<Vec<f32>>, Error> {
    let slices: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut output = vec![vec![0.0; input[0].len()]; 2];
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    chain.process_audio_f32(
        &plughost_core::BlockContext::new(input[0].len()),
        &slices,
        &mut outputs,
        &[],
        &[],
        &mut Vec::new(),
    )?;
    Ok(output)
}

fn process_block(chain: &mut Chain, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
    try_process_block(chain, input).unwrap()
}

/// The scan of a test plugin bundle, named with its extension.
fn outcome<'a>(catalog: &'a plughost::Catalog, file: &str) -> &'a plughost::BundleScan {
    let path = plugins().join(file);
    catalog
        .bundles
        .iter()
        .find(|scan| scan.path == path)
        .unwrap()
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn scanning_reports_classes_and_isolates_crashes_and_hangs() {
    let directory = tempfile::tempdir().unwrap();
    let catalog = scanner_for(
        &cache_path(&directory),
        &[
            "again.vst3",
            "plughost-test-delay.vst3",
            "plughost-test-delay.clap",
            "plughost-test-crash-on-scan.vst3",
            "plughost-test-crash-on-scan.clap",
            "plughost-test-hang-on-scan.vst3",
            "plughost-test-hang-on-scan.clap",
        ],
    )
    .scan(&|_| {})
    .unwrap();

    let found = |file: &str| match &outcome(&catalog, file).outcome {
        ScanOutcome::Found(classes) => classes
            .iter()
            .map(|c| c.class_id.clone())
            .collect::<Vec<_>>(),
        other => panic!("{file}: {other:?}"),
    };
    assert_eq!(found("again.vst3")[0], AGAIN);
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let plugin = delay(format);
        let file = plugin.bundle.as_ref().unwrap().file_name().unwrap();
        assert_eq!(found(file.to_str().unwrap()), [plugin.class_id.as_str()]);
    }
    for extension in ["vst3", "clap"] {
        let crashed = &outcome(
            &catalog,
            &format!("plughost-test-crash-on-scan.{extension}"),
        )
        .outcome;
        assert!(
            matches!(crashed, ScanOutcome::Crashed(status) if !status.is_empty()),
            "{extension}: {crashed:?}"
        );
        assert_eq!(
            outcome(&catalog, &format!("plughost-test-hang-on-scan.{extension}")).outcome,
            ScanOutcome::TimedOut
        );
    }
    assert!(
        catalog
            .plugins()
            .iter()
            .any(|(plugin, _)| *plugin == delay(PluginFormat::Clap))
    );
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn rescans_use_the_cache_and_exclude_bundles_that_keep_failing() {
    let directory = tempfile::tempdir().unwrap();
    let scanner = scanner_for(
        &cache_path(&directory),
        &["again.vst3", "plughost-test-crash-on-scan.vst3"],
    );
    scanner.scan(&|_| {}).unwrap();
    let second = scanner.scan(&|_| {}).unwrap();
    assert_eq!(outcome(&second, "again.vst3").source, Source::Cache);
    let crash = outcome(&second, "plughost-test-crash-on-scan.vst3");
    assert_eq!(
        (crash.source, crash.failures, crash.excluded),
        (Source::Loaded, 2, true)
    );

    let third = scanner.scan(&|_| {}).unwrap();
    let crash = outcome(&third, "plughost-test-crash-on-scan.vst3");
    assert_eq!((crash.source, crash.excluded), (Source::Cache, true));

    let retried = scanner
        .retry(&bundle(PluginFormat::Vst3, "plughost-test-crash-on-scan"))
        .unwrap();
    assert_eq!(retried.source, Source::Loaded);
    assert!(matches!(retried.outcome, ScanOutcome::Crashed(_)));
}

/// A chain that mixes formats renders the input through every slot's latency, aligned.
#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn chains_of_mixed_formats_render_aligned_output() {
    struct Case {
        plugins: Vec<PluginRef>,
        latency: Option<u32>,
        /// A slot's gain edit made before rendering: (slot, parameter, normalized value).
        edit: Option<(usize, u64, f64)>,
        gain: f32,
        tolerance: f32,
    }
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut cases = vec![
        Case {
            plugins: vec![again(), delay(PluginFormat::Vst3)],
            latency: Some(DELAY_LATENCY),
            edit: None,
            gain: 1.0,
            tolerance: 1e-6,
        },
        Case {
            plugins: vec![delay(PluginFormat::Vst3), delay(PluginFormat::Clap)],
            latency: Some(2 * DELAY_LATENCY),
            edit: Some((1, CLAP_GAIN, 0.5)),
            gain: 0.5,
            tolerance: 1e-6,
        },
    ];
    #[cfg(target_os = "macos")]
    cases.push(Case {
        plugins: vec![delay(PluginFormat::Vst3), audio_unit(AU_DELAY)],
        latency: None,
        // Fully dry, the Audio Unit passes the delayed signal through.
        edit: Some((1, AU_WET_DRY_MIX, 0.0)),
        gain: 1.0,
        tolerance: 1e-4,
    });
    for case in cases {
        let names: Vec<_> = case.plugins.iter().map(|p| p.class_id.clone()).collect();
        let mut chain = spawn(&case.plugins);
        prepare(&mut chain);
        if let Some(latency) = case.latency {
            assert_eq!(chain.latency(), latency, "{names:?}");
        }
        if let Some((slot, id, value)) = case.edit {
            chain.set_parameter(slot, id, value).unwrap();
        }
        let input = signal(4800);
        assert!(
            max_difference(&run(&mut chain, &input), &input, case.gain) < case.tolerance,
            "{names:?}"
        );
        chain.reset().unwrap();
        let silence = vec![vec![0.0; 512]; 2];
        assert!(
            process_block(&mut chain, &silence)
                .iter()
                .flatten()
                .all(|sample| sample.abs() <= case.tolerance),
            "{names:?} after reset"
        );
        assert!(
            max_difference(&run(&mut chain, &input), &input, case.gain) < case.tolerance,
            "{names:?} after reset"
        );
    }
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn a_crash_names_the_slot_and_the_chain_can_recover() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let crashing = delay_variant(format, "crash-in-process", 2);
        let mut chain = spawn(&[again(), crashing]);
        prepare(&mut chain);
        match try_process_block(&mut chain, &signal(512)) {
            Err(Error::Crashed {
                slot: Some(1),
                status: Some(status),
            }) => assert!(!status.success()),
            other => panic!("{format:?}: {other:?}"),
        }
        chain = chain.recover(&[]).unwrap();
        assert!(!chain.parameters(0).unwrap().is_empty());
    }
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn a_hang_times_out_and_names_the_slot() {
    let timeouts = {
        let mut timeouts = Timeouts::default();
        timeouts.process = Duration::from_millis(200);
        timeouts
    };
    // The CLAP fixture keeps logging while it hangs, which must not extend the timeout.
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let hanging = delay_variant(format, "hang-in-process", 3);
        let mut chain = spawn_with(&[hanging], &HostIdentity::default(), timeouts);
        prepare(&mut chain);
        let started = Instant::now();
        assert!(
            matches!(
                try_process_block(&mut chain, &signal(512)),
                Err(Error::TimedOut { slot: Some(0) })
            ),
            "{format:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn blocks_keep_processing_while_a_plugin_holds_up_the_main_thread() {
    // The fixture holds up the helper's main thread for 3 s, from a callback its first block
    // requests; each block must still finish within the process timeout.
    let mut chain = spawn_with(
        &[delay_variant(PluginFormat::Clap, "stall-main-thread", 9)],
        &HostIdentity::default(),
        {
            let mut timeouts = Timeouts::default();
            timeouts.process = Duration::from_secs(1);
            timeouts
        },
    );
    prepare(&mut chain);
    let input = [0.25f32; 512];
    let (mut left, mut right) = ([0.0f32; 512], [0.0f32; 512]);
    let started = Instant::now();
    let mut blocks = 0;
    while started.elapsed() < Duration::from_secs(2) {
        chain
            .process_audio_f32(
                &plughost_core::BlockContext::new(512),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        blocks += 1;
    }
    assert!(blocks > 1);
    // A main-thread request waits for the stall, which therefore covered every block above.
    chain.parameters(0).unwrap();
    assert!(started.elapsed() > Duration::from_millis(2500));
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn a_plugin_holding_up_the_idle_main_thread_is_named_when_a_request_times_out() {
    let mut chain = spawn_with(
        &[
            delay(PluginFormat::Clap),
            delay_variant(PluginFormat::Clap, "stall-main-thread", 9),
        ],
        &HostIdentity::default(),
        {
            let mut timeouts = Timeouts::default();
            timeouts.control = Duration::from_millis(500);
            timeouts
        },
    );
    prepare(&mut chain);
    run(&mut chain, &signal(512));
    // Let the main thread's next tick enter the stalling callback before asking about slot 0.
    std::thread::sleep(Duration::from_millis(200));
    assert!(matches!(
        chain.parameters(0),
        Err(Error::TimedOut { slot: Some(1) })
    ));
}

/// A parameter edit saved right after it is made, with no audio in between, is restored in a
/// new helper.
#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn an_edit_saved_right_away_is_restored_in_a_new_helper() {
    struct Case {
        plugin: PluginRef,
        parameter: u64,
        /// The gain a render of the restored plugin applies, where the plugin renders a gain.
        rendered_gain: Option<f32>,
    }
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut cases = vec![
        Case {
            plugin: again(),
            parameter: AGAIN_GAIN,
            rendered_gain: Some(0.25),
        },
        Case {
            plugin: delay(PluginFormat::Clap),
            parameter: CLAP_GAIN,
            rendered_gain: Some(0.25),
        },
    ];
    #[cfg(target_os = "macos")]
    cases.push(Case {
        plugin: audio_unit(AU_DELAY),
        parameter: AU_DELAY_TIME,
        rendered_gain: None,
    });
    for case in cases {
        let value = |chain: &mut Chain| {
            chain
                .parameters(0)
                .unwrap()
                .into_iter()
                .find(|(p, _)| p.id == case.parameter)
                .unwrap()
                .1
        };
        let mut original = spawn(std::slice::from_ref(&case.plugin));
        prepare(&mut original);
        original.set_parameter(0, case.parameter, 0.25).unwrap();
        let state = original
            .save_state(0, plughost::StatePurpose::Project)
            .unwrap();
        let edited = value(&mut original);
        assert!((edited - 0.25).abs() < 1e-4, "{}", case.plugin.class_id);
        drop(original);

        let mut fresh = spawn(std::slice::from_ref(&case.plugin));
        prepare(&mut fresh);
        fresh
            .restore_state(0, &state, plughost::StatePurpose::Project)
            .unwrap();
        assert!(
            (value(&mut fresh) - edited).abs() < 1e-4,
            "{}",
            case.plugin.class_id
        );
        if let Some(gain) = case.rendered_gain {
            let input = signal(4800);
            assert!(
                max_difference(&run(&mut fresh, &input), &input, gain) < 1e-6,
                "{}",
                case.plugin.class_id
            );
        }
    }
}

#[test]
fn a_missing_helper_is_a_start_error() {
    let result = Chain::spawn(
        Path::new("/nonexistent/plughost-helper"),
        &[delay(PluginFormat::Vst3)],
        &HostIdentity::default(),
        Timeouts::default(),
    );
    assert!(matches!(result, Err(Error::HelperStart(_))));
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh; opens a window"]
fn editors_open_while_the_chain_processes() {
    let mut chain = spawn(&[again()]);
    prepare(&mut chain);
    assert_eq!(
        chain.capabilities(0).unwrap().plugin.embedded_editor,
        plughost::Support::Unknown
    );
    chain.open_editor(0).unwrap();
    assert_eq!(
        chain.capabilities(0).unwrap().plugin.embedded_editor,
        plughost::Support::Supported
    );
    assert!(chain.editor_open(0).unwrap());
    let input = signal(512);
    assert!(max_difference(&process_block(&mut chain, &input), &input, 1.0) < 1e-6);
    chain.set_parameter(0, AGAIN_GAIN, 0.5).unwrap();
    assert!(max_difference(&process_block(&mut chain, &input), &input, 0.5) < 1e-6);
    chain.close_editor(0).unwrap();
    assert!(!chain.editor_open(0).unwrap());
    assert_eq!(
        chain.capabilities(0).unwrap().plugin.embedded_editor,
        plughost::Support::Unknown
    );
    chain.open_editor(0).unwrap();
    chain.reset().unwrap();
    assert!(chain.editor_open(0).unwrap());
    assert!(max_difference(&process_block(&mut chain, &input), &input, 0.5) < 1e-6);
    let state = chain
        .save_state(0, plughost::StatePurpose::Project)
        .unwrap();
    chain.set_parameter(0, AGAIN_GAIN, 0.75).unwrap();
    chain
        .restore_state(0, &state, plughost::StatePurpose::Project)
        .unwrap();
    assert!(chain.editor_open(0).unwrap());
    assert!(max_difference(&process_block(&mut chain, &input), &input, 0.5) < 1e-6);
    let mut broken = state;
    broken.component.clear();
    assert_eq!(
        chain
            .restore_state(0, &broken, plughost::StatePurpose::Project)
            .unwrap_err()
            .kind(),
        plughost::FailureKind::State
    );
    assert!(chain.editor_open(0).unwrap());
    assert!(max_difference(&process_block(&mut chain, &input), &input, 0.5) < 1e-6);
    chain.close_editor(0).unwrap();
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn invalid_controls_are_rejected_without_changing_the_plugin() {
    for reference in [again(), delay(PluginFormat::Clap)] {
        let mut chain = spawn(&[reference]);
        prepare(&mut chain);
        chain.set_parameter(0, 0, 0.25).unwrap();
        for error in [
            chain.close_editor(1).unwrap_err(),
            chain.editor_open(1).unwrap_err(),
            chain.open_editor(1).unwrap_err(),
            chain.parameters(1).unwrap_err(),
            chain
                .save_state(1, plughost::StatePurpose::Project)
                .unwrap_err(),
            chain.set_parameter(1, 0, 0.5).unwrap_err(),
        ] {
            assert!(matches!(
                error,
                Error::Input {
                    slot: Some(1),
                    error: InputError::Slot
                }
            ));
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 1.1] {
            assert!(matches!(
                chain.set_parameter(0, 0, value),
                Err(Error::Input {
                    slot: Some(0),
                    error: InputError::ParameterValue,
                })
            ));
        }
        assert!(matches!(
            chain.set_parameter(0, u64::MAX, 0.5),
            Err(Error::Input {
                slot: Some(0),
                error: InputError::UnknownParameter { id: u64::MAX },
            })
        ));
        if chain.plugins()[0].format == PluginFormat::Vst3 {
            let (meter, _) = chain
                .parameters(0)
                .unwrap()
                .into_iter()
                .find(|(p, _)| p.flags.read_only)
                .unwrap();
            assert!(
                matches!(chain.set_parameter(0, meter.id, 0.5), Err(Error::Input {
                slot: Some(0), error: InputError::ReadOnlyParameter { id },
            }) if id == meter.id)
            );
        }
        let input = signal(4800);
        assert!(max_difference(&run(&mut chain, &input), &input, 0.25) < 1e-6);
    }
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn invalid_prepares_preserve_the_prepared_chain() {
    let mut chain = spawn(&[again()]);
    prepare(&mut chain);
    for sample_rate in [f64::NAN, f64::INFINITY, 0.0, -48_000.0] {
        let invalid = RoutedChainConfig {
            sample_rate,
            ..support::stereo_config(&mut chain)
        };
        assert!(matches!(
            chain.prepare_audio(&invalid),
            Err(Error::Input {
                slot: None,
                error: InputError::SampleRate,
            })
        ));
        let input = signal(512);
        assert!(max_difference(&process_block(&mut chain, &input), &input, 1.0) < 1e-6);
    }
    for max_block_size in [0, i32::MAX as usize + 1] {
        let invalid = RoutedChainConfig {
            max_block_size,
            ..support::stereo_config(&mut chain)
        };
        assert!(matches!(
            chain.prepare_audio(&invalid),
            Err(Error::Input {
                slot: None,
                error: InputError::BlockSize,
            })
        ));
        let input = signal(512);
        assert!(max_difference(&process_block(&mut chain, &input), &input, 1.0) < 1e-6);
    }
}

#[test]
#[ignore = "needs scripts/build-helper.sh; opens a window"]
#[cfg(target_os = "macos")]
fn audio_unit_editors_open_in_the_helper() {
    let mut chain = spawn(&[audio_unit(AU_DELAY)]);
    prepare(&mut chain);
    chain.open_editor(0).unwrap();
    assert!(chain.editor_open(0).unwrap());
    process_block(&mut chain, &signal(512));
    chain.close_editor(0).unwrap();
}

#[test]
#[ignore = "needs scripts/build-helper.sh and scripts/build-test-plugins.sh"]
fn notes_render_through_a_chain_in_time() {
    let mut chain = spawn(&[
        fixture(PluginFormat::Clap, "plughost-test-synth", CLAP_SYNTH),
        delay(PluginFormat::Vst3),
    ]);
    let config = chain
        .main_bus_config(48_000.0, 512, Layout::None, &[Layout::Stereo; 2])
        .unwrap();
    chain.prepare_audio(&config).unwrap();
    let events = [
        Event::note_on(1000, 0, 69, 127),
        Event::note_off(2000, 0, 69, 0),
    ];
    let options = RenderOptions::new(TailPolicy::Reported, 0.0);
    let rendered = render::<f32, _>(&mut chain, &[], 4800, &events, &options).unwrap();
    assert_eq!(rendered.latency, DELAY_LATENCY);
    let output = &rendered.channels[0];
    assert!(output[..1000].iter().all(|&s| s == 0.0));
    assert_eq!(output[1000], 1.0);
    assert!(output[2000..].iter().all(|&s| s == 0.0));
}
