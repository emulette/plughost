//! The same executable acts as a deliberately faulty external helper or tests Chain against it.
//! No plugin behavior is mocked: this fixture exercises the process/protocol boundary itself.
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, Stream};
use plughost::{
    AudioBusConfig, AudioInputRoute, AudioSource, BlockContext, Chain, Events, FailureKind,
    HostIdentity, Layout, PluginFormat, PluginRef, RoutedChainConfig, SampleFormat,
    SlotAudioConfig, Timeouts,
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
    if args.get(1).map(String::as_str) == Some("--fault") {
        caller(&args[2]);
        return;
    }
    for fault in [
        "generation",
        "sequence",
        "frames",
        "crash",
        "hang",
        "prepare",
        "event-offset",
        "event-order",
        "event-note",
        "event-port",
        "event-count",
        "control-response",
        "control-crash",
        "control-hang",
        "failure-slot",
        "rejection-slot",
        "activity-slot",
        "loaded-count",
        "reprepare-timeout",
        "reset-timeout",
        "render-targets",
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
    let mut activity_map = None;
    let mut input = Vec::new();
    let mut edits = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut events = Events::for_block();
    let mut blocks = 0;
    let mut preparations = 0;
    let mut validations = 0;
    while let Ok(request) = read.read::<Request>() {
        match request {
            Request::Load {
                plugins, activity, ..
            } => {
                // SAFETY: Chain transferred its activity mapping once, before this request.
                activity_map = Some(unsafe { receiver.receive_activity(activity) }.unwrap());
                fault = plugins[0].id().class_id().to_string();
                if fault == "loaded-count" {
                    write.write(&Response::Loaded(vec![])).unwrap();
                    continue;
                }
                write
                    .write(&Response::Loaded(vec![{
                        let mut plugin_info = PluginInfo::new(
                            plughost::PluginId::new(PluginFormat::Clap, fault.clone()),
                            "Protocol boundary fixture",
                            PluginKind::Effect,
                        );
                        plugin_info.sdk_version = std::process::id().to_string();
                        plugin_info
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
                preparations += 1;
                if preparations == 2 && fault == "reprepare-timeout" {
                    std::thread::sleep(Duration::from_millis(300));
                }
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
                let output_events = if blocks == 2 {
                    use plughost::{Event, EventData, Note};
                    match fault.as_str() {
                        "event-offset" => {
                            Events::from_iter([Event::note_on(context.frames, 0, 60, 100)])
                        }
                        "event-order" => Events::from_iter([
                            Event::note_on(20, 0, 60, 100),
                            Event::note_off(2, 0, 60, 0),
                        ]),
                        "event-note" => Events::from_iter([Event::new(
                            0,
                            EventData::NoteOn(Note::new(0, 60, f64::NAN)),
                        )]),
                        "event-port" => {
                            Events::from_iter([Event::note_on(0, 0, 60, 100).on_port(1)])
                        }
                        _ => Events::new(),
                    }
                } else {
                    Events::new()
                };
                memory
                    .write_output_f32(submission, &[&input[0], &input[1]], &output_events)
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
                        events: if blocks == 2 && fault == "event-count" {
                            MAX_BLOCK_EVENTS + 1
                        } else {
                            output_events.len()
                        },
                        submission: completed,
                        latency: 0,
                        tail: plughost_core::render::Tail::Samples(0),
                    })
                    .unwrap();
            }
            Request::Reset => {
                if fault == "reset-timeout" {
                    std::thread::sleep(Duration::from_millis(300));
                }
                if write
                    .write(&Response::Timing {
                        latency: 0,
                        tail: plughost::Tail::Samples(0),
                        latency_changed: false,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Request::Timing => {
                write
                    .write(&Response::Timing {
                        latency: 0,
                        tail: plughost::Tail::Samples(0),
                        latency_changed: false,
                    })
                    .unwrap();
            }
            Request::Parameters { .. } => {
                assert_ne!(fault, "render-targets", "render must not download metadata");
                let response = match fault.as_str() {
                    "control-crash" => std::process::exit(17),
                    "activity-slot" => {
                        activity_map
                            .as_ref()
                            .unwrap()
                            .enter(plughost_core::ipc::shared::Caller::Main, 999);
                        std::process::exit(17);
                    }
                    "control-hang" => loop {
                        std::thread::park();
                    },
                    "failure-slot" => Response::Failed {
                        slot: Some(999),
                        failure: plughost::Failure::new(
                            FailureKind::Unsupported,
                            "boundary fixture failure",
                        ),
                    },
                    "rejection-slot" => Response::Rejected {
                        slot: Some(999),
                        error: plughost::InputError::Automation,
                    },
                    _ => Response::Done,
                };
                write.write(&response).unwrap();
            }
            Request::ValidateAutomation { targets } => {
                assert_eq!(fault, "render-targets");
                validations += 1;
                assert_eq!(validations, 1);
                assert_eq!(blocks, 0, "all targets must be checked before DSP");
                assert_eq!(targets, [(0, 0), (0, 1), (0, 77)]);
                write
                    .write(&Response::Rejected {
                        slot: Some(0),
                        error: plughost::InputError::UnknownParameter { id: 77 },
                    })
                    .unwrap();
            }
            Request::Shutdown => return,
            _ => panic!("unexpected request: {request:?}"),
        }
    }
}

fn caller(fault: &str) {
    let timeout = {
        let mut timeouts = Timeouts::default();
        timeouts.process = Duration::from_millis(150);
        timeouts.control = Duration::from_millis(150);
        timeouts.load = Duration::from_secs(2);
        timeouts
    };
    let result = Chain::spawn(
        &std::env::current_exe().unwrap(),
        &[PluginRef::clap("fault.clap", fault).into()],
        &plughost::SpawnOptions::new(HostIdentity::default(), timeout),
    );
    if fault == "loaded-count" {
        assert!(matches!(result, Err(plughost::Error::Protocol)));
        return;
    }
    let mut chain = result.unwrap();
    let stereo = AudioBusConfig::new(0, Layout::Stereo, true);
    let config = RoutedChainConfig::new(
        48_000.0,
        32,
        SampleFormat::F32,
        vec![Layout::Stereo],
        vec![
            SlotAudioConfig::new(
                vec![AudioInputRoute::new(
                    stereo,
                    AudioSource::External { bus: 0 },
                )],
                vec![stereo],
            )
            .with_events(plughost::SlotEventConfig::new(vec![], vec![0])),
        ],
    );
    let pid: u32 = chain.plugins()[0].sdk_version.parse().unwrap();
    if fault == "prepare" {
        assert_eq!(
            chain.prepare_audio(&config).unwrap_err().kind(),
            FailureKind::Protocol
        );
    } else {
        chain.prepare_audio(&config).unwrap();
        if fault == "render-targets" {
            let points = [(0, 0), (0, 1), (1, 32), (0, 64), (77, 95)].map(|(id, offset)| {
                plughost::AutomationEvent::new(0, plughost::ParameterChange::new(id, offset, 0.5))
            });
            let input = [1.0f32; 96];
            let error = plughost::render_with_schedule(
                &mut chain,
                &[&input, &input],
                input.len(),
                &Events::new(),
                &plughost::RenderOptions::new(plughost::TailPolicy::Reported, 0.0),
                &plughost::RenderSchedule::default().with_automation(&points),
            )
            .unwrap_err();
            assert!(
                matches!(
                    error,
                    plughost::Error::Input {
                        slot: Some(0),
                        error: plughost::InputError::UnknownParameter { id: 77 }
                    }
                ),
                "{error:?}"
            );
            assert!(chain.helper_monitor().exit_status().is_none());
            return;
        }
        if fault == "reprepare-timeout" {
            // Repreparation reloads plugins, so this response may take longer than control.
            chain.reprepare().unwrap();
            return;
        }
        if fault == "reset-timeout" {
            assert!(matches!(
                chain.reset(),
                Err(plughost::Error::TimedOut { .. })
            ));
            assert!(
                chain
                    .helper_monitor()
                    .wait_for_exit(Duration::from_secs(1))
                    .is_some()
            );
            return;
        }
        let input = [0.25; 32];
        let (mut left, mut right) = ([42.0; 32], [-42.0; 32]);
        chain
            .process_audio_f32(
                &BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &Events::new(),
                &mut Events::for_block(),
            )
            .unwrap();
        assert_eq!(left, input);
        assert_eq!(right, input);
        left.fill(42.0);
        right.fill(-42.0);
        let start = Instant::now();
        let previous_events = Events::from_iter([plughost::Event::note_on(0, 0, 42, 80)]);
        let mut produced = previous_events.clone();
        let error = if fault.starts_with("control-") || fault.ends_with("-slot") {
            chain.parameters(0).map(|_| ())
        } else {
            chain.process_audio_f32(
                &BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &Events::new(),
                &mut produced,
            )
        }
        .unwrap_err();
        assert_eq!(
            error.kind(),
            match fault {
                "crash" | "control-crash" => FailureKind::Crashed,
                "hang" | "control-hang" => FailureKind::TimedOut,
                _ => FailureKind::Protocol,
            },
            "{fault}"
        );
        assert!(start.elapsed() < Duration::from_secs(5), "{fault}");
        assert_eq!(left, [42.0; 32], "{fault}");
        assert_eq!(right, [-42.0; 32], "{fault}");
        assert_eq!(produced, previous_events, "{fault}");
        assert!(
            matches!(
                plughost::render(
                    &mut chain,
                    &[&input, &input],
                    32,
                    &Events::new(),
                    &plughost::RenderOptions::default()
                ),
                Err(plughost::Error::NotPrepared)
            ),
            "{fault}"
        );
        assert_eq!(
            chain
                .process_audio_f32(
                    &BlockContext::new(32),
                    &[&input, &input],
                    &mut [&mut left, &mut right],
                    &[],
                    &Events::new(),
                    &mut Events::for_block(),
                )
                .unwrap_err()
                .kind(),
            FailureKind::NotPrepared
        );
        // Every later request reports the lost helper.
        assert_eq!(
            chain.parameters(0).map(|_| ()).unwrap_err().kind(),
            FailureKind::Crashed,
            "{fault}"
        );
        let initial: Vec<plughost::ChainSlot> = chain
            .references()
            .iter()
            .cloned()
            .map(plughost::ChainSlot::from)
            .collect();
        chain = chain.recover(&initial).unwrap();
        assert_ne!(chain.plugins()[0].sdk_version.parse::<u32>().unwrap(), pid);
        chain
            .process_audio_f32(
                &BlockContext::new(32),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &Events::new(),
                &mut Events::for_block(),
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
