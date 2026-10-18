use std::error::Error;

pub type BenchError = Box<dyn Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, BenchError>;

pub const ARGUMENTS: &str = "invalid arguments; use --help for realtime benchmark options";
pub const RELEASE: &str = "the realtime benchmark requires an optimized release build";
pub const THREAD: &str = "the callback thread panicked";
pub const STALL: &str = "the stall fixture did not hold up its process call";
pub const LATE: &str = "fail: the helper completed quanta later than the endpoint's latency";
pub const RESYNC: &str = "fail: the endpoint resynchronized";
pub const CALLBACK: &str = "fail: callbacks at p99.9 took longer than the limit";
pub const DEADLINE: &str = "fail: a callback took longer than its quantum";

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
