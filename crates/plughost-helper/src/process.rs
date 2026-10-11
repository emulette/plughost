//! Processing on a thread of its own, which owns the chain's processing state: the processors,
//! the shared slots, prepared storage, alignment delays and event buffers. The blocks the reader
//! thread forwards and the commands of the main thread wake it through one bounded queue and run
//! in arrival order, so a command takes effect between blocks. The main thread builds what a
//! command installs and drops what it replaces, which the processing thread hands back in the
//! command. The processing thread never waits for the main thread, which keeps running the
//! editors and the event loop while blocks are processed.

mod audio;
pub(crate) mod audio_timing;
mod channel_views;
pub(crate) mod events;
mod parameters;
pub(crate) mod prepared;
pub(crate) mod shared;

use std::io;
use std::mem::swap;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use plughost_core::ipc::shared::{Caller, Submission};
use plughost_core::{BlockContext, ChangeBatch, InputError, PluginTiming, RoutedChainConfig};
use plughost_formats::BlockProcessor;

use crate::Responder;
use crate::calls::Calls;
use audio_timing::{Alignment, AudioTiming, DelayReplacement};
pub(crate) use parameters::Parameters;

/// Blocks and commands waiting for the processing thread. The application sends one block at a
/// time, and the main thread waits for each command it needs answered.
const QUEUE: usize = 64;

/// What block processing needs from the chain, which the main thread changes with commands.
#[derive(Default)]
pub struct Pipeline {
    pub processors: Vec<Box<dyn BlockProcessor>>,
    pub prepared: Option<prepared::Prepared>,
    pub transport: Option<shared::Transport>,
    pub audio_config: Option<RoutedChainConfig>,
    pub alignment: Option<Alignment>,
    /// Event routing and its block storage, planned with the audio routing.
    pub events: Option<(events::EventPlan, events::EventBuffers)>,
    /// Events overflowed a block's budget; processing waits for a reset, which clears the notes
    /// slots may still hold.
    pub resync: bool,
    pub changes: plughost_core::ChangeBuffer,
    /// Each slot's parameter metadata, against which block automation is validated.
    ///
    /// The main thread refreshes a slot's entry whenever the plugin reports changed metadata: on
    /// every idle tick, and before it answers any request. The application sends a block only
    /// after the previous response, and commands run in the order they were sent, so a block is
    /// validated against metadata at least as new as every request answered before it. A change
    /// the plugin makes on its own (from a timer or its editor) is seen at the next tick; a block
    /// processed in between, or while a plugin holds up the main thread, is validated against the
    /// previous metadata.
    pub parameters: Vec<Parameters>,
    pub calls: Calls,
}

/// What the processing thread does next.
pub(crate) enum Work {
    /// A block the application sent.
    Block {
        context: BlockContext,
        submission: Submission,
    },
    /// A slot's timing the main thread read, for the application's changes.
    Observe(usize, PluginTiming),
    /// A change the main thread waits for; it gets the command back.
    Command(Command),
}

/// A change of the processing state. The processing thread swaps what a command carries with
/// what it replaces, so the command handed back holds the replaced state for the main thread to
/// drop.
pub(crate) enum Command {
    /// The pipeline of a newly loaded chain.
    Load(Box<Pipeline>),
    Processors(Vec<Box<dyn BlockProcessor>>),
    Calls(Calls),
    Prepare(Box<Prepare>),
    Replace(Box<Replace>),
    /// The alignment planned after a native reset, which also ended the notes slots held.
    Reset(Option<Alignment>),
    Parameters(usize, Parameters),
    /// Checks the complete render's distinct targets using the same metadata as block checks.
    ValidateAutomation(Vec<(usize, u64)>, Option<(usize, InputError)>),
    /// Takes the timing changes collected so far.
    Changes(Option<ChangeBatch>),
    /// Runs one of a slot's audio-thread calls.
    Run(usize, Call),
}

/// A prepared audio configuration.
pub(crate) struct Prepare {
    /// The processors of fresh instances, when preparing again replaced them.
    pub processors: Option<Vec<Box<dyn BlockProcessor>>>,
    pub prepared: Option<prepared::Prepared>,
    pub transport: Option<shared::Transport>,
    pub audio_config: Option<RoutedChainConfig>,
    pub alignment: Option<Alignment>,
    pub events: Option<(events::EventPlan, events::EventBuffers)>,
    pub parameters: Vec<Parameters>,
    pub timings: Vec<PluginTiming>,
}

/// A slot's new instance, retimed when the chain is prepared.
pub(crate) struct Replace {
    pub slot: usize,
    pub processor: Box<dyn BlockProcessor>,
    pub retime: Option<Retime>,
}

pub(crate) struct Retime {
    pub parameters: Parameters,
    pub timing: PluginTiming,
    pub delays32: Option<DelayReplacement<f32>>,
    pub delays64: Option<DelayReplacement<f64>>,
    pub plan: AudioTiming,
}

