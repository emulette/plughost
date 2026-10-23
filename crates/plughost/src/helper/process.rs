//! Owns the process and its descendants independently of request/response traffic.
mod scope;

use std::io;
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use plughost_core::FailureKind;

/// Observes one helper generation, even while the chain sends no requests. Clones share a sticky
/// exit status; a recovered chain has a new monitor. Waiting belongs on an application/control
/// thread.
#[derive(Clone)]
pub struct HelperMonitor {
    pid: u32,
    ended: Arc<(Mutex<Ended>, Condvar)>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Ended {
    status: Option<ExitStatus>,
    reason: Option<FailureKind>,
}

impl std::fmt::Debug for HelperMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ended = *self.ended.0.lock().unwrap_or_else(|e| e.into_inner());
        f.debug_struct("HelperMonitor")
            .field("process_id", &self.pid)
            .field("exit_status", &ended.status)
            .field("end_reason", &ended.reason)
            .finish_non_exhaustive()
    }
}

impl HelperMonitor {
    /// The OS process ID of the helper generation observed by this monitor.
    pub fn process_id(&self) -> u32 {
        self.pid
    }

    /// None means no exit has been observed yet, not proof that the helper is responsive.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.ended
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
    }

    /// Why this helper generation ended, once its exit is observed: [`FailureKind::Crashed`] when
    /// it exited on its own (a crash, or a plugin ending the process), [`FailureKind::TimedOut`],
    /// [`FailureKind::Protocol`] or [`FailureKind::Cancelled`] when plughost terminated it for that
    /// reason, and [`FailureKind::Closed`] when its chain shut it down. Errors after the first
    /// one report [`Error::Crashed`](crate::Error::Crashed) whatever ended the helper; this tells
    /// them apart.
    pub fn end_reason(&self) -> Option<FailureKind> {
        self.ended
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reason
    }

    /// Waits up to timeout for the OS exit status. No plugin or IPC request is made.
    pub fn wait_for_exit(&self, timeout: Duration) -> Option<ExitStatus> {
        let ended = self.ended.0.lock().unwrap_or_else(|e| e.into_inner());
        let (ended, _) = self
            .ended
            .1
            .wait_timeout_while(ended, timeout, |ended| ended.status.is_none())
            .unwrap_or_else(|e| e.into_inner());
        ended.status
    }

    fn publish(&self, status: ExitStatus, reason: FailureKind) {
        *self.ended.0.lock().unwrap_or_else(|e| e.into_inner()) = Ended {
            status: Some(status),
            reason: Some(reason),
        };
        self.ended.1.notify_all();
    }
}

/// Cancels the calls a chain is making on one helper generation, from any thread. Clones cancel
/// the same generation; a recovered chain has its own canceller, and cancelling after the helper
/// ended does nothing.
#[derive(Clone)]
pub struct ChainCanceller {
    state: Weak<Mutex<State>>,
}

impl std::fmt::Debug for ChainCanceller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainCanceller").finish_non_exhaustive()
    }
}

impl ChainCanceller {
    /// Terminates the helper without waiting for the chain. The call it is blocked in, or else
    /// its next call, returns [`Error::Cancelled`](crate::Error::Cancelled); later calls return
    /// [`Error::Crashed`](crate::Error::Crashed) until the application recovers the chain.
    pub fn cancel(&self) {
        if let Some(state) = self.state.upgrade() {
            let _ = state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .terminate(FailureKind::Cancelled);
        }
    }
}

struct State {
    child: Child,
    scope: scope::Scope,
    status: Option<ExitStatus>,
    /// Why plughost ended or is ending the helper; `None` until it does.
    reason: Option<FailureKind>,
    monitor: HelperMonitor,
}

impl State {
    fn poll(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() && self.scope.exited(&self.child)? {
            // Terminate the group before reaping its leader, keeping its PID reserved on macOS.
            self.finish()?;
        }
        Ok(self.status)
    }

    /// Records why the helper is about to end, unless it ended already or on its own.
    fn expect(&mut self, reason: FailureKind) {
        if self.status.is_none() && !self.scope.exited(&self.child).unwrap_or(true) {
            self.reason.get_or_insert(reason);
        }
    }

    fn terminate(&mut self, reason: FailureKind) -> io::Result<ExitStatus> {
        self.expect(reason);
        self.finish()
    }

    fn finish(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        self.scope.terminate();
        let _ = self.child.kill();
        let status = self.child.wait()?;
        self.status = Some(status);
        self.monitor
            .publish(status, self.reason.unwrap_or(FailureKind::Crashed));
        Ok(status)
    }
}

pub(super) struct Process {
    state: Arc<Mutex<State>>,
    monitor: HelperMonitor,
    worker: Option<JoinHandle<()>>,
}

impl Process {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        scope::configure(command);
        let mut child = command.spawn()?;
        let scope = match scope::Scope::attach(&child) {
            Ok(scope) => scope,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let monitor = HelperMonitor {
            pid: child.id(),
            ended: Arc::new((Mutex::new(Ended::default()), Condvar::new())),
        };
        let exit = scope::Exit::of(&child);
        let state = Arc::new(Mutex::new(State {
            child,
            scope,
            status: None,
            reason: None,
            monitor: monitor.clone(),
        }));
        // Keeps the child, and so the handle `exit` waits on, alive while the watcher runs.
        let watched = state.clone();
        let worker = thread::spawn(move || {
            // An error means the owner already reaped the child.
            while exit.wait().is_ok() {
                if !matches!(
                    watched.lock().unwrap_or_else(|e| e.into_inner()).poll(),
                    Ok(None)
                ) {
                    break;
                }
            }
        });
        Ok(Self {
            state,
            monitor,
            worker: Some(worker),
        })
    }

    pub fn monitor(&self) -> HelperMonitor {
        self.monitor.clone()
    }
    pub fn canceller(&self) -> ChainCanceller {
        ChainCanceller {
            state: Arc::downgrade(&self.state),
        }
    }
    #[cfg(target_os = "windows")]
    pub fn id(&self) -> u32 {
        self.monitor.pid
    }
    pub fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).poll()
    }
    /// Terminates the helper for `reason`, unless it already ended.
    pub fn kill(&self, reason: FailureKind) -> io::Result<()> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .terminate(reason)
            .map(|_| ())
    }
    /// Records `reason` for an exit plughost asked the helper for, as by `Shutdown`.
    pub fn expect_exit(&self, reason: FailureKind) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .expect(reason);
    }
    pub fn with_child<T>(&self, f: impl FnOnce(&Child) -> T) -> T {
        f(&self.state.lock().unwrap_or_else(|e| e.into_inner()).child)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.kill(FailureKind::Closed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
