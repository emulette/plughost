//! The synthetic audio device: a realtime thread that calls back on an absolute grid of quanta,
//! as an audio device's I/O thread does. On macOS it takes the time-constraint policy and waits
//! with `mach_wait_until`; on Windows it joins MMCSS "Pro Audio" and waits on a high-resolution
//! waitable timer. Times are `plughost_core::ipc::shared::clock` ticks.

#[cfg(target_os = "macos")]
mod platform {
    unsafe extern "C" {
        fn mach_wait_until(deadline: u64) -> i32;
    }

    /// Takes the time-constraint policy for quanta of `quantum` ticks, of which the callback may
    /// take a quarter within half of the quantum.
    pub fn schedule(quantum: u64) {
        let quantum = u32::try_from(quantum).unwrap_or(u32::MAX);
        let mut policy = libc::thread_time_constraint_policy {
            period: quantum,
            computation: quantum / 4,
            constraint: quantum / 2,
            preemptible: 1,
        };
        // SAFETY: the policy's words for this thread.
        unsafe {
            libc::thread_policy_set(
                libc::pthread_mach_thread_np(libc::pthread_self()),
                libc::THREAD_TIME_CONSTRAINT_POLICY as u32,
                (&raw mut policy).cast(),
                libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
            )
        };
    }

    pub fn wait_until(deadline: u64) {
        // SAFETY: waits on the mach clock.
        unsafe { mach_wait_until(deadline) };
    }
}

#[cfg(windows)]
mod platform {
    use std::cell::Cell;

    use plughost_core::ipc::shared::clock;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Threading::{
        AvSetMmThreadCharacteristicsW, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
        CreateWaitableTimerExW, INFINITE, SetWaitableTimer, TIMER_ALL_ACCESS, WaitForSingleObject,
    };

    thread_local! {
        static TIMER: Cell<HANDLE> = const { Cell::new(std::ptr::null_mut()) };
    }

    pub fn schedule(_quantum: u64) {
        // `--rate-change` calls this again for each profile; the thread stays in its task.
        if !TIMER.get().is_null() {
            return;
        }
        let name: Vec<u16> = "Pro Audio".encode_utf16().chain([0]).collect();
        let mut index = 0;
        // SAFETY: a NUL-terminated task name; the thread stays in the task until it ends.
        unsafe { AvSetMmThreadCharacteristicsW(name.as_ptr(), &mut index) };
        // SAFETY: creates an unnamed timer; it lives as long as the thread.
        let timer = unsafe {
            CreateWaitableTimerExW(
                std::ptr::null(),
                std::ptr::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS,
            )
        };
        TIMER.set(timer);
    }

    pub fn wait_until(deadline: u64) {
        let remaining = clock::duration(deadline.saturating_sub(clock::now()));
        if remaining.is_zero() {
            return;
        }
        // A negative due time is relative, in units of 100 ns.
        let due = -i64::try_from(remaining.as_nanos() / 100).unwrap_or(i64::MAX);
        let timer = TIMER.get();
        // SAFETY: the thread's timer, set once and waited on.
        unsafe {
            SetWaitableTimer(timer, &due, 0, None, std::ptr::null(), 0);
            WaitForSingleObject(timer, INFINITE);
        }
    }
}

pub use platform::{schedule, wait_until};