impl Pipeline {
    fn apply(&mut self, command: &mut Command) {
        match command {
            Command::Load(pipeline) => swap(self, pipeline),
            Command::Processors(processors) => swap(&mut self.processors, processors),
            Command::Calls(calls) => swap(&mut self.calls, calls),
            Command::Prepare(prepare) => {
                if let Some(processors) = &mut prepare.processors {
                    swap(&mut self.processors, processors);
                }
                swap(&mut self.prepared, &mut prepare.prepared);
                swap(&mut self.transport, &mut prepare.transport);
                swap(&mut self.audio_config, &mut prepare.audio_config);
                swap(&mut self.alignment, &mut prepare.alignment);
                swap(&mut self.events, &mut prepare.events);
                swap(&mut self.parameters, &mut prepare.parameters);
                for (slot, timing) in prepare.timings.iter().enumerate() {
                    self.changes.observe(slot, *timing);
                }
                self.resync = false;
            }
            Command::Replace(replace) => {
                swap(&mut self.processors[replace.slot], &mut replace.processor);
                if let (Some(retime), Some(alignment)) = (&mut replace.retime, &mut self.alignment)
                {
                    swap(&mut self.parameters[replace.slot], &mut retime.parameters);
                    self.changes.observe(replace.slot, retime.timing);
                    if let (Some(bank), Some(delays)) =
                        (&mut alignment.delays32, &mut retime.delays32)
                    {
                        bank.replace_slot(delays);
                    }
                    if let (Some(bank), Some(delays)) =
                        (&mut alignment.delays64, &mut retime.delays64)
                    {
                        bank.replace_slot(delays);
                    }
                    swap(&mut alignment.timing, &mut retime.plan);
                }
            }
            Command::Reset(alignment) => {
                self.resync = false;
                swap(&mut self.alignment, alignment);
            }
            Command::Parameters(slot, parameters) => {
                if let Some(cached) = self.parameters.get_mut(*slot) {
                    swap(cached, parameters);
                }
            }
            Command::Changes(changes) => *changes = Some(self.changes.take()),
            Command::ValidateAutomation(targets, failure) => {
                *failure = targets.iter().find_map(|&(slot, id)| {
                    self.parameters
                        .get(slot)
                        .ok_or(InputError::Slot)
                        .and_then(|parameters| parameters.get(id))
                        .and_then(|info| info.automation_value(0.0))
                        .err()
                        .map(|error| (slot, error))
                });
            }
            Command::Run(slot, call) => {
                let _call = self.calls.enter(Caller::Processing, *slot);
                crate::events::with_pool(|| call.run());
            }
        }
    }
}

/// The main thread's way to the processing thread.
pub(crate) struct Processing {
    work: SyncSender<Work>,
    /// Commands handed back; the main thread waits for each before it sends the next.
    returned: Mutex<Receiver<Command>>,
    thread: ThreadId,
}

impl Processing {
    /// Starts the processing thread, which answers blocks through `responder`. Blocks go to the
    /// returned sender.
    pub fn spawn(responder: Responder) -> io::Result<(Arc<Processing>, SyncSender<Work>)> {
        let (work, queue) = mpsc::sync_channel(QUEUE);
        let (handed_back, returned) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("plughost processing".to_owned())
            .spawn(move || serve(&queue, &handed_back, &responder))?;
        let processing = Processing {
            work: work.clone(),
            returned: Mutex::new(returned),
            thread: thread.thread().id(),
        };
        Ok((Arc::new(processing), work))
    }

    /// Runs `command` between blocks and returns it with the state it replaced. A helper whose
    /// processing thread is gone cannot go on.
    pub fn request(&self, command: Command) -> Command {
        let returned = self
            .returned
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.work.send(Work::Command(command)).is_err() {
            crate::lifetime::disconnected();
        }
        returned
            .recv()
            .unwrap_or_else(|_| crate::lifetime::disconnected())
    }

    /// Records a slot's timing without waiting. A full queue skips it: the next idle tick reads
    /// the timing again.
    pub fn observe(&self, slot: usize, timing: PluginTiming) {
        let _ = self.work.try_send(Work::Observe(slot, timing));
    }
}

fn serve(queue: &Receiver<Work>, handed_back: &SyncSender<Command>, responder: &Responder) {
    let mut pipeline = Pipeline::default();
    while let Ok(work) = queue.recv() {
        let _audit = plughost_core::audit::processing();
        match work {
            Work::Block {
                context,
                submission,
            } => {
                let processed = crate::events::with_pool(|| {
                    shared::process(&mut pipeline, &context, submission, responder)
                });
                // The connection closed; the reader thread ends the helper.
                if processed.is_err() {
                    return;
                }
            }
            Work::Observe(slot, timing) => pipeline.changes.observe(slot, timing),
            Work::Command(mut command) => {
                pipeline.apply(&mut command);
                if handed_back.send(command).is_err() {
                    return;
                }
            }
        }
    }
}

/// A slot's audio thread for its plugin: the processing thread (see
/// [`plughost_formats::AudioThread`]).
pub(crate) struct AudioThread {
    processing: Arc<Processing>,
    slot: usize,
}

impl AudioThread {
    pub fn new(processing: &Arc<Processing>, slot: usize) -> Arc<AudioThread> {
        Arc::new(AudioThread {
            processing: Arc::clone(processing),
            slot,
        })
    }
}

impl plughost_formats::AudioThread for AudioThread {
    fn run(&self, call: &mut (dyn FnMut() + Send)) {
        if thread::current().id() == self.processing.thread {
            return call();
        }
        // SAFETY: `request` returns only after the processing thread has run the call.
        let call = unsafe { Call::new(call) };
        self.processing.request(Command::Run(self.slot, call));
    }
}

/// A call the main thread lends the processing thread while it waits for it.
pub(crate) struct Call(*mut (dyn FnMut() + Send + 'static));

// SAFETY: the call itself is `Send`, and its lender waits until it has run.
unsafe impl Send for Call {}

impl Call {
    /// # Safety
    ///
    /// The lender must not use or drop `call` until the processing thread has run it.
    unsafe fn new(call: &mut (dyn FnMut() + Send)) -> Call {
        // SAFETY: only the lifetime changes; the caller keeps `call` alive while it is lent.
        Call(unsafe {
            std::mem::transmute::<*mut (dyn FnMut() + Send + '_), *mut (dyn FnMut() + Send + 'static)>(
                call,
            )
        })
    }

    fn run(&mut self) {
        // SAFETY: the lender waits, so the call is alive and borrowed by no one else.
        unsafe { (*self.0)() }
    }
}
