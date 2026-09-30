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
