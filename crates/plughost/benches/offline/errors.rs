use std::error::Error;

pub type BenchError = Box<dyn Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, BenchError>;

pub const ARGUMENTS: &str = "invalid arguments; use --help for benchmark options";
pub const RELEASE: &str = "the benchmark requires an optimized release build";
pub const THREAD: &str = "the native processing thread panicked";
pub const AUTOMATION: &str = "the benchmark schedule contains an unsupported automation target";
pub const AUDIO: &str = "rendered audio does not match the routing fixture's expected samples";
pub const TIMING: &str = "the routing fixture must report zero latency and tail";
pub const STATE: &str = "the restored fixture state does not match the saved state";

pub fn fail(message: &'static str) -> BenchError {
    std::io::Error::other(message).into()
}

/// The error followed by each cause; plughost errors keep the native reason in `source()`.
pub fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(error) = cause {
        text = format!("{text}: {error}");
        cause = error.source();
    }
    text
}
