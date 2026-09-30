//! Fixture locations and chain helpers shared by the integration test targets. Build the helper
//! and the test plugins with `scripts/build-helper.sh` and `scripts/build-test-plugins.sh` (the
//! `.ps1` versions on Windows) first.
// Every test target compiles this module and uses a different subset of it.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use plughost::{
    BlockContext, Chain, HostIdentity, Layout, PluginFormat, PluginRef, RoutedChainConfig, Timeouts,
};

pub fn target() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")
}

pub fn helper() -> PathBuf {
    target().join(if cfg!(target_os = "macos") {
        "helper/PlughostHelper.app/Contents/MacOS/plughost-helper"
    } else {
        "helper/plughost-helper.exe"
    })
}

pub fn plugins() -> PathBuf {
    target().join("test-plugins")
}

fn extension(format: PluginFormat) -> &'static str {
    match format {
        PluginFormat::Vst3 => "vst3",
        PluginFormat::Clap => "clap",
        _ => unreachable!("fixtures exist for VST3 and CLAP only"),
    }
}

/// A test plugin bundle, such as `plughost-test-delay`, in the format's bundle form.
pub fn bundle(format: PluginFormat, name: &str) -> PathBuf {
    plugins().join(format!("{name}.{}", extension(format)))
}

pub fn fixture(format: PluginFormat, name: &str, class_id: &str) -> PluginRef {
    PluginRef {
        format,
        bundle: Some(bundle(format, name)),
        class_id: class_id.to_owned(),
    }
}

/// A variant of the delay fixture. Variant 1 is the plain delay, and each variant is a bundle
/// named `plughost-test-<name>` with class ID `0x79000000 + variant`.
pub fn delay_variant(format: PluginFormat, name: &str, variant: u32) -> PluginRef {
    let class_id = match format {
        PluginFormat::Vst3 => format!("706C7567686F737444656C61{:08X}", 0x7900_0000 + variant),
        _ => format!("com.studio.plughost.test-delay.{:x}", 0x7900_0000 + variant),
    };
    fixture(format, &format!("plughost-test-{name}"), &class_id)
}

pub fn delay(format: PluginFormat) -> PluginRef {
    delay_variant(format, "delay", 1)
}

pub fn routing(format: PluginFormat) -> PluginRef {
    let class_id = match format {
        PluginFormat::Vst3 => "706C7567686F7374526F7574696E6701",
        _ => "com.studio.plughost.test-routing",
    };
    fixture(format, "plughost-test-routing", class_id)
}

pub fn spawn(plugins: &[PluginRef]) -> Chain {
    spawn_with(plugins, &HostIdentity::default(), Timeouts::default())
}

pub fn spawn_with(plugins: &[PluginRef], identity: &HostIdentity, timeouts: Timeouts) -> Chain {
    Chain::spawn(&helper(), plugins, identity, timeouts).unwrap()
}

/// A stereo-in, stereo-out configuration at 48 kHz with 512-frame blocks.
pub fn stereo_config(chain: &mut Chain) -> RoutedChainConfig {
    let outputs = vec![Layout::Stereo; chain.plugins().len()];
    chain
        .main_bus_config(48_000.0, 512, Layout::Stereo, &outputs)
        .unwrap()
}

pub fn prepare(chain: &mut Chain) {
    let config = stereo_config(chain);
    chain.prepare_audio(&config).unwrap();
}

/// Processes `frames` of constant stereo `value` and returns the left channel.
pub fn block(chain: &mut Chain, frames: usize, value: f32) -> Vec<f32> {
    let input = vec![value; frames];
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    chain
        .process_audio_f32(
            &BlockContext::new(frames),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
    left
}

/// Copies a file or a whole directory tree.
pub fn copy(source: &Path, destination: &Path) {
    if source.is_dir() {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            copy(&entry.path(), &destination.join(entry.file_name()));
        }
    } else {
        fs::copy(source, destination).unwrap();
    }
}

/// The plain delay fixture in a prepared single-slot chain.
pub fn prepared_delay(format: PluginFormat) -> Chain {
    let mut chain = spawn(&[delay(format)]);
    prepare(&mut chain);
    chain
}
