use std::error::Error;

pub type BenchError = Box<dyn Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, BenchError>;

pub const ARGUMENTS: &str = "invalid arguments; use --help for comparison options";
pub const RELEASE: &str = "the comparison requires an optimized release build";
pub const NO_CLASS: &str = "no plugin class found for that bundle and index";
pub const THREAD: &str = "the native processing thread panicked";

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
