//! A process's CPU time and memory, read with the platform's process tools: `ps` and the physical
//! footprint on macOS (resident size there also counts mapped plugin binaries), and the processor
//! time and working set on Windows.

use std::process::Command;
use std::time::Duration;

/// What a process used so far.
pub struct Usage {
    pub cpu: Duration,
    pub memory_mib: f64,
}

#[cfg(target_os = "macos")]
pub fn usage(pid: u32) -> Option<Usage> {
    let pid = pid.to_string();
    // "MM:SS.ss" or "HH:MM:SS.ss"
    let time = Command::new("ps")
        .args(["-o", "time=", "-p", &pid])
        .output()
        .ok()?;
    let seconds = String::from_utf8_lossy(&time.stdout)
        .trim()
        .split(':')
        .try_fold(0.0, |total, part| {
            Some(total * 60.0 + part.parse::<f64>().ok()?)
        })?;
    let footprint = Command::new("footprint")
        .args(["-f", "bytes", &pid])
        .output()
        .ok()?;
    // "phys_footprint: 13896640 B"
    let bytes: f64 = String::from_utf8_lossy(&footprint.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix("phys_footprint: "))?
        .trim_end_matches(" B")
        .parse()
        .ok()?;
    Some(Usage {
        cpu: Duration::from_secs_f64(seconds),
        memory_mib: bytes / 1_048_576.0,
    })
}

#[cfg(windows)]
pub fn usage(pid: u32) -> Option<Usage> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "$p = Get-Process -Id {pid}; \"$($p.TotalProcessorTime.TotalSeconds) $($p.WorkingSet64)\""
            ),
        ])
        .output()
        .ok()?;
    let output = String::from_utf8_lossy(&output.stdout);
    let mut values = output.split_whitespace().map(str::parse::<f64>);
    let (Some(Ok(seconds)), Some(Ok(bytes))) = (values.next(), values.next()) else {
        return None;
    };
    Some(Usage {
        cpu: Duration::from_secs_f64(seconds),
        memory_mib: bytes / 1_048_576.0,
    })
}
