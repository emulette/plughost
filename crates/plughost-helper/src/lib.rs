//! Helper process that loads and runs plugins for plughost.
//!
//! Applications build a small executable whose `main` calls [`run`], ship it with the same
//! plughost version, and pass its path to the plughost crate. The application starts the helper
//! with `<scan|host> <socket name> <token> <parent pid>`; the helper connects back for control messages and
//! completion notifications. PCM and events use prepared shared memory. On Unix, host startup
//! takes a handle-transfer socket from stdin and restores null stdin before loading plugins.
//! Standard output remains available to plugins and is never used for protocol messages.
//!
//! An IPC thread receives requests and passes audio blocks to a processing thread, so a main
//! thread held up by a plugin (a modal dialog, a window being dragged) does not stall audio. The
//! processing thread owns the chain's processing state and also makes the audio-thread calls
//! plugins need between blocks. The main thread makes every other plugin call, runs the editors,
//! and runs the OS event loop between requests, since plugins' dialogs, timers, and editors
//! depend on it. Both threads record the slot they are calling in the application's activity
//! mapping.

mod calls;
mod editor;
mod errors;
mod events;
mod host;
mod lifetime;
mod presets;
mod process;
mod scan;

use std::io;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, SendHalf, Stream};
use plughost_core::internal::DiagnosticBuffer;
use plughost_core::ipc::shared::transfer;
use plughost_core::ipc::{
    Hello, MessageReader, MessageWriter, PROTOCOL_VERSION, Request, Response,
};
use plughost_core::{Failure, FailureKind};

use crate::errors::{CONNECT, USAGE};

/// The audit build counts the host's allocations on the processing thread. Unit tests count
/// allocations with their own.
#[cfg(all(feature = "realtime-audit", not(test)))]
#[global_allocator]
static ALLOCATOR: plughost_core::audit::Allocator = plughost_core::audit::Allocator;

/// How often the main thread services OS events, including while requests keep arriving.
const EVENT_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Scan,
    Host,
}

/// Runs the helper until the application disconnects or asks it to shut down.
pub fn run() -> ExitCode {
    abort_on_panic();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match args.as_slice() {
        [mode, name, token, parent] => {
            let mode = match mode.as_str() {
                "scan" => Some(Mode::Scan),
                "host" => Some(Mode::Host),
                _ => None,
            };
            mode.zip(u64::from_str_radix(token, 16).ok())
                .zip(parent.parse::<u32>().ok())
                .map(|((mode, token), parent)| (mode, name.as_str(), token, parent))
        }
        _ => None,
    };
    let Some((mode, name, token, parent)) = parsed else {
        errors::report(USAGE);
        return ExitCode::from(2);
    };
    if let Err(error) = lifetime::watch_parent(parent) {
        errors::report(&error.to_string());
        return ExitCode::FAILURE;
    }
    match serve(mode, name, token) {
        Ok(()) => {
            #[cfg(feature = "realtime-audit")]
            errors::report(&plughost_core::audit::report());
            ExitCode::SUCCESS
        }
        Err(error) => {
            errors::report(&format!("{CONNECT}: {error}"));
            ExitCode::FAILURE
        }
    }
}

/// Ends the whole helper on any panic, after the default report. A panic that unwound instead
/// would end one thread without a reply, clear the slot that thread recorded, and leave the
/// application waiting for a timeout that names no slot; a panic in a VST3 or Objective-C
/// callback cannot unwind at all. An abort keeps the slot recorded, so the application reports
/// a crash of it. clack-host catches a panic in a CLAP host callback and goes on, which this
/// replaces with the abort too: the state the panic left behind is not known to be sound.
fn abort_on_panic() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        report(info);
        std::process::abort();
    }));
}

