pub const PARTIAL_STATE: &str = "test state rejected after partial application";
pub const PREPARE_STATE: &str = "test state rejected during preparation";
pub const FIRST_START: &str = "test processor rejects its first start";
pub const DESCENDANT: &str = "could not start the lifetime test descendant";
pub const MODULE_PATH: &str = "could not find the path of the loaded test plugin module";
pub const UNKNOWN_VARIANT: &str =
    "the test plugin module is not named after one of its variants, plughost-test-<variant>";
#[cfg(target_os = "macos")]
pub const GUI_API: &str = "the test editor draws only inside a Cocoa view of the host";
#[cfg(target_os = "macos")]
pub const GUI_PARENT: &str = "the host gave the test editor no Cocoa view to draw in";
#[cfg(target_os = "windows")]
pub const GUI_API: &str = "the test editor draws only inside a Win32 window of the host";
#[cfg(target_os = "windows")]
pub const GUI_PARENT: &str = "the host gave the test editor no Win32 window to draw in";
/// Starts the `thread-check` variant's report of a call on the wrong thread.
pub const WRONG_THREAD: &str = "called on the wrong thread: ";
pub const STALL_ADDRESS: &str = "the stall fixture needs a barrier address as its host name";
pub const STALL_CONNECT: &str = "the stall fixture could not announce entry to its barrier";
pub const EXIT_MARKER: &str = "could not create the module exit marker";
