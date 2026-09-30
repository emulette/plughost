use std::fmt;
use std::process::ExitStatus;

use plughost_core::messages::{BUFFERS, CHAIN_NOT_PREPARED};
use plughost_core::{Failure, FailureKind, InputError, RenderError};

const HELPER_START: &str = "could not start the helper";
const HANDSHAKE: &str = "the helper did not identify itself with the expected token and protocol";
const CRASHED: &str = "the helper exited";
const TIMED_OUT: &str = "the helper stopped responding and was terminated";
const OPERATION: &str = "the operation failed";
const PROTOCOL: &str = "the helper sent an invalid or unexpected message";
const SLOT: &str = "slot";
const CACHE: &str = "could not access the scan cache";

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    HelperStart(std::io::Error),
    Handshake,
    /// The helper process ended. `slot` is the plugin the helper was calling, as far as the helper
    /// had recorded it, preferring the thread serving the failed request; a plugin-owned thread,
    /// the other helper thread, or earlier memory corruption can also be the cause.
    Crashed {
        slot: Option<usize>,
        status: Option<ExitStatus>,
    },
    /// The helper did not respond within the operation's timeout and was terminated.
    TimedOut {
        slot: Option<usize>,
    },
    Operation {
        slot: Option<usize>,
        failure: Failure,
    },
    Protocol,
    NotPrepared,
    Input {
        slot: Option<usize>,
        error: InputError,
    },
    Buffers,
    Render(RenderError),
    Cache(std::io::Error),
}

fn slot(f: &mut fmt::Formatter<'_>, slot: &Option<usize>) -> fmt::Result {
    match slot {
        Some(slot) => write!(f, " ({SLOT} {slot})"),
        None => Ok(()),
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::HelperStart(error) => write!(f, "{HELPER_START}: {error}"),
            Error::Handshake => f.write_str(HANDSHAKE),
            Error::Crashed {
                slot: which,
                status,
            } => {
                f.write_str(CRASHED)?;
                slot(f, which)?;
                match status {
                    Some(status) => write!(f, ": {status}"),
                    None => Ok(()),
                }
            }
            Error::TimedOut { slot: which } => {
                f.write_str(TIMED_OUT)?;
                slot(f, which)
            }
            Error::Operation {
                slot: which,
                failure,
            } => {
                f.write_str(OPERATION)?;
                slot(f, which)?;
                write!(f, ": {failure}")
            }
            Error::Protocol => f.write_str(PROTOCOL),
            Error::NotPrepared => f.write_str(CHAIN_NOT_PREPARED),
            Error::Input { slot: which, error } => {
                error.fmt(f)?;
                slot(f, which)
            }
            Error::Buffers => f.write_str(BUFFERS),
            Error::Render(error) => error.fmt(f),
            Error::Cache(error) => write!(f, "{CACHE}: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::HelperStart(error) | Error::Cache(error) => Some(error),
            Error::Operation { failure, .. } => Some(failure),
            Error::Input { error, .. } => Some(error),
            Error::Render(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RenderError> for Error {
    fn from(error: RenderError) -> Error {
        Error::Render(error)
    }
}

impl Error {
    /// Common category for local errors and structured helper failures.
    pub fn kind(&self) -> FailureKind {
        match self {
            Self::HelperStart(_) | Self::Cache(_) => FailureKind::Host,
            Self::Handshake | Self::Protocol => FailureKind::Protocol,
            Self::Crashed { .. } => FailureKind::Crashed,
            Self::TimedOut { .. } => FailureKind::TimedOut,
            Self::Operation { failure, .. } => failure.kind,
            Self::NotPrepared => FailureKind::NotPrepared,
            Self::Input { .. } | Self::Buffers => FailureKind::InvalidInput,
            Self::Render(error) => error.kind(),
        }
    }
}
