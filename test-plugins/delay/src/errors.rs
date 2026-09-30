pub const PARTIAL_STATE: &str = "test state rejected after partial application";
pub const PREPARE_STATE: &str = "test state rejected during preparation";
pub const DESCENDANT: &str = "could not start the lifetime test descendant";
pub const MODULE_PATH: &str = "could not find the path of the loaded test plugin module";
pub const UNKNOWN_VARIANT: &str =
    "the test plugin module is not named after one of its variants, plughost-test-<variant>";
pub const GUI_API: &str = "the test editor draws only inside a Cocoa view of the host";
pub const GUI_PARENT: &str = "the host gave the test editor no Cocoa view to draw in";
