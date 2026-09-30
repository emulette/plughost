//! The same executable acts as a deliberately faulty external helper or tests Chain against it.
//! No plugin behavior is mocked: this fixture exercises the process/protocol boundary itself.
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, Stream};
use plughost::{
    AudioBusConfig, AudioInputRoute, AudioSource, BlockContext, Chain, ChannelAdaptation,
    FailureKind, HostIdentity, Layout, PluginFormat, PluginRef, ProcessMode, RoutedChainConfig,
    SampleFormat, SlotAudioConfig, Timeouts,
};
use plughost_core::ipc::shared::{SharedAudio, transfer};
use plughost_core::ipc::{
    AudioSamples, Hello, MessageReader, MessageWriter, PROTOCOL_VERSION, Request, Response,
};
use plughost_core::{MAX_BLOCK_EVENTS, PluginInfo, PluginKind};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("host") {
        peer(&args[2], u64::from_str_radix(&args[3], 16).unwrap());
        return;
    }
    for fault in [
        "generation",
        "sequence",
        "frames",
        "crash",
        "hang",
        "prepare",
    ] {
        caller(fault);
    }
}

fn peer(name: &str, token: u64) {
    let receiver = transfer::Receiver::take_from_stdin().unwrap();
    let stream = Stream::connect(name.to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
    let (read, write) = stream.split();
    let mut read = MessageReader::new(read);
    let mut write = MessageWriter::new(write);
    write
        .write(&Hello {
            protocol: PROTOCOL_VERSION,
            token,
        })
        .unwrap();
    let mut fault = String::new();
    let mut memory: Option<SharedAudio> = None;
    let mut input = Vec::new();
    let mut edits = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut events = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut blocks = 0;
    while let Ok(request) = read.read::<Request>() {
        match request {
            Request::Load {
                plugins, activity, ..
            } => {
                // SAFETY: Chain transferred its activity mapping once, before this request.
                unsafe { receiver.receive_activity(activity) }.unwrap();
                fault = plugins[0].class_id.clone();
                write
                    .write(&Response::Loaded(vec![PluginInfo {
                        format: PluginFormat::Clap,
                        class_id: fault.clone(),
                        name: "Protocol boundary fixture".into(),
                        vendor: String::new(),
                        version: String::new(),
                        sdk_version: std::process::id().to_string(),
                        kind: PluginKind::Effect,
                        categories: vec![],
                    }]))
                    .unwrap();
            }
            Request::PrepareAudio {
                memory: descriptor,
                handle,
                ..
            } => {
                // SAFETY: Chain transferred this exact descriptor/handle once through its private channel.
                memory = Some(unsafe { receiver.receive(descriptor, handle) }.unwrap());
                let AudioSamples::F32(storage) = descriptor.config.input_storage().unwrap() else {
                    unreachable!()
                };
                input = storage;
                write
                    .write(&Response::AudioPrepared {
                        generation: descriptor.generation + u64::from(fault == "prepare"),
                        buses: vec![Vec::new()],
                        latency: 0,
                        tail: plughost_core::render::Tail::Samples(0),
                    })
                    .unwrap();
            }
            Request::Process {
                context,
                submission,
            } => {
                let memory = memory.as_mut().unwrap();
                memory
                    .read_input_f32(submission, &mut input, &mut edits, &mut events)
                    .unwrap();
                blocks += 1;
                if blocks == 2 && fault == "crash" {
                    std::process::exit(17);
                }
                if blocks == 2 && fault == "hang" {
                    loop {
                        std::thread::park();
                    }
                }
                assert_eq!(context.frames, input[0].len());
                memory
                    .write_output_f32(submission, &[&input[0], &input[1]], &[])
                    .unwrap();
                let mut completed = submission;
                if blocks == 2 {
                    match fault.as_str() {
                        "generation" => completed.generation += 1,
                        "sequence" => completed.sequence -= 1,
                        "frames" => completed.frames += 1,
                        _ => {}
                    }
                }
                write
                    .write(&Response::Processed {
                        events: 0,
                        submission: completed,
                        latency: 0,
                        tail: plughost_core::render::Tail::Samples(0),
                    })
                    .unwrap();
            }
            Request::Shutdown => return,
            _ => panic!("unexpected request: {request:?}"),
        }
    }
}

fn caller(fault: &str) {
    let timeout = Timeouts {
        process: Duration::from_millis(150),
        ..Timeouts::default()
    };
    let mut chain = Chain::spawn(
        &std::env::current_exe().unwrap(),
        &[PluginRef {
            format: PluginFormat::Clap,
            bundle: None,
            class_id: fault.into(),
        }],
        &HostIdentity::default(),
        timeout,
    )
    .unwrap();
    let stereo = AudioBusConfig {
        id: 0,
        layout: Layout::Stereo,
        active: true,
    };
    let config = RoutedChainConfig {
        event_inputs: 0,
        sample_rate: 48_000.0,
        max_block_size: 32,
        sample_format: SampleFormat::F32,
        mode: ProcessMode::Offline,
        inputs: vec![Layout::Stereo],
        slots: vec![SlotAudioConfig {
            events: Default::default(),
            configuration: None,
            inputs: vec![AudioInputRoute {
                bus: stereo,
                source: AudioSource::External { bus: 0 },
                adaptation: ChannelAdaptation::Exact,
            }],
            outputs: vec![stereo],
        }],
    };
    let pid: u32 = chain.plugins()[0].sdk_version.parse().unwrap();
    if fault == "prepare" {
        assert_eq!(
            chain.prepare_audio(&config).unwrap_err().kind(),
            FailureKind::Protocol
        );
    } else {
        chain.prepare_audio(&config).unwrap();
        let input = [0.25; 32];
        let (mut left, mut right) = ([42.0; 32], [-42.0; 32]);
        chain
            .process_audio_f32(
                &BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(left, input);
        assert_eq!(right, input);
        left.fill(42.0);
        right.fill(-42.0);
        let start = Instant::now();
        let error = chain
            .process_audio_f32(
                &BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap_err();
        assert_eq!(
            error.kind(),
            match fault {
                "crash" => FailureKind::Crashed,
                "hang" => FailureKind::TimedOut,
                _ => FailureKind::Protocol,
            },
            "{fault}"
        );
        assert!(start.elapsed() < Duration::from_secs(5), "{fault}");
        assert_eq!(left, [42.0; 32], "{fault}");
        assert_eq!(right, [-42.0; 32], "{fault}");
        assert_eq!(
            chain
                .process_audio_f32(
                    &BlockContext::new(32),
                    &[&input, &input],
                    &mut [&mut left, &mut right],
                    &[],
                    &[],
                    &mut Vec::new(),
                )
                .unwrap_err()
                .kind(),
            FailureKind::NotPrepared
        );
        chain = chain.recover(&[]).unwrap();
        assert_ne!(chain.plugins()[0].sdk_version.parse::<u32>().unwrap(), pid);
        chain
            .process_audio_f32(
                &BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(left, input);
        assert_eq!(right, input);
    }
    #[cfg(unix)]
    assert!(
        !std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success(),
        "unreaped helper for {fault}"
    );
}
