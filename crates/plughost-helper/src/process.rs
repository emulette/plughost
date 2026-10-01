//! Processing on the IPC thread: blocks run through the chain as they arrive, while the main
//! thread runs the editors and the event loop. They never wait for the main thread, except for the
//! short moments it holds [`Pipeline`] to change the chain.

mod audio;
pub(crate) mod audio_timing;
pub(crate) mod events;
pub(crate) mod prepared;
pub(crate) mod shared;
use std::sync::{Arc, Mutex, MutexGuard};

use plughost_core::ipc::shared::Submission;
use plughost_core::{BlockContext, Event};
use plughost_formats::BlockProcessor;

use crate::Responder;
use crate::calls::Calls;
use plughost_core::messages::CHAIN_NOT_PREPARED;

/// What block processing needs from the chain, kept current by the main thread.
#[derive(Default)]
pub struct Pipeline {
    pub processors: Vec<Box<dyn BlockProcessor>>,
    pub prepared: Option<prepared::Prepared>,
    pub transport: Option<shared::Transport>,
    pub audio_config: Option<plughost_core::RoutedChainConfig>,
    pub audio_slots: Vec<plughost_core::AudioConfig>,
    pub alignment: Option<audio_timing::Alignment>,
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
    /// after the previous response, so a block is validated against metadata at least as new as
    /// every request answered before it. A change the plugin makes on its own (from a timer or
    /// its editor) is seen at the next tick; a block processed in between, or while a plugin
    /// holds up the main thread, is validated against the previous metadata.
    pub parameters: Vec<Vec<plughost_core::ParameterInfo>>,
    pub calls: Calls,
}

pub type Shared = Arc<Mutex<Pipeline>>;

pub fn lock(pipeline: &Shared) -> MutexGuard<'_, Pipeline> {
    pipeline
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Processes one block the application sent and answers it. An error means the connection closed.
pub fn block(
    pipeline: &Shared,
    context: &BlockContext,
    submission: Submission,
    responder: &Responder,
) -> std::io::Result<()> {
    shared::process(&mut lock(pipeline), context, submission, responder)
}
