//! Offline rendering at every common sample rate and at block sizes that do and do not divide the
//! input, through real helpers. Build the helper and the test plugins first (see `support`).

use plughost::{
    BlockContext, Chain, Event, FailureKind, Layout, PluginFormat, RenderOptions, SampleFormat,
    TailPolicy, render,
};

mod support;

use support::{delay, spawn, synth};

const RATES: [f64; 6] = [44_100.0, 48_000.0, 88_200.0, 96_000.0, 176_400.0, 192_000.0];
const BLOCKS: [usize; 3] = [32, 500, 4096];
/// The delay fixture's latency at 48 kHz; its CLAP side scales it with the sample rate.
const LATENCY: f64 = 480.0;

fn expected_latency(format: PluginFormat, rate: f64) -> u32 {
    match format {
        PluginFormat::Clap => (LATENCY * rate / 48_000.0).round() as u32,
        _ => LATENCY as u32,
    }
}

/// A tenth of a second of distinct, non-silent samples per channel.
fn signal(rate: f64, channel: usize) -> Vec<f64> {
    (0..(rate / 10.0) as usize + 7)
        .map(|frame| ((frame * (channel + 3)) % 97) as f64 / 97.0 - 0.5)
        .collect()
}

fn check<S>(chain: &mut Chain, format: PluginFormat, rate: f64, block: usize)
where
    S: plughost_core::Sample + From<f32> + Into<f64>,
    Chain: plughost_core::render::Process<S, Error = plughost::Error>,
{
    let input: Vec<Vec<S>> = (0..2)
        .map(|channel| {
            signal(rate, channel)
                .into_iter()
                .map(|sample| S::from(sample as f32))
                .collect()
        })
        .collect();
    let frames = input[0].len();
    let options = RenderOptions::new(TailPolicy::Reported, 0.0);
    let rendered = render(chain, &[&input[0], &input[1]], frames, &[], &options).unwrap();
    let case = format!("{format:?} {rate} Hz, {block}-frame blocks");
    assert_eq!(rendered.latency, expected_latency(format, rate), "{case}");
    // The delay has unit gain, so latency compensation leaves the input exactly.
    for (channel, output) in rendered.channels.iter().enumerate() {
        assert_eq!(output.len(), frames, "{case}");
        assert!(
            output
                .iter()
                .zip(&input[channel])
                .all(|(out, expected)| (*out).into() == (*expected).into()),
            "{case}, channel {channel}"
        );
    }
}

#[test]
#[ignore = "needs helper and delay fixtures (.ps1 or .sh build scripts)"]
fn delay_renders_exactly_at_every_rate_and_block_size() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = spawn(&[delay(format)]);
        for rate in RATES {
            for block in BLOCKS {
                let mut config = chain
                    .main_bus_config(rate, block, Layout::Stereo, &[Layout::Stereo])
                    .unwrap();
                chain.prepare_audio(&config).unwrap();
                check::<f32>(&mut chain, format, rate, block);
                // Only the VST3 side of the fixture processes 64-bit samples.
                if format == PluginFormat::Vst3 {
                    config.sample_format = SampleFormat::F64;
                    chain.prepare_audio(&config).unwrap();
                    check::<f64>(&mut chain, format, rate, block);
                }
            }
        }
    }
}

/// Plays a note for one block and returns the loudest sample of the left channel.
fn note_peak(chain: &mut Chain) -> Result<f32, plughost::Error> {
    let (mut left, mut right) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    chain.process_audio_f32(
        &BlockContext::new(256),
        &[],
        &mut [&mut left, &mut right],
        &[],
        &[Event::note_on(0, 0, 60, 100)],
        &mut Vec::new(),
    )?;
    Ok(left.iter().fold(0.0, |peak, sample| peak.max(sample.abs())))
}

#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn a_plugin_that_cannot_save_its_state_is_prepared_again_in_place() {
    // The synth fixture has no CLAP state extension, so no fresh instance can take its state.
    let mut chain = spawn(&[synth(PluginFormat::Clap)]);
    let at = |chain: &mut Chain, rate: f64, output: Layout| {
        chain
            .main_bus_config(rate, 512, Layout::None, &[output])
            .unwrap()
    };
    let config = at(&mut chain, 48_000.0, Layout::Stereo);
    chain.prepare_audio(&config).unwrap();
    assert!(note_peak(&mut chain).unwrap() > 0.0);
    let config = at(&mut chain, 44_100.0, Layout::Stereo);
    chain.prepare_audio(&config).unwrap();
    assert!(note_peak(&mut chain).unwrap() > 0.0);
    chain.reprepare().unwrap();
    assert!(note_peak(&mut chain).unwrap() > 0.0);
    // The active instance changed before the failure, so the chain is no longer prepared.
    let refused = at(&mut chain, 44_100.0, Layout::Mono);
    assert!(chain.prepare_audio(&refused).is_err());
    assert_eq!(
        note_peak(&mut chain).unwrap_err().kind(),
        FailureKind::NotPrepared
    );
    chain.prepare_audio(&config).unwrap();
    assert!(note_peak(&mut chain).unwrap() > 0.0);
}
