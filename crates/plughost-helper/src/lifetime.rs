//! Parent death and connection loss must not depend on a responsive plugin/main thread.
use std::io;

#[cfg(target_os = "macos")]
pub fn watch_parent(parent: u32) -> io::Result<()> {
    use rustix::process::{getpgrp, getpid, getppid};
    if getpgrp() != getpid() || getppid().map(|pid| pid.as_raw_pid() as u32) != Some(parent) {
        return Err(io::Error::other(crate::errors::PROCESS_SCOPE));
    }
    std::thread::spawn(move || {
        loop {
            if getppid().map(|pid| pid.as_raw_pid() as u32) != Some(parent) {
                disconnected();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn disconnected() -> ! {
    use rustix::process::{Signal, getpid, kill_process_group};
    // Startup checked ownership. Never inspect/terminate unrelated licensing services.
    let _ = kill_process_group(getpid(), Signal::KILL);
    std::process::exit(1)
}

#[cfg(target_os = "windows")]
pub fn watch_parent(parent: u32) -> io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, parent) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    std::thread::spawn(move || {
        unsafe { WaitForSingleObject(handle.as_raw_handle(), INFINITE) };
        disconnected();
    });
    Ok(())
}

/// Terminates at once, without running exit handlers or plugin DLL detach code, which could wait
/// on locks a hung main thread holds. The application's kill-on-close job ends the rest.
#[cfg(target_os = "windows")]
pub fn disconnected() -> ! {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    // SAFETY: the pseudo handle always refers to this process.
    unsafe { TerminateProcess(GetCurrentProcess(), 1) };
    unreachable!()
}
