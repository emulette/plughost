//! The realtime worker's scheduling, for as long as it runs. On macOS the worker takes the
//! time-constraint policy for its quantum, joins a workgroup interval of its own, which it starts
//! and finishes around each quantum, and holds a latency-critical activity that keeps App Nap away
//! from the helper. On Windows it joins the MMCSS "Pro Audio" task. Each is undone when the worker
//! stops. A part the system refuses leaves the worker as it was and is reported in the helper's
//! diagnostics.

use std::time::Duration;

use plughost_core::ipc::shared::clock;

use crate::Responder;

/// The worker's scheduling, undone on drop.
pub(super) struct Schedule {
    /// Each quantum's deadline after its submission, in [`clock`] ticks.
    deadline: u64,
    platform: platform::Schedule,
}

impl Schedule {
    /// Schedules the calling thread for quanta of `quantum` that must complete within
    /// `deadline` ticks of their submission.
    pub fn enter(quantum: Duration, deadline: u64, responder: &Responder) -> Schedule {
        Schedule {
            deadline,
            platform: platform::Schedule::enter(clock::ticks(quantum), responder),
        }
    }

    /// Marks the work on a quantum submitted at `submitted_at` until the returned guard drops.
    pub fn interval(&self, submitted_at: u64) -> Interval<'_> {
        Interval {
            started: self
                .platform
                .start(submitted_at, submitted_at.saturating_add(self.deadline)),
            schedule: self,
        }
    }
}

/// One quantum's work.
pub(super) struct Interval<'a> {
    schedule: &'a Schedule,
    started: bool,
}

