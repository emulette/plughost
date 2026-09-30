//! Opt-in state-stream fixture. Normal gain writes 8 MiB; gain zero attempts 129 MiB.
//! Gain 0.125 never returns from the native save call, for helper timeout/restart verification.
//! The native adapters deliberately ignore rejected writes and report save success.

pub fn write(gain: f64, mut write: impl FnMut(&[u8])) {
    if gain == 0.125 {
        loop {
            std::thread::park();
        }
    }
    let chunks = if gain == 0.0 { 129 * 16 } else { 8 * 16 };
    for index in 0..chunks {
        write(&[index as u8; 65536]);
    }
}