fn serve(mode: Mode, name: &str, token: u64) -> io::Result<()> {
    let _runtime = events::init(mode == Mode::Host)?;
    let stream = Stream::connect(name.to_ns_name::<GenericNamespaced>()?)?;
    let (receive, send) = stream.split();
    let mut send = MessageWriter::new(send);
    send.write(&Hello {
        protocol: PROTOCOL_VERSION,
        token,
    })?;
    let responder = Responder {
        send: Arc::new(Mutex::new(send)),
        diagnostics: DiagnosticBuffer::default(),
    };

    let (requests, incoming) = mpsc::channel::<Request<'static>>();
    let mut host = None;
    let mut blocks = None;
    if mode == Mode::Host {
        let (processing, work) = process::Processing::spawn(responder.clone())?;
        host = Some(host::Host::new(
            transfer::Receiver::take_from_stdin()?,
            responder.clone(),
            processing,
        ));
        blocks = Some(work);
    }
    thread::spawn(move || {
        let mut receive = MessageReader::new(receive);
        while let Ok(request) = receive.read::<Request<'static>>() {
            let handled = match (request, &blocks) {
                (
                    Request::Process {
                        context,
                        submission,
                    },
                    Some(blocks),
                ) => blocks
                    .send(process::Work::Block {
                        context,
                        submission,
                    })
                    .is_ok(),
                (request, _) => requests.send(request).is_ok(),
            };
            if !handled {
                break;
            }
        }
        // This reader must terminate a modal/hung main thread when its owner disconnects.
        lifetime::disconnected();
    });

    let mut next_tick = Instant::now() + EVENT_INTERVAL;
    loop {
        if Instant::now() >= next_tick {
            events::with_pool(|| {
                events::pump();
                if let Some(host) = &mut host {
                    host.tick();
                }
            });
            next_tick = Instant::now() + EVENT_INTERVAL;
        }
        let request =
            match incoming.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
                Ok(Request::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Ok(request) => request,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
            };
        events::with_pool(|| handle(&mut host, request, &responder))?;
    }
}

/// Serves one request on the main thread.
fn handle(
    host: &mut Option<host::Host>,
    request: Request<'_>,
    responder: &Responder,
) -> io::Result<()> {
    match (host, request) {
        (
            None,
            Request::Scan {
                bundle,
                host,
                interactive,
            },
        ) => {
            if interactive {
                events::activate();
            }
            scan::scan(&bundle, &host, responder)?
        }
        (None, Request::PresetProviders { bundle, host }) => {
            presets::providers(&bundle, &host, responder)?
        }
        (
            None,
            Request::DiscoverPresets {
                bundle,
                host,
                target,
            },
        ) => presets::discover(&bundle, &host, &target, responder)?,
        (None, Request::ListAudioUnits) => scan::list_audio_units(responder)?,
        (Some(host), request) => responder.send(&host.handle(request))?,
        (None, _) => responder.fail(
            None,
            Failure::new(FailureKind::Protocol, errors::WRONG_MODE),
        )?,
    }
    Ok(())
}

/// Writes responses to the application, from the main thread and the processing thread. The
/// application sends one request at a time, so their responses do not interleave.
#[derive(Clone)]
pub(crate) struct Responder {
    send: Arc<Mutex<MessageWriter<SendHalf>>>,
    diagnostics: DiagnosticBuffer,
}

impl Responder {
    /// Writes one response. A failure is also kept for the application's diagnostics drain.
    pub fn send(&self, response: &Response) -> io::Result<()> {
        if let Response::Failed { slot, failure } = response {
            record_failure(&self.diagnostics, *slot, failure);
        }
        let mut send = self
            .send
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        send.write(response)
    }

    pub fn fail(&self, slot: Option<usize>, failure: Failure) -> io::Result<()> {
        self.send(&Response::Failed { slot, failure })
    }

    /// Keeps a warning about the helper itself for the application's diagnostics drain.
    pub fn warn(&self, message: &str) {
        self.diagnostics.record(
            plughost_core::DiagnosticSeverity::Warning,
            message,
            None,
            None,
            None,
        );
    }
}

pub(crate) fn record_failure(
    diagnostics: &DiagnosticBuffer,
    slot: Option<usize>,
    failure: &Failure,
) {
    diagnostics.record(
        plughost_core::DiagnosticSeverity::Error,
        &failure.message,
        None,
        slot,
        Some(failure.kind),
    );
}
