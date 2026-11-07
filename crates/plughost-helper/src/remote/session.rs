//! The channel of a remote session: two pipes, the application's commands and the helper's
//! notices. Notices go out on a writer thread, so the main thread never waits for the
//! application; a reader thread takes the commands that come in and sees the application close
//! its end. Starting a session ends the one before: its notices' writer closes, which its
//! application end reads as the end.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::sync::mpsc;
use std::sync::{Mutex, PoisonError};
use std::thread;

use plughost_core::ipc::shared::transfer;
use plughost_core::ipc::{CommandMessage, MessageReader, MessageWriter, NoticeMessage, Response};
use plughost_core::remote::Notice;
use plughost_core::{Failure, FailureKind};

struct Current {
    generation: u64,
    /// The notices' writer thread ends once this is dropped, closing the session for the
    /// application.
    notices: mpsc::Sender<Notice>,
}

static CURRENT: Mutex<Option<Current>> = Mutex::new(None);
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn current() -> std::sync::MutexGuard<'static, Option<Current>> {
    CURRENT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts the session whose pipes the application transferred as `commands` and `notices`,
/// ending the one before, and answers the request. The request reader calls it, whatever loop
/// the main thread is in: a session that would wait for a plugin's dialog to end could not close
/// it. The main thread then tells the session the current state.
pub fn open(
    transfer: &transfer::Receiver,
    commands: u64,
    notices: u64,
    menu_timeout: std::time::Duration,
) -> Response {
    let received = transfer
        .receive_channel(commands)
        .and_then(|commands| Ok((commands, transfer.receive_channel(notices)?)));
    match received {
        Ok((commands, notices)) => {
            // SAFETY: the descriptor is open; the flag changes only how writes to it fail.
            unsafe { libc::fcntl(notices.as_raw_fd(), libc::F_SETNOSIGPIPE, 1) };
            start(File::from(commands), File::from(notices), menu_timeout);
            Response::Done
        }
        Err(error) => Response::Failed {
            slot: None,
            failure: Failure::new(FailureKind::Protocol, error.to_string()),
        },
    }
}

/// Starts a session on its pipes, ending the one before.
fn start(commands: File, notices: File, menu_timeout: std::time::Duration) {
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (sender, outgoing) = mpsc::channel::<Notice>();
    let mut writer = MessageWriter::new(notices);
    thread::spawn(move || {
        for notice in outgoing {
            if writer.write(&NoticeMessage(notice)).is_err() {
                break;
            }
        }
    });
    {
        let mut current = current();
        let replaced = current.replace(Current {
            generation,
            notices: sender,
        });
        if replaced.is_some() {
            // Under the lock, so the release comes before any command of the new session.
            super::input::replaced();
        }
        super::input::started(menu_timeout);
    }
    // Once current, so the session's first commands are taken.
    thread::spawn(move || {
        let mut reader = MessageReader::new(commands);
        while let Ok(CommandMessage(command)) = reader.read::<CommandMessage>()
            && received(generation, command)
        {}
        end(generation);
    });
}

/// Queues `command` if it came on the current session, and tells whether it did. A session
/// replaced has had its end queued; what it sent after that is dropped.
fn received(generation: u64, command: plughost_core::remote::Command) -> bool {
    let current = current();
    let open = current
        .as_ref()
        .is_some_and(|current| current.generation == generation);
    if open {
        super::input::receive(command);
    }
    open
}

/// Sends `notice` on the session, if one is open.
pub fn notify(notice: Notice) {
    if let Some(current) = &*current() {
        let _ = current.notices.send(notice);
    }
}

fn end(generation: u64) {
    let mut current = current();
    if current
        .as_ref()
        .is_some_and(|current| current.generation == generation)
    {
        current.take();
        super::input::ended();
    }
}
