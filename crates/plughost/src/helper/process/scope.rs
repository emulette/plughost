//! OS containment is owned only by the application; no global process/service enumeration.
use std::io;
use std::process::{Child, Command};

#[cfg(target_os = "macos")]
pub fn configure(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(target_os = "windows")]
pub fn configure(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
}

#[cfg(target_os = "macos")]
pub struct Scope(rustix::process::Pid);

#[cfg(target_os = "macos")]
impl Scope {
    pub fn attach(child: &Child) -> io::Result<Self> {
        Ok(Self(
            rustix::process::Pid::from_raw(child.id() as i32).unwrap(),
        ))
    }
    pub fn exited(&self, _: &Child) -> io::Result<bool> {
        use rustix::process::{WaitId, WaitIdOptions, waitid};
        waitid(
            WaitId::Pid(self.0),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        )
        .map(|status| status.is_some())
        .map_err(Into::into)
    }
    pub fn terminate(&self) {
        let _ = rustix::process::kill_process_group(self.0, rustix::process::Signal::KILL);
    }
}

/// Blocks until the child has exited, without reaping it.
#[cfg(target_os = "macos")]
pub struct Exit(rustix::process::Pid);

#[cfg(target_os = "macos")]
impl Exit {
    pub fn of(child: &Child) -> Self {
        Self(rustix::process::Pid::from_raw(child.id() as i32).unwrap())
    }
    pub fn wait(&self) -> io::Result<()> {
        use rustix::process::{WaitId, WaitIdOptions, waitid};
        loop {
            match waitid(
                WaitId::Pid(self.0),
                WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
            ) {
                Err(rustix::io::Errno::INTR) => continue,
                result => return result.map(|_| ()).map_err(Into::into),
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub struct Scope(std::os::windows::io::OwnedHandle);

#[cfg(target_os = "windows")]
impl Scope {
    pub fn attach(child: &Child) -> io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: unnamed, non-inheritable job; ownership transfers once to OwnedHandle.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let scope = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: structures and process/job handles remain live; no breakaway is enabled.
        if unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        } == 0
            || unsafe { AssignProcessToJobObject(raw, child.as_raw_handle()) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(scope)
    }
    pub fn exited(&self, child: &Child) -> io::Result<bool> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0};
        let result = unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(child.as_raw_handle(), 0)
        };
        if result == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(result == WAIT_OBJECT_0)
    }
    pub fn terminate(&self) {
        use std::os::windows::io::AsRawHandle;
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0.as_raw_handle(), 1)
        };
    }
}

/// Blocks until the child has exited. The process handle belongs to the `Child`, which its owner
/// keeps alive while any `Exit` for it is waiting.
#[cfg(target_os = "windows")]
pub struct Exit(usize);

#[cfg(target_os = "windows")]
impl Exit {
    pub fn of(child: &Child) -> Self {
        use std::os::windows::io::AsRawHandle;
        Self(child.as_raw_handle() as usize)
    }
    pub fn wait(&self) -> io::Result<()> {
        use windows_sys::Win32::Foundation::WAIT_FAILED;
        use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};
        // SAFETY: the handle stays open while this waits, as documented on the type.
        if unsafe { WaitForSingleObject(self.0 as _, INFINITE) } == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
