use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::process::{Child, Stdio};

use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use super::super::{
    Activity, Descriptor, SharedAudio,
    errors::{TRANSFER, invalid},
};

pub struct Sender;
pub struct Receiver;

impl Sender {
    pub fn new() -> io::Result<(Self, Stdio)> {
        Ok((Self, Stdio::null()))
    }

    /// Duplicates ownership into the actual child process, not a PID looked up later. The caller
    /// must deliver the token once or terminate/reap the child if notification fails, so an
    /// undelivered duplicate cannot remain in a live helper.
    pub fn send(&self, memory: &SharedAudio, child: &Child) -> io::Result<u64> {
        duplicate(&memory.file, child)
    }

    /// Duplicates the activity mapping the same way, for the load request that carries its token.
    pub fn send_activity(&self, activity: &Activity, child: &Child) -> io::Result<u64> {
        duplicate(&activity.file, child)
    }
}

fn duplicate(file: &File, child: &Child) -> io::Result<u64> {
    let mut handle = std::ptr::null_mut();
    // SAFETY: both borrowed handles remain open for this call. The returned handle is owned by
    // the child and is not closed in this process. It is explicitly non-inheritable.
    let result = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            file.as_raw_handle(),
            child.as_raw_handle(),
            &mut handle,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(handle as usize as u64)
}

impl Receiver {
    pub fn take_from_stdin() -> io::Result<Self> {
        Ok(Self)
    }

    /// Takes ownership of the parent's duplicated file handle.
    ///
    /// # Safety
    /// `token` must be a live file handle duplicated into this process by `Sender::send`, uniquely
    /// owned by this transfer. Receive it exactly once. The parent must never truncate the file
    /// or access its contents other than through `SharedAudio` while mappings are live.
    /// `descriptor` must exactly match the transferred mapping, including its sample precision.
    pub unsafe fn receive(&self, descriptor: Descriptor, token: u64) -> io::Result<SharedAudio> {
        // SAFETY: forwarded from the caller.
        SharedAudio::from_file(unsafe { take(token) }?, descriptor)
    }

    /// Takes ownership of the activity mapping the load request carries.
    ///
    /// # Safety
    /// As for [`Receiver::receive`], with a handle from `Sender::send_activity`.
    pub unsafe fn receive_activity(&self, token: u64) -> io::Result<Activity> {
        // SAFETY: forwarded from the caller.
        Activity::from_file(unsafe { take(token) }?)
    }
}

/// # Safety
/// `token` must be a live file handle uniquely owned by this transfer.
unsafe fn take(token: u64) -> io::Result<File> {
    if token == 0 || token == u64::MAX {
        return Err(invalid(TRANSFER));
    }
    // SAFETY: the authenticated parent transferred unique ownership as required by the caller.
    Ok(unsafe { File::from_raw_handle(token as usize as _) })
}
