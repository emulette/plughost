//! Lifecycle costs are measured independently from audio renders.
use serde::Serialize;
use std::time::Instant;

pub fn timed<T, E>(call: impl FnOnce() -> Result<T, E>) -> Result<(T, f64), E> {
    let start = Instant::now();
    let value = call()?;
    Ok((value, start.elapsed().as_secs_f64()))
}
#[derive(Serialize)]
pub struct StateCost {
    pub save_seconds: f64,
    pub restore_seconds: f64,
    pub payload_bytes: usize,
}
#[derive(Serialize)]
pub struct Lifecycle {
    pub load_seconds: f64,
    pub prepare_seconds: f64,
    pub states: Vec<StateCost>,
}
