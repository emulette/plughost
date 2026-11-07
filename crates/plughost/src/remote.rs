//! Remote editors: editors a chain started in remote mode ([`SpawnOptions::remote`]) shows in a
//! region of a display that the application reads, for someone who is not in front of the
//! machine (macOS only). The application owns the display, reading the region, and sending what it
//! reads.
//!
//! [`SpawnOptions::remote`]: crate::SpawnOptions::remote

pub use plughost_core::remote::*;

#[cfg(target_os = "macos")]
pub(crate) use session::pipe;
#[cfg(target_os = "macos")]
pub use session::{Sender, Session};

#[cfg(target_os = "macos")]
mod session {
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::sync::mpsc::{self, SyncSender, TrySendError};
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};
    use std::time::{Duration, Instant};

    use plughost_core::ipc::{CommandMessage, FrameDecoder, MessageWriter, NoticeMessage};
    use rustix::event::{PollFd, PollFlags, Timespec};

    use super::{Command, Notice};
    use crate::Error;

    /// How many commands wait for the helper before [`Sender::send`] refuses more.
    const BACKLOG: usize = 1024;

    /// A pipe for a session, both ends closed on exec. A session's channel is a pair of pipes,
    /// not a socket pair: macOS's collection of Unix sockets in flight between processes flushed
    /// a socket sent to the helper while another Unix socket closed, and the session read as
    /// ended at once.
    pub(crate) fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
        let (read, write) = rustix::pipe::pipe()?;
        for end in [&read, &write] {
            rustix::io::fcntl_setfd(end, rustix::io::FdFlags::CLOEXEC)?;
        }
        Ok((read, write))
    }

    /// A pipe end written to fails rather than raising `SIGPIPE` once its reader is gone.
    fn no_sigpipe(end: &OwnedFd) {
        // SAFETY: the descriptor is open; the flag changes only how writes to it fail.
        unsafe { libc::fcntl(end.as_raw_fd(), libc::F_SETNOSIGPIPE, 1) };
    }

    /// A remote session: the channel on which the chain's helper tells the application what
    /// happens to its remote editors as it happens, including while the helper's main thread is
    /// in a dialog's, a menu's or a panel's loop and the chain's requests wait. It starts with the
    /// current state. Dropping it closes the channel.
    pub struct Session {
        notices: File,
        /// The commands' write end, taken to end the session for the helper while senders are
        /// still held.
        commands: Arc<Mutex<Option<File>>>,
        decoder: FrameDecoder,
        sender: OnceLock<Sender>,
    }

    /// Sends commands on a remote session, from any thread, without waiting for the helper. A
    /// writer thread hands them to the helper in order.
    #[derive(Clone, Debug)]
    pub struct Sender {
        queue: SyncSender<Command>,
    }

    impl Sender {
        /// Queues `command` for the helper. Fails with [`Error::RemoteBacklog`] when the helper
        /// has not taken the commands before it, and with [`Error::RemoteSessionEnded`] once the
        /// session ended.
        pub fn send(&self, command: Command) -> Result<(), Error> {
            self.queue.try_send(command).map_err(|error| match error {
                TrySendError::Full(_) => Error::RemoteBacklog,
                TrySendError::Disconnected(_) => Error::RemoteSessionEnded,
            })
        }
    }

    /// The commands' write end as the writer thread holds it, which the session may close.
    struct Commands(Arc<Mutex<Option<File>>>);

    impl Write for Commands {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            match &mut *self.0.lock().unwrap_or_else(PoisonError::into_inner) {
                Some(file) => file.write(bytes),
                None => Err(io::ErrorKind::BrokenPipe.into()),
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Session {
        /// Ends the session for the helper and for the senders still held.
        fn drop(&mut self) {
            self.commands
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
        }
    }

    impl std::fmt::Debug for Session {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Session").finish_non_exhaustive()
        }
    }

    impl Session {
        pub(crate) fn new(notices: OwnedFd, commands: OwnedFd) -> Session {
            no_sigpipe(&commands);
            Session {
                notices: File::from(notices),
                commands: Arc::new(Mutex::new(Some(File::from(commands)))),
                decoder: FrameDecoder::default(),
                sender: OnceLock::new(),
            }
        }

        /// A sender of commands to the helper; its clones send on the same session.
        pub fn sender(&self) -> Result<Sender, Error> {
            Ok(self
                .sender
                .get_or_init(|| {
                    let mut writer = MessageWriter::new(Commands(Arc::clone(&self.commands)));
                    let (queue, commands) = mpsc::sync_channel::<Command>(BACKLOG);
                    std::thread::spawn(move || {
                        for command in commands {
                            if writer.write(&CommandMessage(command)).is_err() {
                                break;
                            }
                        }
                    });
                    Sender { queue }
                })
                .clone())
        }

        /// The next notice, waiting up to `timeout` for it; `None` when none came in time.
        /// Fails with [`Error::RemoteSessionEnded`] once the helper exits or a later session
        /// replaces this one.
        pub fn recv(&mut self, timeout: Duration) -> Result<Option<Notice>, Error> {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(message) = self.decoder.message::<NoticeMessage>() {
                    return match message {
                        Ok(NoticeMessage(notice)) => Ok(Some(notice)),
                        Err(_) => Err(Error::Protocol),
                    };
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Ok(None);
                }
                let timeout = Timespec {
                    tv_sec: left.as_secs() as i64,
                    tv_nsec: i64::from(left.subsec_nanos()),
                };
                let mut ready = [PollFd::new(&self.notices, PollFlags::IN)];
                match rustix::event::poll(&mut ready, Some(&timeout)) {
                    Ok(0) => continue,
                    Ok(_) => {}
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(_) => return Err(Error::RemoteSessionEnded),
                }
                let space = self.decoder.unfilled().map_err(|_| Error::Protocol)?;
                match self.notices.read(space) {
                    Ok(0) => return Err(Error::RemoteSessionEnded),
                    Ok(count) => self.decoder.advance(count),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => return Err(Error::RemoteSessionEnded),
                }
            }
        }
    }
}