impl Drop for Interval<'_> {
    fn drop(&mut self) {
        if self.started {
            self.schedule.platform.finish();
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_char, c_int, c_void};

    use objc2::rc::Retained;
    use objc2::runtime::{NSObjectProtocol, ProtocolObject};
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

    use crate::Responder;
    use crate::errors::{REALTIME_TIME_CONSTRAINT, REALTIME_WORKGROUP};

    const OS_CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;
    const WORKGROUP_NAME: &std::ffi::CStr = c"plughost realtime";
    const ACTIVITY_REASON: &str = "plughost realtime processing";

    /// `os_workgroup_join_token_s` on 64-bit targets.
    #[repr(C)]
    struct JoinToken {
        sig: u32,
        opaque: [u8; 36],
    }

    #[link(name = "AudioToolbox", kind = "framework")]
    unsafe extern "C" {
        fn AudioWorkIntervalCreate(
            name: *const c_char,
            clock: u32,
            attr: *const c_void,
        ) -> *mut c_void;
    }

    unsafe extern "C" {
        fn os_workgroup_join(workgroup: *mut c_void, token: *mut JoinToken) -> c_int;
        fn os_workgroup_leave(workgroup: *mut c_void, token: *mut JoinToken);
        fn os_workgroup_interval_start(
            workgroup: *mut c_void,
            start: u64,
            deadline: u64,
            data: *mut c_void,
        ) -> c_int;
        fn os_workgroup_interval_finish(workgroup: *mut c_void, data: *mut c_void) -> c_int;
        fn os_release(object: *mut c_void);
    }

    /// The joined workgroup interval and its join token.
    struct Workgroup {
        interval: *mut c_void,
        token: JoinToken,
    }

    pub struct Schedule {
        workgroup: Option<Workgroup>,
        activity: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }

    impl Schedule {
        pub fn enter(quantum: u64, responder: &Responder) -> Schedule {
            if !time_constraint(quantum) {
                responder.warn(REALTIME_TIME_CONSTRAINT);
            }
            let workgroup = join();
            if workgroup.is_none() {
                responder.warn(REALTIME_WORKGROUP);
            }
            let options = NSActivityOptions::LatencyCritical
                | NSActivityOptions::UserInitiatedAllowingIdleSystemSleep;
            let activity = NSProcessInfo::processInfo()
                .beginActivityWithOptions_reason(options, &NSString::from_str(ACTIVITY_REASON));
            Schedule {
                workgroup,
                activity,
            }
        }

        /// Starts a workgroup interval; whether it started.
        pub fn start(&self, start: u64, deadline: u64) -> bool {
            self.workgroup.as_ref().is_some_and(|workgroup| {
                // SAFETY: this thread joined the interval, which lives until `self` drops.
                unsafe {
                    os_workgroup_interval_start(
                        workgroup.interval,
                        start,
                        deadline,
                        std::ptr::null_mut(),
                    ) == 0
                }
            })
        }

        pub fn finish(&self) {
            if let Some(workgroup) = &self.workgroup {
                // SAFETY: as in `start`, after an interval started.
                unsafe { os_workgroup_interval_finish(workgroup.interval, std::ptr::null_mut()) };
            }
        }
    }

    impl Drop for Schedule {
        fn drop(&mut self) {
            if let Some(mut workgroup) = self.workgroup.take() {
                // SAFETY: this thread joined the interval with `token`; leaving releases nothing
                // else, and the interval's own reference is released last.
                unsafe {
                    os_workgroup_leave(workgroup.interval, &mut workgroup.token);
                    os_release(workgroup.interval);
                }
            }
            // SAFETY: the activity came from `beginActivityWithOptions_reason`.
            unsafe { NSProcessInfo::processInfo().endActivity(&self.activity) };
            default_policy();
        }
    }

    /// Takes the time-constraint policy for periods of `quantum` ticks, of which the work may
    /// take half; whether the system granted it.
    fn time_constraint(quantum: u64) -> bool {
        let quantum = u32::try_from(quantum).unwrap_or(u32::MAX);
        let mut policy = libc::thread_time_constraint_policy {
            period: quantum,
            computation: quantum / 2,
            constraint: quantum,
            preemptible: 1,
        };
        set_policy(
            libc::THREAD_TIME_CONSTRAINT_POLICY,
            (&raw mut policy).cast(),
            libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    }

    /// Returns the thread to the standard policy.
    fn default_policy() {
        let mut policy = libc::thread_standard_policy { no_data: 0 };
        set_policy(
            libc::THREAD_STANDARD_POLICY,
            (&raw mut policy).cast(),
            libc::THREAD_STANDARD_POLICY_COUNT as u32,
        );
    }

    fn set_policy(flavor: c_int, policy: libc::thread_policy_t, count: u32) -> bool {
        // SAFETY: `policy` points to `count` words of the flavor's policy, for this thread.
        unsafe {
            libc::thread_policy_set(
                libc::pthread_mach_thread_np(libc::pthread_self()),
                flavor as u32,
                policy,
                count,
            ) == libc::KERN_SUCCESS
        }
    }

    /// Creates a workgroup interval on the mach clock and joins it.
    fn join() -> Option<Workgroup> {
        // SAFETY: the name is a static C string and the attributes default.
        let interval = unsafe {
            AudioWorkIntervalCreate(
                WORKGROUP_NAME.as_ptr(),
                OS_CLOCK_MACH_ABSOLUTE_TIME,
                std::ptr::null(),
            )
        };
        if interval.is_null() {
            return None;
        }
        let mut workgroup = Workgroup {
            interval,
            token: JoinToken {
                sig: 0,
                opaque: [0; 36],
            },
        };
        // SAFETY: a valid interval, and a token of the size the system expects.
        if unsafe { os_workgroup_join(workgroup.interval, &mut workgroup.token) } != 0 {
            // SAFETY: the interval was created retained and is not used again.
            unsafe { os_release(workgroup.interval) };
            return None;
        }
        Some(workgroup)
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Threading::{
        AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
    };

    use crate::Responder;
    use crate::errors::REALTIME_MMCSS;

    pub struct Schedule {
        task: HANDLE,
    }

    impl Schedule {
        pub fn enter(_quantum: u64, responder: &Responder) -> Schedule {
            let name: Vec<u16> = "Pro Audio".encode_utf16().chain([0]).collect();
            let mut index = 0;
            // SAFETY: a NUL-terminated task name and a valid index.
            let task = unsafe { AvSetMmThreadCharacteristicsW(name.as_ptr(), &mut index) };
            if task.is_null() {
                responder.warn(REALTIME_MMCSS);
            }
            Schedule { task }
        }

        /// MMCSS has no intervals.
        pub fn start(&self, _start: u64, _deadline: u64) -> bool {
            false
        }

        pub fn finish(&self) {}
    }

    impl Drop for Schedule {
        fn drop(&mut self) {
            if !self.task.is_null() {
                // SAFETY: the task handle this thread joined.
                unsafe { AvRevertMmThreadCharacteristics(self.task) };
            }
        }
    }
}
