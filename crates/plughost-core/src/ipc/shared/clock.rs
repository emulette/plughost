//! The clock the application and the helper both stamp realtime quanta with: mach absolute time
//! on macOS and the performance counter on Windows, which every process reads alike.

use std::time::Duration;

/// The current time in ticks.
#[inline]
pub fn now() -> u64 {
    platform::now()
}

/// Ticks in `duration`, rounded down.
pub fn ticks(duration: Duration) -> u64 {
    let (numerator, denominator) = platform::ticks_per_nanosecond();
    (duration.as_nanos() * u128::from(numerator) / u128::from(denominator))
        .try_into()
        .unwrap_or(u64::MAX)
}

/// The duration of `ticks`, rounded down.
pub fn duration(ticks: u64) -> Duration {
    let (numerator, denominator) = platform::ticks_per_nanosecond();
    let nanoseconds = u128::from(ticks) * u128::from(denominator) / u128::from(numerator);
    Duration::from_nanos(nanoseconds.try_into().unwrap_or(u64::MAX))
}

#[cfg(target_os = "macos")]
mod platform {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }

    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }

    #[inline]
    pub fn now() -> u64 {
        // SAFETY: reads the monotonic clock.
        unsafe { mach_absolute_time() }
    }

    /// Ticks per nanosecond as a fraction: one tick is `numer / denom` nanoseconds.
    pub fn ticks_per_nanosecond() -> (u64, u64) {
        let mut timebase = Timebase { numer: 0, denom: 0 };
        // SAFETY: fills the struct; it never fails on macOS.
        unsafe { mach_timebase_info(&mut timebase) };
        (u64::from(timebase.denom), u64::from(timebase.numer))
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };

    #[inline]
    pub fn now() -> u64 {
        let mut count = 0;
        // SAFETY: writes the counter; it never fails on supported Windows versions.
        unsafe { QueryPerformanceCounter(&mut count) };
        count as u64
    }

    pub fn ticks_per_nanosecond() -> (u64, u64) {
        let mut frequency = 0;
        // SAFETY: writes the frequency; it never fails on supported Windows versions.
        unsafe { QueryPerformanceFrequency(&mut frequency) };
        (frequency as u64, 1_000_000_000)
    }
}
