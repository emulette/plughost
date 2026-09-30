use std::io;

pub(super) const CONFIGURATION: &str =
    "the shared audio configuration exceeds its fixed storage budget";
pub(super) const TRANSFER: &str = "the shared audio handle transfer is invalid";
pub(super) const BLOCK: &str = "the shared audio block does not match the prepared slot";
pub(super) const GENERATION: &str = "the shared audio generation is invalid";
pub(super) const EVENTS: &str = "the shared audio events are invalid";
pub(super) const ACTIVITY: &str = "the shared activity mapping is invalid";
pub(super) const STORAGE: &str = "could not reserve local shared audio storage";

pub(super) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
