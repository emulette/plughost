//! A plugin's latency is read as it activates and holds until the chain is prepared again or
//! reset, through real helper processes: a latency change the plugin announces while it
//! processes is reported, and rendering keeps one alignment. Build the helper and the test
//! plugins first (see `support`).

use plughost::{PluginFormat, RenderOptions, TailPolicy, render};

mod support;

use support::{delay_variant, prepare, spawn};

/// The latency the delay fixture reports, and the one its `latency-change` variant announces
/// after it processed 2,400 frames.
const LATENCY: u32 = 480;
const CHANGED: u32 = 960;
const FRAMES: usize = 9600;

#[test]
#[ignore = "needs helper and the latency-change fixture (.ps1 or .sh build scripts)"]
fn a_latency_change_takes_effect_when_the_chain_is_reset() {
    let mut chain = spawn(&[delay_variant(PluginFormat::Vst3, "latency-change", 0xE)]);
    prepare(&mut chain);
    assert_eq!(chain.latency(), LATENCY);
    let input: Vec<f32> = (0..FRAMES).map(|i| (i as f32 * 0.01).sin()).collect();
    let options = RenderOptions {
        tail: TailPolicy::Reported,
        max_tail_seconds: 0.0,
    };
    // The plugin announces its new latency halfway through the render and keeps its delay until
    // it activates again, so the whole render keeps the alignment it started with. Its latency
    // and tail getters answer wrong values on any thread but the one that created it.
    let rendered = render(&mut chain, &[&input, &input], FRAMES, &[], &options).unwrap();
    assert_eq!(rendered.latency, LATENCY);
    assert_eq!(rendered.channels, [input.clone(), input.clone()]);
    let timing = chain.take_changes().unwrap().snapshot[0].unwrap();
    assert_eq!((timing.latency, timing.latency_changed), (LATENCY, true));
    assert_eq!(chain.latency(), LATENCY);

    // Reset activates the plugin again, which applies the latency it announced.
    chain.reset().unwrap();
    assert_eq!(chain.latency(), CHANGED);
    let timing = chain.take_changes().unwrap().snapshot[0].unwrap();
    assert_eq!((timing.latency, timing.latency_changed), (CHANGED, false));
    let rendered = render(&mut chain, &[&input, &input], FRAMES, &[], &options).unwrap();
    assert_eq!(rendered.latency, CHANGED);
    assert_eq!(rendered.channels, [input.clone(), input]);
}
