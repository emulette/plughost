use std::io::Write;

pub const USAGE: &str = "usage: <scan|host> <socket name> <hexadecimal token> <parent pid>";
#[cfg(target_os = "macos")]
pub const PROCESS_SCOPE: &str =
    "the helper must run in its own process group with its application parent";
pub const CONNECT: &str = "the helper lost its connection to the application";
pub const WRONG_MODE: &str = "the request does not belong to this helper mode";
pub const LATENCY_OVERFLOW: &str = "the chain latency exceeds the supported sample count";
#[cfg(target_os = "macos")]
pub const NOT_MAIN_THREAD: &str = "editors can only open on the helper's main thread";
#[cfg(target_os = "windows")]
pub const EDITOR_WINDOW: &str = "could not create or resize the editor window";
#[cfg(target_os = "windows")]
pub const COM_INIT: &str = "could not initialize the helper's COM apartment";

/// Reports a startup failure. The helper has no connection to report it over, and standard error
/// is the one place a developer running it by hand will look.
pub fn report(message: &str) {
    let _ = writeln!(std::io::stderr(), "plughost helper: {message}");
}

pub const AUDIO_NEGOTIATION: &str =
    "native bus activation or layout does not match the requested configuration";
pub const AUDIO_LATENCY_CHANGED: &str =
    "native latency or audio topology changed; prepare the routed chain again before processing";

pub const EVENT_PORT_PLAN: &str = "a prepared plugin does not list an event port its routing uses";
pub const EVENT_RESYNC: &str = "processing stopped after events exceeded a block's budget; reset the chain before processing again";

pub const ALIGNMENT_STORAGE: &str =
    "could not reserve the chain's alignment delays within MAX_ALIGNMENT_BYTES";
