//! Starting and supervising one helper process.

mod listener;
mod process;
#[cfg(test)]
mod scan_benchmark;
#[cfg(test)]
mod tests;
mod transport;
pub use process::{ChainCanceller, HelperMonitor};

use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use plughost_core::PluginRef;
use plughost_core::ipc::shared::{Activity, Caller, transfer};
use plughost_core::ipc::{PROTOCOL_VERSION, Request, Response};
use plughost_core::{FailureKind, HostIdentity};

use crate::errors::Error;
use transport::{Incoming, Stopped, Transport, Unqueued};

/// Time for the helper process to start and connect back.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// Time for the helper to exit after being asked to shut down.
const EXIT_TIMEOUT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(5);

#[derive(Clone, Copy)]
pub(crate) enum Mode {
    Scan,
    Host,
}

pub(crate) struct Helper {
    child: process::Process,
    transport: Transport,
    /// Host mode: the slot each helper thread is calling, and the transfer channel for mappings.
    shared: Option<Shared>,
    /// Whether the outstanding request runs on the helper's processing thread.
    processing: bool,
    /// Whether a call reported the helper's end, after which a cancellation reads as an exit.
    end_reported: bool,
}

struct Shared {
    activity: Activity,
    sender: transfer::Sender,
    slots: usize,
}

fn random() -> u64 {
    // RandomState is seeded from the OS random source and differs per instance.
    RandomState::new().hash_one((std::process::id(), Instant::now()))
}

impl Helper {
    pub fn spawn(path: &Path, mode: Mode) -> Result<Helper, Error> {
        // The non-cancellable entry point cannot produce a cancelled result.
        Self::spawn_controlled(path, mode, &|| false).map(Option::unwrap)
    }

