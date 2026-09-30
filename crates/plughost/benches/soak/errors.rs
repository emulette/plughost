use std::error::Error;

pub type BenchError = Box<dyn Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, BenchError>;

pub const ARGUMENTS: &str = "invalid arguments; use --help for soak options";
pub const RELEASE: &str = "the soak run requires an optimized release build";
pub const NO_CLASS: &str = "no plugin class found in that bundle";

pub fn fail(message: &'static str) -> BenchError {
    std::io::Error::other(message).into()
}
