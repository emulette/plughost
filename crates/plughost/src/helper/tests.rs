use super::*;
use plughost_core::ipc::shared::{Descriptor, SharedAudio, SlotConfig};
use plughost_core::{
    AudioBusConfig, AudioInputRoute, AudioSource, ChannelAdaptation, Layout, ProcessMode,
    RoutedChainConfig, SampleFormat, SlotAudioConfig,
};

#[test]
#[ignore = "needs scripts/build-helper.sh or scripts/build-helper.ps1"]
fn transferring_to_an_exited_helper_reports_its_exit() {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    #[cfg(target_os = "windows")]
    let path = target.join("helper/plughost-helper.exe");
    #[cfg(target_os = "macos")]
    let path = target.join("helper/PlughostHelper.app/Contents/MacOS/plughost-helper");
    let mut helper = Helper::spawn(&path, Mode::Host).unwrap();
    helper
        .send(Request::Shutdown, Duration::from_secs(5))
        .unwrap();
    helper
        .monitor()
        .wait_for_exit(Duration::from_secs(5))
        .unwrap();
    let memory = memory(1);
    let result = prepare(&mut helper, &memory, stereo(), Duration::from_secs(1));
    assert!(matches!(result, Err(Error::Crashed { .. })), "{result:?}");
}

/// Stereo through the main buses of one VST3 slot, whose bus IDs are their indices.
fn stereo() -> RoutedChainConfig {
    let bus = AudioBusConfig {
        id: 0,
        layout: Layout::Stereo,
        active: true,
    };
    RoutedChainConfig {
        event_inputs: 0,
        sample_rate: 48_000.0,
        max_block_size: 512,
        sample_format: SampleFormat::F32,
        mode: ProcessMode::Offline,
        inputs: vec![Layout::Stereo],
        slots: vec![SlotAudioConfig {
            events: Default::default(),
            configuration: None,
            inputs: vec![AudioInputRoute {
                bus,
                source: AudioSource::External { bus: 0 },
                adaptation: ChannelAdaptation::Exact,
            }],
            outputs: vec![bus],
        }],
    }
}

fn memory(generation: u64) -> SharedAudio {
    SharedAudio::new(Descriptor {
        generation,
        config: SlotConfig {
            sample_format: plughost_core::SampleFormat::F32,
            max_frames: 512,
            input_channels: 2,
            output_channels: 2,
        },
    })
    .unwrap()
}

fn prepare(
    helper: &mut Helper,
    memory: &SharedAudio,
    config: RoutedChainConfig,
    timeout: Duration,
) -> Result<Response, Error> {
    let descriptor = memory.descriptor();
    helper.prepare_shared(
        memory,
        |handle| Request::PrepareAudio {
            config,
            memory: descriptor,
            handle,
        },
        timeout,
    )
}
