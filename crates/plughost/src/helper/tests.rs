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

#[test]
#[ignore = "needs helper and native delay fixture"]
fn editing_controls_return_timing_in_their_own_response() {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let path = target.join(if cfg!(target_os = "macos") {
        "helper/PlughostHelper.app/Contents/MacOS/plughost-helper"
    } else {
        "helper/plughost-helper.exe"
    });
    let timeout = Duration::from_secs(10);
    let mut helper = Helper::spawn(&path, Mode::Host).unwrap();
    helper
        .load(
            &HostIdentity::default(),
            &[PluginRef {
                format: plughost_core::PluginFormat::Clap,
                bundle: Some(target.join("test-plugins/plughost-test-delay.clap")),
                class_id: "com.studio.plughost.test-delay.79000001".into(),
            }],
            timeout,
        )
        .unwrap();
    let memory = memory(1);
    prepare(&mut helper, &memory, stereo(), timeout).unwrap();
    let Response::Parameters(parameters) = helper
        .request(Request::Parameters { slot: 0 }, timeout)
        .unwrap()
    else {
        panic!("expected parameters")
    };
    let id = parameters
        .iter()
        .find(|(info, _)| info.title == "Tail samples")
        .unwrap()
        .0
        .id;
    let Response::ParameterText(text) = helper
        .request(
            Request::ParameterText {
                slot: 0,
                id,
                value: 0.25,
            },
            timeout,
        )
        .unwrap()
    else {
        panic!("expected parameter text")
    };
    for (request, tail) in [
        (
            Request::SetParameter {
                slot: 0,
                id,
                value: 0.5,
            },
            16,
        ),
        (Request::SetParameterText { slot: 0, id, text }, 8),
        (Request::Reset, 8),
    ] {
        assert_eq!(
            helper.request(request, timeout).unwrap(),
            Response::Timing {
                latency: 480,
                tail: plughost_core::render::Tail::Samples(tail),
                latency_changed: false,
            }
        );
    }
}

#[test]
#[ignore = "needs the helper and timers CLAP fixture"]
fn main_thread_ticks_during_queued_requests() {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    #[cfg(target_os = "windows")]
    let path = target.join("helper/plughost-helper.exe");
    #[cfg(target_os = "macos")]
    let path = target.join("helper/PlughostHelper.app/Contents/MacOS/plughost-helper");
    let mut helper = Helper::spawn(&path, Mode::Host).unwrap();
    helper
        .load(
            &HostIdentity::default(),
            &[PluginRef {
                format: plughost_core::PluginFormat::Clap,
                bundle: Some(target.join("test-plugins/plughost-test-timers.clap")),
                class_id: "com.studio.plughost.test-delay.7900000d".into(),
            }],
            Duration::from_secs(30),
        )
        .unwrap();

    // Fill the main-thread queue before reading responses. The IPC reader can enqueue the
    // entire burst even while the main thread is waiting for response-buffer space. A host
    // that ticks only after recv_timeout has no idle interval inside this backlog.
    const REQUESTS: usize = 30_000;
    let timeout = Duration::from_secs(10);
    helper
        .send(Request::Parameters { slot: 0 }, timeout)
        .unwrap();
    for _ in 0..REQUESTS {
        helper.send(Request::Bypass { slot: 0 }, timeout).unwrap();
    }
    helper
        .send(Request::Parameters { slot: 0 }, timeout)
        .unwrap();
    let value = |response: Response, id| {
        let Response::Parameters(values) = response else {
            panic!("{response:?}");
        };
        values
            .into_iter()
            .find(|(info, _)| info.id == id)
            .unwrap()
            .1
    };
    let before = value(helper.receive(timeout).unwrap(), 17);
    for _ in 0..REQUESTS {
        let response = helper.receive(timeout).unwrap();
        assert!(matches!(response, Response::Bypass(_)), "{response:?}");
    }
    let after = value(helper.receive(timeout).unwrap(), 17);
    assert!(
        after > before,
        "timer did not run during queued requests: {before} -> {after}"
    );
}
