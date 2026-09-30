//! Owns the process and its descendants independently of request/response traffic.
mod scope;

use std::io;
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Observes one helper generation, even while the chain sends no requests. Clones share a sticky
/// exit status; a recovered chain has a new monitor. Waiting belongs on an application/control
/// thread.
#[derive(Clone)]
pub struct HelperMonitor {
    pid: u32,
    ended: Arc<(Mutex<Option<ExitStatus>>, Condvar)>,
}

impl HelperMonitor {
    pub fn process_id(&self) -> u32 {
        self.pid
    }

    /// None means no exit has been observed yet, not proof that the helper is responsive.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        *self.ended.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits up to timeout for the OS exit status. No plugin or IPC request is made.
    pub fn wait_for_exit(&self, timeout: Duration) -> Option<ExitStatus> {
        let status = self.ended.0.lock().unwrap_or_else(|e| e.into_inner());
        let (status, _) = self
            .ended
            .1
            .wait_timeout_while(status, timeout, |s| s.is_none())
            .unwrap_or_else(|e| e.into_inner());
        *status
    }

    fn publish(&self, status: ExitStatus) {
        *self.ended.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(status);
        self.ended.1.notify_all();
    }
}

struct State {
    child: Child,
    scope: scope::Scope,
    status: Option<ExitStatus>,
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

    fn finish(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        self.scope.terminate();
        let _ = self.child.kill();
        let status = self.child.wait()?;
        self.status = Some(status);
        self.monitor.publish(status);
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
            ended: Arc::new((Mutex::new(None), Condvar::new())),
        };
        let exit = scope::Exit::of(&child);
        let state = Arc::new(Mutex::new(State {
            child,
            scope,
            status: None,
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
    #[cfg(target_os = "windows")]
    pub fn id(&self) -> u32 {
        self.monitor.pid
    }
    pub fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).poll()
    }
    pub fn kill(&self) -> io::Result<()> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finish()
            .map(|_| ())
    }
    pub fn with_child<T>(&self, f: impl FnOnce(&Child) -> T) -> T {
        f(&self.state.lock().unwrap_or_else(|e| e.into_inner()).child)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.kill();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
