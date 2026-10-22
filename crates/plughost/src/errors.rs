use std::fmt;
use std::process::ExitStatus;

use plughost_core::messages::{BUFFERS, CHAIN_NOT_PREPARED};
use plughost_core::{Failure, FailureKind, InputError, RenderError};

const HELPER_START: &str = "could not start the helper";
const HANDSHAKE: &str = "the helper did not identify itself with the expected token";
const PROTOCOL_VERSION: &str =
    "the helper was built with another plughost version: its protocol is";
const APPLICATION_PROTOCOL: &str = "the application's is";
const CRASHED: &str = "the helper exited";
const TIMED_OUT: &str = "the helper stopped responding and was terminated";
const OPERATION: &str = "the operation failed";
const PROTOCOL: &str = "the helper sent an invalid or unexpected message";
const SLOT: &str = "slot";
const CACHE: &str = "could not access the scan cache";
const INPUT: &str = "invalid input";
const RENDER: &str = "rendering failed";
const REALTIME_RUNNING: &str =
    "the operation is refused while the chain's realtime endpoint runs; stop it first";

/// Failures in helper supervision, native operations, input validation, or rendering.
/// Use [`Error::kind`] for a stable category and [`std::error::Error::source`] for details.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The helper could not be launched or its local transport could not be established.
    HelperStart(std::io::Error),
    /// The helper did not present the expected token.
    Handshake,
    /// The helper presented the expected token with another internal protocol version: it was
    /// built with another plughost version than the application. Build both with the same one.
    ProtocolVersion {
        /// The application's internal protocol version.
        application: u32,
        /// The helper's internal protocol version.
        helper: u32,
    },
    /// The helper process ended. `slot` is the plugin the helper was calling, as far as the helper
    /// had recorded it, preferring the thread serving the failed request; a plugin-owned thread,
    /// the other helper thread, or earlier memory corruption can also be the cause.
    Crashed {
        /// Zero-based plugin slot when attributable to one slot; otherwise `None`.
        slot: Option<usize>,
        /// The OS exit status, when the supervisor has observed it.
        status: Option<ExitStatus>,
    },
    /// The helper did not respond within the operation's timeout and was terminated.
    TimedOut {
        /// Zero-based plugin slot when attributable to one slot; otherwise `None`.
        slot: Option<usize>,
    },
    /// A helper operation returned a structured native or host failure.
    Operation {
        /// Zero-based plugin slot when attributable to one slot; otherwise `None`.
        slot: Option<usize>,
        /// The portable failure category and diagnostic from the helper.
        failure: Failure,
    },
    /// A malformed or unexpected helper response invalidated the connection.
    Protocol,
    /// The chain has no usable prepared audio configuration.
    NotPrepared,
    /// Caller input failed validation before it could be applied.
    Input {
        /// Zero-based plugin slot when attributable to one slot; otherwise `None`.
        slot: Option<usize>,
        /// The violated input contract.
        error: InputError,
    },
    /// Block audio has the wrong channel count or slice lengths, or more frames than the
    /// prepared maximum block size.
    Buffers,
    /// The render scheduler rejected its input or could not complete delivery.
    Render(RenderError),
    /// A scan writer could not lock, read, publish, or flush its cache. Discovery reads
    /// instead treat unreadable caches as empty.
    Cache(std::io::Error),
    /// The chain's realtime endpoint runs, which refuses what would reconfigure or reset the
    /// chain under it, and processing through the chain itself.
    RealtimeRunning,
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
            Error::HelperStart(_) => f.write_str(HELPER_START),
            Error::Handshake => f.write_str(HANDSHAKE),
            Error::ProtocolVersion {
                application,
                helper,
            } => write!(
                f,
                "{PROTOCOL_VERSION} {helper}, {APPLICATION_PROTOCOL} {application}"
            ),
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
            Error::Operation { slot: which, .. } => {
                f.write_str(OPERATION)?;
                slot(f, which)
            }
            Error::Protocol => f.write_str(PROTOCOL),
            Error::NotPrepared => f.write_str(CHAIN_NOT_PREPARED),
            Error::Input { slot: which, .. } => {
                f.write_str(INPUT)?;
                slot(f, which)
            }
            Error::Buffers => f.write_str(BUFFERS),
            Error::Render(_) => f.write_str(RENDER),
            Error::Cache(_) => f.write_str(CACHE),
            Error::RealtimeRunning => f.write_str(REALTIME_RUNNING),
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
    /// Flattens the full chain when a scan must store its diagnostic as a portable string.
    pub(crate) fn diagnostic(&self) -> String {
        let mut message = self.to_string();
        let mut cause = std::error::Error::source(self);
        while let Some(error) = cause {
            message.push_str(": ");
            message.push_str(&error.to_string());
            cause = error.source();
        }
        message
    }

    /// Common category for local errors and structured helper failures.
    pub fn kind(&self) -> FailureKind {
        match self {
            Self::HelperStart(_) | Self::Cache(_) => FailureKind::Host,
            Self::Handshake | Self::ProtocolVersion { .. } | Self::Protocol => {
                FailureKind::Protocol
            }
            Self::Crashed { .. } => FailureKind::Crashed,
            Self::TimedOut { .. } => FailureKind::TimedOut,
            Self::Operation { failure, .. } => failure.kind,
            Self::NotPrepared => FailureKind::NotPrepared,
            Self::Input { .. } | Self::Buffers | Self::RealtimeRunning => FailureKind::InvalidInput,
            Self::Render(error) => error.kind(),
        }
    }
}
