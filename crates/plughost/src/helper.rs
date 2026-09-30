//! Starting and supervising one helper process.

mod process;
#[cfg(test)]
mod tests;
mod transport;
pub use process::HelperMonitor;

use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, ListenerNonblockingMode, ListenerOptions};
use plughost_core::HostIdentity;
use plughost_core::PluginRef;
use plughost_core::ipc::shared::{Activity, Caller, transfer};
use plughost_core::ipc::{PROTOCOL_VERSION, Request, Response};

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
}

struct Shared {
    activity: Activity,
    sender: transfer::Sender,
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
                (Some(Shared { activity, sender }), stdin)
            }
            Mode::Scan => (None, Stdio::null()),
        };
        let name = format!("plughost-{:016x}{:016x}.sock", random(), random());
        let token = random();
        let listener = ListenerOptions::new()
            .name(
                name.as_str()
                    .to_ns_name::<GenericNamespaced>()
                    .map_err(Error::HelperStart)?,
            )
            .nonblocking(ListenerNonblockingMode::Accept)
            .create_sync()
            .map_err(Error::HelperStart)?;
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
                let _ = child.kill();
                return Ok(None);
            }
            match listener.accept() {
                Ok(stream) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if let Ok(Some(status)) = child.try_wait() {
                        return Err(Error::Crashed {
                            slot: None,
                            status: Some(status),
                        });
                    }
                    if Instant::now() > deadline {
                        let _ = child.kill();
                        return Err(Error::TimedOut { slot: None });
                    }
                    thread::sleep(POLL);
                }
                Err(error) => {
                    let _ = child.kill();
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
                let _ = child.kill();
                return Err(Error::HelperStart(error));
            }
        };
        let mut helper = Helper {
            child,
            transport,
            shared,
            processing: false,
        };
        match helper.message_controlled(START_TIMEOUT, cancelled)? {
            Some(Incoming::Hello(hello))
                if hello.protocol == PROTOCOL_VERSION && hello.token == token =>
            {
                Ok(Some(helper))
            }
            None => Ok(None),
            _ => {
                helper.kill();
                Err(Error::Handshake)
            }
        }
    }

    /// Sends a request and waits for its reply. `timeout` bounds writing the request and the wait
    /// for each message, so progress messages keep a long operation alive.
    pub fn request(&mut self, request: Request, timeout: Duration) -> Result<Response, Error> {
        self.send(request, timeout)?;
        match self.receive(timeout)? {
            Response::Failed { slot, failure } => {
                if failure.kind == plughost_core::FailureKind::Protocol {
                    self.kill();
                    return Err(Error::Protocol);
                }
                Err(Error::Operation { slot, failure })
            }
            Response::Rejected { slot, error } => Err(Error::Input { slot, error }),
            response => Ok(response),
        }
    }

    /// Host mode: loads the chain, handing the helper the mapping it announces calls through.
    pub fn load(
        &mut self,
        host: &HostIdentity,
        plugins: &[PluginRef],
        timeout: Duration,
    ) -> Result<Response, Error> {
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
        request: impl FnOnce(u64) -> Request,
        timeout: Duration,
    ) -> Result<Response, Error> {
        let handle = self.transfer(|sender, _, child| sender.send(memory, child))?;
        // Connection errors/deadlines reap the child, including unclaimed transferred handles.
        // A normal rejected prepare consumes its transferred file before returning an error.
        self.request(request(handle), timeout)
    }

    fn transfer(
        &mut self,
        send: impl FnOnce(&transfer::Sender, &Activity, &std::process::Child) -> io::Result<u64>,
    ) -> Result<u64, Error> {
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

    /// Writes a request within `timeout`. A request not written in time leaves the connection
    /// mid-frame, so the helper is terminated.
    pub fn send(&mut self, request: Request, timeout: Duration) -> Result<(), Error> {
        self.send_controlled(request, timeout, &|| false)
            .map(Option::unwrap)
    }

    /// Writes a request within `timeout`, polling cancellation, which reaps the child.
    pub fn send_controlled(
        &mut self,
        request: Request,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<()>, Error> {
        if cancelled() {
            self.kill();
            return Ok(None);
        }
        match self.queue(&request) {
            Ok(()) => {}
            // The helper is fine; the request is not one this crate should have made.
            Err(Unqueued::Unencodable) => {
                self.kill();
                return Err(Error::Protocol);
            }
            Err(Unqueued::Closed) => return self.ended_controlled(cancelled).map_or(Ok(None), Err),
        }
        let started = Instant::now();
        loop {
            if cancelled() {
                self.kill();
                return Ok(None);
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            match self.transport.flush(remaining.min(POLL)) {
                Ok(()) => return Ok(Some(())),
                Err(Stopped::TimedOut) if started.elapsed() < timeout => continue,
                Err(Stopped::TimedOut) => {
                    self.kill();
                    return Err(Error::TimedOut {
                        slot: self.culprit(),
                    });
                }
                Err(Stopped::Closed) => {
                    return self.ended_controlled(cancelled).map_or(Ok(None), Err);
                }
            }
        }
    }

    fn queue(&mut self, request: &Request) -> Result<(), Unqueued> {
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
                self.kill();
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
                self.kill();
                Err(Error::Protocol)
            }
            None => Ok(None),
        }
    }

    /// The slot the helper was calling when it died or stopped responding. The thread serving
    /// the outstanding request is asked first; the other thread may hold up that request, or
    /// crash on its own, as the main thread can between requests. Slots are known only as far as
    /// the helper recorded them, so a crash on a plugin-owned thread names no slot.
    fn culprit(&self) -> Option<usize> {
        let activity = &self.shared.as_ref()?.activity;
        let (first, second) = if self.processing {
            (Caller::Processing, Caller::Main)
        } else {
            (Caller::Main, Caller::Processing)
        };
        activity.slot(first).or_else(|| activity.slot(second))
    }

    fn message_controlled(
        &mut self,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<Incoming>, Error> {
        let started = Instant::now();
        loop {
            if cancelled() {
                self.kill();
                return Ok(None);
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            match self.transport.receive(remaining.min(POLL)) {
                Ok(Ok(message)) => return Ok(Some(message)),
                Err(Stopped::TimedOut) if started.elapsed() < timeout => continue,
                Err(Stopped::TimedOut) => {
                    self.kill();
                    return Err(Error::TimedOut {
                        slot: self.culprit(),
                    });
                }
                Ok(Err(error)) if error.kind() == io::ErrorKind::InvalidData => {
                    self.kill();
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
                self.kill();
                Err(Error::TimedOut {
                    slot: self.culprit(),
                })
            }
            Ok(Err(error)) if error.kind() == io::ErrorKind::InvalidData => {
                self.kill();
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
                self.kill();
                return None;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Some(status) = monitor.wait_for_exit(remaining.min(POLL)) {
                break Some(status);
            }
            if remaining.is_zero() {
                self.kill();
                break None;
            }
        };
        Some(Error::Crashed {
            slot: self.culprit(),
            status,
        })
    }

    /// Terminates and reaps the helper, then closes the connection. A reply that arrived after a
    /// deadline is discarded, and every later request reports the exit; the owner recovers the
    /// chain in a new helper.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        self.transport.close();
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        if self.transport.queue(&Request::Shutdown).is_ok()
            && self.transport.flush(EXIT_TIMEOUT).is_ok()
        {
            self.child.monitor().wait_for_exit(EXIT_TIMEOUT);
        }
        self.kill();
    }
}
