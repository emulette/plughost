//! The control thread of the fault-injection runs (`--crash`, `--hang`, `--overflow`,
//! `--rate-change`): it injects the fault, notices it, brings the endpoint back and records how
//! long that took and how many quanta the callback played its fallback meanwhile, while the
//! callback thread goes on.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use plughost::{Chain, ChainSnapshot, Layout, RealtimeConfig, RealtimeEndpoint};

use crate::Options;
use crate::callback::{Handover, Measured, Profile, Shared};
use crate::errors::{self, Result};

/// The control thread's tick.
const TICK: Duration = Duration::from_millis(10);
/// How often the control thread saves a snapshot while the helper is well.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);
/// The helper is well while it has completed a quantum within this long, when one is due.
const HEALTHY: Duration = Duration::from_millis(20);
/// How long a recovered or restarted endpoint may take to play its first processed output.
const COME_BACK: Duration = Duration::from_secs(10);

/// What a run injects.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Kills the helper process.
    Crash,
    /// Holds up the helper's processing: the stall fixture does not return.
    Hang,
    /// Makes the callback submit more events than a block may hold, which starts a resync.
    Overflow,
    /// Stops the endpoint, prepares the chain at the other profile and starts a new endpoint.
    RateChange,
}

/// How one fault went.
pub struct Recovery {
    /// From noticing the fault (for a rate change, from the stop) to the first processed output
    /// of the new generation.
    pub time: Duration,
    /// Callbacks that played the fallback from the injection to that output.
    pub fallbacks: u64,
}

#[derive(Default)]
pub struct Report {
    pub injected: u64,
    pub recoveries: Vec<Recovery>,
    pub snapshots: u64,
    /// Why the run could not go on, when a fault was not recovered from.
    pub failure: Option<String>,
}

/// The barrier each helper's stall fixture announces its stall at. The first `limit` stalls are
/// held until the control thread has recovered from them; later ones, and all after
/// [`Hang::open`], are released at once.
pub struct Hang {
    address: String,
    entered: Receiver<TcpStream>,
    open: Arc<AtomicBool>,
    held: Vec<TcpStream>,
}

impl Hang {
    pub fn bind(limit: u64) -> Result<Hang> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?.to_string();
        let (sender, entered) = channel();
        let open = Arc::new(AtomicBool::new(false));
        let opened = Arc::clone(&open);
        thread::spawn(move || {
            let mut stalls = 0;
            while let Ok((mut connection, _)) = listener.accept() {
                let mut signal = [0];
                if connection.read_exact(&mut signal).is_err() || signal != [1] {
                    continue;
                }
                if stalls < limit && !opened.load(Ordering::Relaxed) {
                    stalls += 1;
                    if sender.send(connection).is_err() {
                        break;
                    }
                } else {
                    let _ = connection.write_all(&[0]);
                }
            }
        });
        Ok(Hang {
            address,
            entered,
            open,
            held: Vec::new(),
        })
    }

    /// The address the stall fixture finds in the host name.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Stops holding stalls.
    fn open(&self) {
        self.open.store(true, Ordering::Relaxed);
    }

    /// Whether a stall entered since the last call.
    fn entered(&mut self) -> bool {
        match self.entered.try_recv() {
            Ok(connection) => {
                self.held.push(connection);
                true
            }
            Err(_) => false,
        }
    }

    /// Lets the stalled helpers' process calls return.
    fn release(&mut self) {
        for mut connection in self.held.drain(..) {
            let _ = connection.write_all(&[0]);
        }
    }
}

pub struct Faults<'a> {
    pub fault: Fault,
    pub options: &'a Options,
    pub shared: &'a Shared,
    /// Taken before the run started, for `--crash` and `--hang`.
    pub snapshot: Option<ChainSnapshot>,
    /// For `--hang`.
    pub hang: Option<Hang>,
    /// For `--rate-change`: the profiles to alternate between, starting with the second, and the
    /// callback thread's channels for its endpoints.
    pub profiles: [Profile; 2],
    pub handover: Option<Sender<Handover>>,
    pub retired: Option<Receiver<RealtimeEndpoint>>,
}

