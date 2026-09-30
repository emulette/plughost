//! Anonymous file ownership transfer at prepare time, separate from PCM processing.
//! On Unix a private datagram socket is inherited as stdin; Windows duplicates into the child.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{Receiver, Sender};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Receiver, Sender};
