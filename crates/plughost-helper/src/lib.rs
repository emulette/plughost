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
#[cfg(target_os = "macos")]
mod remote;
mod scan;

use std::io;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
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
    /// Host mode for remote editors (macOS): the helper never activates itself.
    #[cfg(target_os = "macos")]
    Remote,
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
                #[cfg(target_os = "macos")]
                "remote" => Some(Mode::Remote),
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
    let _runtime = events::init(mode != Mode::Scan)?;
    // Before any plugin loads, so none sees the helper activate or inactive.
    #[cfg(target_os = "macos")]
    if mode == Mode::Remote
        && let Some(mtm) = objc2::MainThreadMarker::new()
    {
        remote::enter();
        remote::input::install(mtm);
    }
    let stream = Stream::connect(name.to_ns_name::<GenericNamespaced>()?)?;
    let (receive, send) = stream.split();
    let mut send = MessageWriter::new(send);
    send.write(&Hello {
        protocol: PROTOCOL_VERSION,
        token,
    })?;
    let responder = Responder {
        send: Arc::new(Mutex::new(send)),
        waiting: Arc::new(Mutex::new(Waiting {
            requests: 0,
            since: Instant::now(),
        })),
        diagnostics: DiagnosticBuffer::default(),
    };
    #[cfg(target_os = "macos")]
    if mode == Mode::Remote {
        let responder = responder.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(KEEP_ALIVE / 4);
                if responder.keep_alive().is_err() {
                    break;
                }
            }
        });
    }

    let (requests, incoming) = mpsc::channel::<Request<'static>>();
    let mut host = None;
    let mut blocks = None;
    let mut transfers = None;
    if mode != Mode::Scan {
        let (processing, work) = process::Processing::spawn(responder.clone())?;
        #[cfg(target_os = "macos")]
        let remote = mode == Mode::Remote;
        #[cfg(target_os = "windows")]
        let remote = false;
        let transfer = Arc::new(transfer::Receiver::take_from_stdin()?);
        transfers = Some(Arc::clone(&transfer));
        host = Some(host::Host::new(
            transfer,
            responder.clone(),
            processing,
            remote,
        ));
        blocks = Some(work);
    }
    let reader = responder.clone();
    // Only a remote-mode helper keeps the requests its main thread holds alive.
    #[cfg(target_os = "macos")]
    let counted = mode == Mode::Remote;
    #[cfg(target_os = "windows")]
    let counted = false;
    // A remote-mode helper starts its sessions on this reader.
    let sessions = transfers.filter(|_| counted);
    thread::spawn(move || {
        let responder = reader;
        let mut receive = MessageReader::new(receive);
        while let Ok(request) = receive.read::<Request<'static>>() {
            let handled = match (request, &blocks, &sessions) {
                (
                    Request::Process {
                        context,
                        submission,
                    },
                    Some(blocks),
                    _,
                ) => blocks
                    .send(process::Work::Block {
                        context,
                        submission,
                    })
                    .is_ok(),
                // Not through the main thread, which a plugin's dialog or menu may hold.
                #[cfg(target_os = "macos")]
                (
                    Request::RemoteSession {
                        commands,
                        notices,
                        menu_timeout_ms,
                    },
                    _,
                    Some(transfer),
                ) => {
                    let response = remote::session::open(
                        transfer,
                        commands,
                        notices,
                        Duration::from_millis(menu_timeout_ms),
                    );
                    responder.send(&response).is_ok()
                }
                (request, _, _) => {
                    if counted {
                        responder.begin();
                    }
                    let sent = requests.send(request).is_ok();
                    events::wake();
                    sent
                }
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
        // macOS waits on the main run loop, which the reader wakes, so run loop sources, timers
        // and the main queue run while the helper waits.
        #[cfg(target_os = "macos")]
        let request = match incoming.try_recv() {
            Ok(Request::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            Ok(request) => request,
            Err(mpsc::TryRecvError::Empty) => {
                events::with_pool(|| {
                    events::wait(next_tick.saturating_duration_since(Instant::now()));
                });
                continue;
            }
        };
        #[cfg(target_os = "windows")]
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
        (Some(host), request) => responder.answer(&host.handle(request))?,
        (None, _) => responder.fail(
            None,
            Failure::new(FailureKind::Protocol, errors::WRONG_MODE),
        )?,
    }
    Ok(())
}

/// How long a request the main thread holds goes without a message to the application before a
/// remote-mode helper keeps it alive. The helper checks four times as often, so the application
/// hears at most a quarter of it later.
#[cfg(target_os = "macos")]
const KEEP_ALIVE: Duration = Duration::from_secs(1);

/// The requests a remote-mode helper's main thread has yet to answer, and since when it works on
/// the first of them or last kept them alive. It is apart from the writer: the reader thread
/// records requests while the main thread may wait to write a response the application does not
/// read yet.
struct Waiting {
    requests: usize,
    since: Instant,
}

/// Writes responses to the application, from the main thread and the processing thread. The
/// application sends one request at a time, so their responses do not interleave.
#[derive(Clone)]
pub(crate) struct Responder {
    send: Arc<Mutex<MessageWriter<SendHalf>>>,
    waiting: Arc<Mutex<Waiting>>,
    diagnostics: DiagnosticBuffer,
}

impl Responder {
    /// Writes one response. A failure is also kept for the application's diagnostics drain.
    pub fn send(&self, response: &Response) -> io::Result<()> {
        if let Response::Failed { slot, failure } = response {
            record_failure(&self.diagnostics, *slot, failure);
        }
        self.writer().write(response)
    }

    /// Writes the main thread's answer to a request, which it then no longer holds.
    fn answer(&self, response: &Response) -> io::Result<()> {
        if let Response::Failed { slot, failure } = response {
            record_failure(&self.diagnostics, *slot, failure);
        }
        let mut writer = self.writer();
        let written = writer.write(response);
        let mut waiting = self.waiting();
        waiting.requests = waiting.requests.saturating_sub(1);
        waiting.since = Instant::now();
        written
    }

    fn writer(&self) -> std::sync::MutexGuard<'_, MessageWriter<SendHalf>> {
        self.send.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn waiting(&self) -> std::sync::MutexGuard<'_, Waiting> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records that the main thread has a request to answer.
    fn begin(&self) {
        let mut waiting = self.waiting();
        if waiting.requests == 0 {
            waiting.since = Instant::now();
        }
        waiting.requests += 1;
    }

    /// Keeps a request the main thread has held for a while alive while its run loop runs, in
    /// a dialog's, a menu's or a panel's loop. A main thread held up outside any loop sends
    /// nothing, and the application's timeout ends the helper as before.
    #[cfg(target_os = "macos")]
    fn keep_alive(&self) -> io::Result<()> {
        // The writer first, as `answer` takes them: an answer written meanwhile is seen.
        let mut writer = self.writer();
        let held = {
            let waiting = self.waiting();
            waiting.requests > 0 && waiting.since.elapsed() >= KEEP_ALIVE
        };
        if held && remote::watch::running_within(KEEP_ALIVE) {
            writer.write(&Response::Progress)?;
            self.waiting().since = Instant::now();
        }
        Ok(())
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