    /// Cancellation reaps a child even before connection/handshake completion.
    pub fn spawn_controlled(
        path: &Path,
        mode: Mode,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Helper>, Error> {
        if cancelled() {
            return Ok(None);
        }
        let (shared, stdin) = match mode {
            Mode::Host => {
                let (sender, stdin) = transfer::Sender::new().map_err(Error::HelperStart)?;
                let activity = Activity::new().map_err(Error::HelperStart)?;
                (
                    Some(Shared {
                        activity,
                        sender,
                        slots: 0,
                    }),
                    stdin,
                )
            }
            Mode::Scan => (None, Stdio::null()),
        };
        let name = format!("plughost-{:016x}{:016x}.sock", random(), random());
        let token = random();
        let mut listener = listener::Listener::new(&name).map_err(Error::HelperStart)?;
        let mut command = Command::new(path);
        if cancelled() {
            return Ok(None);
        }
        command
            .arg(match mode {
                Mode::Scan => "scan",
                Mode::Host => "host",
            })
            .arg(&name)
            .arg(format!("{token:x}"))
            .arg(std::process::id().to_string())
            .stdin(stdin)
            // Plugins print to standard output; the application may use its own for data.
            .stdout(Stdio::null())
            // Pre-handshake errors and arbitrary plugin stderr remain available to the app.
            .stderr(Stdio::inherit());
        let child = process::Process::spawn(&mut command).map_err(Error::HelperStart)?;

        let deadline = Instant::now() + START_TIMEOUT;
        let stream = loop {
            if cancelled() {
                let _ = child.kill(FailureKind::Cancelled);
                return Ok(None);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            match listener.wait(remaining.min(POLL)) {
                Ok(true) => {
                    break listener.accept().map_err(|error| {
                        let _ = child.kill(FailureKind::Host);
                        Error::HelperStart(error)
                    })?;
                }
                Ok(false) => {
                    if let Ok(Some(status)) = child.try_wait() {
                        return Err(Error::Crashed {
                            slot: None,
                            status: Some(status),
                        });
                    }
                    if Instant::now() > deadline {
                        let _ = child.kill(FailureKind::TimedOut);
                        return Err(Error::TimedOut { slot: None });
                    }
                }
                Err(error) => {
                    let _ = child.kill(FailureKind::Host);
                    return Err(Error::HelperStart(error));
                }
            }
        };
        let transport = match stream
            .set_nonblocking(false)
            .and_then(|()| Transport::new(stream))
        {
            Ok(transport) => transport,
            Err(error) => {
                let _ = child.kill(FailureKind::Host);
                return Err(Error::HelperStart(error));
            }
        };
        let mut helper = Helper {
            child,
            transport,
            shared,
            processing: false,
            end_reported: false,
        };
        let error = match helper.message_controlled(START_TIMEOUT, cancelled)? {
            Some(Incoming::Hello(hello)) if hello.token == token => {
                if hello.protocol == PROTOCOL_VERSION {
                    return Ok(Some(helper));
                }
                Error::ProtocolVersion {
                    application: PROTOCOL_VERSION,
                    helper: hello.protocol,
                }
            }
            None => return Ok(None),
            _ => Error::Handshake,
        };
        helper.kill(FailureKind::Protocol);
        Err(error)
    }

    /// Sends a request and waits for its reply. `timeout` bounds writing the request and the wait
    /// for each message, so progress messages keep a long operation alive.
    pub fn request(&mut self, request: Request<'_>, timeout: Duration) -> Result<Response, Error> {
        let slot = request.slot();
        if !self.valid_slot(slot) {
            return Err(Error::Input {
                slot,
                error: plughost_core::InputError::Slot,
            });
        }
        self.send(request, timeout)?;
        let response = loop {
            match self.receive(timeout)? {
                Response::Progress => {}
                response => break response,
            }
        };
        match response {
            Response::Failed { slot, failure } => {
                self.check_slot(slot)?;
                if failure.kind == FailureKind::Protocol {
                    self.kill(FailureKind::Protocol);
                    return Err(Error::Protocol);
                }
                Err(Error::Operation { slot, failure })
            }
            Response::Rejected { slot, error } => {
                self.check_slot(slot)?;
                Err(Error::Input { slot, error })
            }
            response => Ok(response),
        }
    }

    fn valid_slot(&self, slot: Option<usize>) -> bool {
        slot.is_none_or(|slot| {
            self.shared
                .as_ref()
                .is_none_or(|shared| slot < shared.slots)
        })
    }

    fn check_slot(&mut self, slot: Option<usize>) -> Result<(), Error> {
        if !self.valid_slot(slot) {
            self.kill(FailureKind::Protocol);
            return Err(Error::Protocol);
        }
        Ok(())
    }

    /// Host mode: loads the chain, handing the helper the mapping it announces calls through.
    pub fn load(
        &mut self,
        host: &HostIdentity,
        plugins: &[PluginRef],
        timeout: Duration,
    ) -> Result<Response, Error> {
        self.shared.as_mut().ok_or(Error::Protocol)?.slots = plugins.len();
        let activity =
            self.transfer(|sender, activity, child| sender.send_activity(activity, child))?;
        self.request(
            Request::Load {
                host: host.clone(),
                plugins: plugins.to_vec(),
                activity,
            },
            timeout,
        )
    }

    pub fn prepare_shared(
        &mut self,
        memory: &plughost_core::ipc::shared::SharedAudio,
        request: impl FnOnce(u64) -> Request<'static>,
        timeout: Duration,
    ) -> Result<Response, Error> {
        let handle = self.transfer(|sender, _, child| sender.send(memory, child))?;
        // Connection errors/deadlines reap the child, including unclaimed transferred handles.
        // A normal rejected prepare consumes its transferred file before returning an error.
        self.request(request(handle), timeout)
    }

    /// Transfers a realtime ring's mappings and doorbell, then sends the request naming them.
    pub fn install_ring(
        &mut self,
        ring: &plughost_core::ipc::shared::realtime::Ring,
        request: impl FnOnce(Vec<u64>) -> Request<'static>,
        timeout: Duration,
    ) -> Result<Response, Error> {
        let tokens = self.transfer(|sender, _, child| sender.send_ring(ring, child))?;
        // As in `prepare_shared`: the helper takes every handle before it answers.
        self.request(request(tokens), timeout)
    }

    fn transfer<T>(
        &mut self,
        send: impl FnOnce(&transfer::Sender, &Activity, &std::process::Child) -> io::Result<T>,
    ) -> Result<T, Error> {
        let shared = self.shared.as_ref().ok_or(Error::Protocol)?;
        match self
            .child
            .with_child(|child| send(&shared.sender, &shared.activity, child))
        {
            Ok(handle) => Ok(handle),
            Err(_) => Err(self.ended()),
        }
    }

    pub fn monitor(&self) -> HelperMonitor {
        self.child.monitor()
    }

    pub fn canceller(&self) -> ChainCanceller {
        self.child.canceller()
    }

    /// Writes a request within `timeout`. A request not written in time leaves the connection
    /// mid-frame, so the helper is terminated.
    pub fn send(&mut self, request: Request<'_>, timeout: Duration) -> Result<(), Error> {
        self.send_controlled(request, timeout, &|| false)
            .map(Option::unwrap)
    }

    /// Writes a request within `timeout`, polling cancellation, which reaps the child.
    pub fn send_controlled(
        &mut self,
        request: Request<'_>,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<()>, Error> {
        if cancelled() {
            self.kill(FailureKind::Cancelled);
            return Ok(None);
        }
        match self.queue(&request) {
            Ok(()) => {}
            // The helper is fine; the request is not one this crate should have made.
            Err(Unqueued::Unencodable) => {
                self.kill(FailureKind::Protocol);
                return Err(Error::Protocol);
            }
            Err(Unqueued::Closed) => return self.ended_controlled(cancelled).map_or(Ok(None), Err),
        }
        let started = Instant::now();
        loop {
            if cancelled() {
                self.kill(FailureKind::Cancelled);
                return Ok(None);
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            match self.transport.flush(remaining.min(POLL)) {
                Ok(()) => return Ok(Some(())),
                Err(Stopped::TimedOut) if started.elapsed() < timeout => continue,
                Err(Stopped::TimedOut) => {
                    self.kill(FailureKind::TimedOut);
                    return Err(Error::TimedOut {
                        slot: self.culprit()?,
                    });
                }
                Err(Stopped::Closed) => {
                    return self.ended_controlled(cancelled).map_or(Ok(None), Err);
                }
            }
        }
    }

    fn queue(&mut self, request: &Request<'_>) -> Result<(), Unqueued> {
        self.processing = matches!(request, Request::Process { .. });
        #[cfg(target_os = "windows")]
        if matches!(
            &request,
            Request::OpenEditor { .. }
                | Request::Scan {
                    interactive: true,
                    ..
                }
        ) {
            // Windows may deny this if the application itself is not in the foreground.
            // The editor still opens; an interactive caller can grant permission on a later try.
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                    self.child.id(),
                )
            };
        }
        self.transport.queue(request)
    }

    pub fn receive(&mut self, timeout: Duration) -> Result<Response, Error> {
        match self.message(timeout)? {
            Incoming::Response(response) => Ok(response),
            Incoming::Hello(_) => {
                self.kill(FailureKind::Protocol);
                Err(Error::Protocol)
            }
        }
    }

    /// Polls supervision without changing the stall deadline. Cancellation is distinct from
    /// timeout or plugin failure and always reaps the child before returning `None`.
    pub fn receive_controlled(
        &mut self,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Response>, Error> {
        match self.message_controlled(timeout, cancelled)? {
            Some(Incoming::Response(response)) => Ok(Some(response)),
            Some(Incoming::Hello(_)) => {
                self.kill(FailureKind::Protocol);
                Err(Error::Protocol)
            }
            None => Ok(None),
        }
    }

    /// The slot the helper was calling when it died or stopped responding. The thread serving
    /// the outstanding request is asked first; the other thread may hold up that request, or
    /// crash on its own, as the main thread can between requests. Slots are known only as far as
    /// the helper recorded them, so a crash on a plugin-owned thread names no slot.
    fn culprit(&mut self) -> Result<Option<usize>, Error> {
        let Some(shared) = &self.shared else {
            return Ok(None);
        };
        let activity = &shared.activity;
        let (first, second) = if self.processing {
            (Caller::Processing, Caller::Main)
        } else {
            (Caller::Main, Caller::Processing)
        };
        let (first, second) = (activity.slot(first), activity.slot(second));
        if !self.valid_slot(first) || !self.valid_slot(second) {
            // Later requests report the exit, not this mapping again.
            self.kill(FailureKind::Protocol);
            self.shared = None;
            return Err(Error::Protocol);
        }
        Ok(first.or(second))
    }

    fn message_controlled(
        &mut self,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Incoming>, Error> {
        let started = Instant::now();
        loop {
            if cancelled() {
                self.kill(FailureKind::Cancelled);
                return Ok(None);
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            match self.transport.receive(remaining.min(POLL)) {
                Ok(Ok(message)) => return Ok(Some(message)),
                Err(Stopped::TimedOut) if started.elapsed() < timeout => continue,
                Err(Stopped::TimedOut) => {
                    self.kill(FailureKind::TimedOut);
                    return Err(Error::TimedOut {
                        slot: self.culprit()?,
                    });
                }
                Ok(Err(error)) if error.kind() == io::ErrorKind::InvalidData => {
                    self.kill(FailureKind::Protocol);
                    return Err(Error::Protocol);
                }
                _ => return self.ended_controlled(cancelled).map_or(Ok(None), Err),
            }
        }
    }

    fn message(&mut self, timeout: Duration) -> Result<Incoming, Error> {
        match self.transport.receive(timeout) {
            Ok(Ok(message)) => Ok(message),
            Err(Stopped::TimedOut) => {
                self.kill(FailureKind::TimedOut);
                Err(Error::TimedOut {
                    slot: self.culprit()?,
                })
            }
            Ok(Err(error)) if error.kind() == io::ErrorKind::InvalidData => {
                self.kill(FailureKind::Protocol);
                Err(Error::Protocol)
            }
            _ => Err(self.ended()),
        }
    }

    /// The connection closed: the helper exited, or is about to.
    fn ended(&mut self) -> Error {
        self.ended_controlled(&|| false).unwrap()
    }

    fn ended_controlled(&mut self, cancelled: &dyn Fn() -> bool) -> Option<Error> {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        let monitor = self.child.monitor();
        let status = loop {
            if cancelled() {
                self.kill(FailureKind::Cancelled);
                return None;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Some(status) = monitor.wait_for_exit(remaining.min(POLL)) {
                break Some(status);
            }
            if remaining.is_zero() {
                self.kill(FailureKind::Crashed);
                break None;
            }
        };
        let by_canceller =
            !self.end_reported && monitor.end_reason() == Some(FailureKind::Cancelled);
        self.end_reported = true;
        // A canceller ended the process; the connection is closed here.
        self.transport.close();
        Some(match self.culprit() {
            Ok(slot) if by_canceller => Error::Cancelled { slot },
            Ok(slot) => Error::Crashed { slot, status },
            Err(error) => error,
        })
    }

    /// Terminates and reaps the helper for `reason`, then closes the connection. A reply that
    /// arrived after a deadline is discarded, and every later request reports the exit; the owner
    /// recovers the chain in a new helper.
    pub fn kill(&mut self, reason: FailureKind) {
        let _ = self.child.kill(reason);
        self.end_reported = true;
        self.transport.close();
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.child.expect_exit(FailureKind::Closed);
        if self.transport.queue(&Request::Shutdown).is_ok()
            && self.transport.flush(EXIT_TIMEOUT).is_ok()
        {
            self.child.monitor().wait_for_exit(EXIT_TIMEOUT);
        }
        self.kill(FailureKind::Closed);
    }
}