impl Faults<'_> {
    /// Runs the control thread until `options.seconds` have passed, any fault in progress is
    /// recovered from, and the callback thread has ended, or until a fault is not recovered from.
    pub fn run(
        mut self,
        chain: &mut Chain,
        callback: &JoinHandle<Measured>,
        started: Instant,
    ) -> Report {
        let mut report = Report::default();
        if let Err(error) = self.control(chain, callback, started, &mut report) {
            report.failure = Some(errors::describe(&*error));
        }
        if let Some(hang) = &mut self.hang {
            hang.release();
        }
        self.shared.stop.store(true, Ordering::Relaxed);
        report
    }

    fn control(
        &mut self,
        chain: &mut Chain,
        callback: &JoinHandle<Measured>,
        started: Instant,
        report: &mut Report,
    ) -> Result<()> {
        let seconds = Duration::from_secs_f64(self.options.seconds);
        let hang = self.options.hang;
        let mut monitor = chain.helper_monitor();
        let mut taken = Instant::now();
        // When the last fault was recovered from, or the run started.
        let mut last = taken;
        // The fallback count at the injection, while a fault is injected and not yet recovered from.
        let mut injected = None;
        let mut profile = 1;
        while !callback.is_finished() {
            thread::sleep(TICK);
            let ending = started.elapsed() >= seconds;
            let status = chain.realtime_status().unwrap_or_default();
            let healthy = status.running && !status.resync && status.stalled < HEALTHY;
            if ending && let Some(hang) = &self.hang {
                hang.open();
            }
            if let Some(hang) = &mut self.hang
                && hang.entered()
            {
                report.injected += 1;
                injected = Some(self.shared.fallbacks());
            }
            if let Some(snapshot) = &mut self.snapshot
                && injected.is_none()
                && healthy
                && taken.elapsed() >= SNAPSHOT_INTERVAL
            {
                *snapshot = chain.snapshot()?;
                taken = Instant::now();
                report.snapshots += 1;
            }
            let Some(at) = injected else {
                let due = last.elapsed() >= self.options.fault_interval
                    && self
                        .options
                        .faults
                        .is_none_or(|faults| report.injected < faults);
                if ending {
                    break;
                }
                if !healthy || !due {
                    continue;
                }
                match self.fault {
                    Fault::Crash => {
                        injected = Some(self.shared.fallbacks());
                        report.injected += 1;
                        terminate(monitor.process_id())?;
                    }
                    Fault::RateChange => {
                        report.injected += 1;
                        let next = self.profiles[profile % 2];
                        self.change(chain, next, report)?;
                        profile += 1;
                        last = Instant::now();
                    }
                    Fault::Overflow => {
                        injected = Some(self.shared.fallbacks());
                        report.injected += 1;
                        self.shared.overflow.store(true, Ordering::Relaxed);
                    }
                    Fault::Hang => {}
                }
                continue;
            };
            // A fault is in progress: recover from it once it shows.
            let shows = match self.fault {
                Fault::Crash => monitor.exit_status().is_some(),
                Fault::Overflow => status.resync,
                _ => status.stalled >= hang,
            };
            if shows {
                if self.fault == Fault::Overflow {
                    self.resync(chain, at, report)?;
                } else {
                    self.recover(chain, at, report)?;
                    monitor = chain.helper_monitor();
                }
                injected = None;
                last = Instant::now();
            }
        }
        Ok(())
    }

    /// Recovers `chain`'s endpoint in a new helper from the snapshot, as an application does on
    /// noticing a crashed or hung helper, and waits for its first processed output. The old chain
    /// is dropped only then, after its helper's stall is released.
    fn recover(&mut self, chain: &mut Chain, injected: u64, report: &mut Report) -> Result<()> {
        let noticed = Instant::now();
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| errors::fail(errors::SNAPSHOT))?;
        let recovered = chain.recover_realtime(snapshot)?;
        let old = std::mem::replace(chain, recovered);
        let processed = self.shared.processed();
        let came_back = self.await_processed(chain, processed);
        let time = noticed.elapsed();
        if let Some(hang) = &mut self.hang {
            hang.release();
        }
        drop(old);
        came_back?;
        report.recoveries.push(Recovery {
            time,
            fallbacks: self.shared.fallbacks() - injected,
        });
        Ok(())
    }

    /// Resynchronizes the endpoint without stopping it, as an application does on seeing `resync`
    /// set, and waits for its first processed output.
    fn resync(&mut self, chain: &mut Chain, injected: u64, report: &mut Report) -> Result<()> {
        let noticed = Instant::now();
        chain.resync_realtime()?;
        let processed = self.shared.processed();
        self.await_processed(chain, processed)?;
        report.recoveries.push(Recovery {
            time: noticed.elapsed(),
            fallbacks: self.shared.fallbacks() - injected,
        });
        Ok(())
    }

    /// Stops the endpoint, prepares the chain at `profile`, creates a new endpoint for it and
    /// hands it to the callback thread, which takes it between two callbacks.
    fn change(&mut self, chain: &mut Chain, profile: Profile, report: &mut Report) -> Result<()> {
        let injected = self.shared.fallbacks();
        let stopped = Instant::now();
        chain.stop_realtime()?;
        let config = chain.main_bus_config(
            profile.rate,
            profile.quantum,
            Layout::Stereo,
            &[Layout::Stereo],
        )?;
        chain.prepare_audio(&config)?;
        let config = RealtimeConfig::new(profile.quantum).with_latency(self.options.latency);
        let endpoint = chain.realtime_endpoint(&config)?;
        chain.start_realtime()?;
        let processed = self.shared.processed();
        self.handover
            .as_ref()
            .ok_or_else(|| errors::fail(errors::THREAD))?
            .send(Handover { endpoint, profile })
            .map_err(|_| errors::fail(errors::THREAD))?;
        self.await_processed(chain, processed)?;
        report.recoveries.push(Recovery {
            time: stopped.elapsed(),
            fallbacks: self.shared.fallbacks() - injected,
        });
        // The callback thread has left its old endpoint; free it here.
        if let Some(retired) = &self.retired {
            retired.try_iter().for_each(drop);
        }
        Ok(())
    }

    /// Waits until the callback thread has played processed output beyond the `processed` count.
    fn await_processed(&self, chain: &Chain, processed: u64) -> Result<()> {
        let start = Instant::now();
        while self.shared.processed() <= processed {
            if start.elapsed() > COME_BACK {
                return Err(errors::fail(errors::COME_BACK));
            }
            // This also releases the ring the endpoint left after a recovery.
            chain.realtime_status();
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}

/// Kills the helper process, as a crash does.
fn terminate(pid: u32) -> Result<()> {
    #[cfg(target_os = "macos")]
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid as i32).ok_or_else(|| errors::fail(errors::PROCESS))?,
        rustix::process::Signal::KILL,
    )
    .map_err(std::io::Error::from)?;
    #[cfg(target_os = "windows")]
    // SAFETY: opens the helper's process for termination, terminates it and closes the handle.
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let terminated = TerminateProcess(process, 17);
        CloseHandle(process);
        if terminated == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
