//! A chain of plugins processed in order in one helper process.

mod audio;
mod shared;
mod state;

use std::path::{Path, PathBuf};
use std::time::Duration;

use plughost_core::CapabilityReport;
use plughost_core::PluginRef;
use plughost_core::ipc::{Request, Response};
use plughost_core::render::Tail;
use plughost_core::{HostIdentity, MidiEvent, ParameterInfo, PluginInfo, PluginState, events_fit};

use crate::errors::Error;
use crate::helper::{Helper, Mode};

/// How long each kind of operation may go without a response from the helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// Loading plugins and restoring state; large instruments take minutes.
    pub load: Duration,
    /// Preparing, resetting, saving state, and parameter access.
    pub control: Duration,
    /// Processing one block.
    pub process: Duration,
}

impl Default for Timeouts {
    fn default() -> Timeouts {
        Timeouts {
            load: Duration::from_secs(300),
            control: Duration::from_secs(60),
            process: Duration::from_secs(10),
        }
    }
}

/// Plugins processed in order, isolated in their own helper process. If a plugin crashes or
/// hangs, the chain's methods return [`Error::Crashed`] or [`Error::TimedOut`] and the application
/// decides whether to [`Chain::recover`].
pub struct Chain {
    identity: HostIdentity,
    helper_path: PathBuf,
    helper: Helper,
    plugins: Vec<PluginRef>,
    infos: Vec<PluginInfo>,
    timeouts: Timeouts,
    config: Option<plughost_core::RoutedChainConfig>,
    shared: Option<shared::Transport>,
    generation: u64,
    latency: u32,
    tail: Tail,
}

impl Chain {
    /// Starts a helper and loads `plugins` into it, in chain order. `host` identifies the calling
    /// application to plugins and is retained across resets and recovery.
    pub fn spawn(
        helper: &Path,
        plugins: &[PluginRef],
        host: &HostIdentity,
        timeouts: Timeouts,
    ) -> Result<Chain, Error> {
        host.validate()
            .map_err(|error| Error::Input { slot: None, error })?;
        if plugins.is_empty() {
            return Err(Error::Input {
                slot: None,
                error: plughost_core::InputError::EmptyChain,
            });
        }
        start(helper, plugins, &[], host, timeouts)
    }

    /// Starts a separate chain with this chain's helper, plugins, host identity and timeouts,
    /// restores `states` (saved with [`plughost_core::StatePurpose::Project`], one per plugin in chain order, or
    /// none for initial states) as each plugin loads, and prepares it with this chain's
    /// configuration when this chain has one. This chain is left as it is, crashed or not: the
    /// application switches to the returned chain when it is ready, and keeps this one to retry
    /// after a failure. The recovered chain holds only what the states hold: edits made after
    /// they were saved and DSP history such as tails are not recovered, and editors are closed.
    pub fn recover(&self, states: &[PluginState]) -> Result<Chain, Error> {
        if !states.is_empty() && states.len() != self.plugins.len() {
            return Err(Error::Input {
                slot: None,
                error: plughost_core::InputError::StateCount,
            });
        }
        for (slot, state) in states.iter().enumerate() {
            state.validate().map_err(|error| Error::Input {
                slot: Some(slot),
                error,
            })?;
        }
        let mut chain = start(
            &self.helper_path,
            &self.plugins,
            states,
            &self.identity,
            self.timeouts,
        )?;
        if let Some(config) = &self.config {
            chain.prepare_audio(config)?;
        }
        Ok(chain)
    }

    pub fn plugins(&self) -> &[PluginInfo] {
        &self.infos
    }

    /// Observes this helper's exit without issuing IPC requests, including while the chain is
    /// idle. The handle stays attached to this helper after recovery or Chain drop.
    pub fn helper_monitor(&self) -> crate::HelperMonitor {
        self.helper.monitor()
    }

    /// Drains bounded plugin and helper diagnostics. Poll from application/control code.
    /// Unread child-local logs are lost when the helper exits.
    pub fn take_diagnostics(&mut self) -> Result<plughost_core::DiagnosticBatch, Error> {
        match self
            .helper
            .request(Request::Diagnostics, self.timeouts.control)?
        {
            Response::Diagnostics(batch) => Ok(batch),
            _ => Err(Error::Protocol),
        }
    }

    /// Queries the loaded plugin without opening its editor or saving state. Results are a
    /// fresh snapshot, separate from scan/cache metadata. Re-query after configuration changes.
    pub fn capabilities(&mut self, slot: usize) -> Result<CapabilityReport, Error> {
        match self
            .helper
            .request(Request::Capabilities { slot }, self.timeouts.control)?
        {
            Response::Capabilities(report) => Ok(report),
            _ => Err(Error::Protocol),
        }
    }

    /// Drains coalesced timing notifications. This is a pull channel independent of diagnostics;
    /// an idle consumer cannot backpressure processing or shutdown. A recovered chain has its own
    /// stream.
    pub fn take_changes(&mut self) -> Result<plughost_core::ChangeBatch, Error> {
        match self.helper.request(Request::Changes, self.timeouts.load)? {
            Response::Changes(batch) => Ok(batch),
            _ => Err(Error::Protocol),
        }
    }

    /// Explicitly reactivates the saved configuration once. A repeated plugin restart request
    /// remains visible and never causes an automatic retry loop. DSP history may be discarded.
    pub fn reprepare(&mut self) -> Result<(), Error> {
        let config = self.config.clone().ok_or(Error::NotPrepared)?;
        self.prepare_audio(&config).map(|_| ())
    }

    /// Latest reported latency of the whole chain in samples, cached between helper responses.
    pub fn latency(&self) -> u32 {
        self.latency
    }

    /// Latest reported tail of the whole chain, cached between helper responses.
    pub fn tail(&self) -> Tail {
        self.tail
    }

    /// Each parameter of the plugin in `slot`, with its current normalized value.
    pub fn parameters(&mut self, slot: usize) -> Result<Vec<(ParameterInfo, f64)>, Error> {
        match self
            .helper
            .request(Request::Parameters { slot }, self.timeouts.control)?
        {
            Response::Parameters(parameters) => Ok(parameters),
            _ => Err(Error::Protocol),
        }
    }

    /// Reads current native range, default availability and group identity without editing.
    pub fn parameter_details(
        &mut self,
        slot: usize,
        id: u64,
    ) -> Result<plughost_core::ParameterDetails, Error> {
        match self.helper.request(
            Request::ParameterDetails { slot, id },
            self.timeouts.control,
        )? {
            Response::ParameterDetails(details) => Ok(details),
            _ => Err(Error::Protocol),
        }
    }

    /// Native discrete labels/values in pages of up to 256, without changing the parameter.
    pub fn parameter_choices(
        &mut self,
        slot: usize,
        id: u64,
        start: u64,
        count: u32,
    ) -> Result<plughost_core::ParameterChoicePage, Error> {
        plughost_core::validate_choice_page(start, count).map_err(|error| Error::Input {
            slot: Some(slot),
            error,
        })?;
        match self.helper.request(
            Request::ParameterChoices {
                slot,
                id,
                start,
                count,
            },
            self.timeouts.control,
        )? {
            Response::ParameterChoices(page) => Ok(page),
            _ => Err(Error::Protocol),
        }
    }

    /// Changes a parameter the way an edit in the plugin's editor does.
    pub fn set_parameter(&mut self, slot: usize, id: u64, value: f64) -> Result<(), Error> {
        let request = Request::SetParameter { slot, id, value };
        expect_done(self.helper.request(request, self.timeouts.control)?)?;
        self.refresh_timing()
    }

    /// Uses the native formatter without changing the parameter.
    pub fn parameter_text(&mut self, slot: usize, id: u64, value: f64) -> Result<String, Error> {
        match self.helper.request(
            Request::ParameterText { slot, id, value },
            self.timeouts.control,
        )? {
            Response::ParameterText(text) => Ok(text),
            _ => Err(Error::Protocol),
        }
    }

    /// Parses and edits in one helper operation; rejected text does not queue an edit.
    pub fn set_parameter_text(&mut self, slot: usize, id: u64, text: &str) -> Result<(), Error> {
        plughost_core::validate_parameter_text(text).map_err(|error| Error::Input {
            slot: Some(slot),
            error,
        })?;
        expect_done(self.helper.request(
            Request::SetParameterText {
                slot,
                id,
                text: text.to_owned(),
            },
            self.timeouts.control,
        )?)?;
        self.refresh_timing()
    }

    pub fn parameter_from_text(&mut self, slot: usize, id: u64, text: &str) -> Result<f64, Error> {
        plughost_core::validate_parameter_text(text).map_err(|error| Error::Input {
            slot: Some(slot),
            error,
        })?;
        match self.helper.request(
            Request::ParameterFromText {
                slot,
                id,
                text: text.to_owned(),
            },
            self.timeouts.control,
        )? {
            Response::ParameterValue(value) => Ok(value),
            _ => Err(Error::Protocol),
        }
    }

    pub fn parameter_to_plain(&mut self, slot: usize, id: u64, value: f64) -> Result<f64, Error> {
        match self.helper.request(
            Request::ParameterToPlain { slot, id, value },
            self.timeouts.control,
        )? {
            Response::ParameterValue(value) => Ok(value),
            _ => Err(Error::Protocol),
        }
    }

    pub fn parameter_to_normalized(
        &mut self,
        slot: usize,
        id: u64,
        plain: f64,
    ) -> Result<f64, Error> {
        match self.helper.request(
            Request::ParameterToNormalized { slot, id, plain },
            self.timeouts.control,
        )? {
            Response::ParameterValue(value) => Ok(value),
            _ => Err(Error::Protocol),
        }
    }

    /// Clears every plugin's internal state (tails, delay lines), keeping its settings.
    pub fn reset(&mut self) -> Result<(), Error> {
        expect_done(self.helper.request(Request::Reset, self.timeouts.load)?)?;
        self.refresh_timing()
    }

    /// Opens the editor of the plugin in `slot` in a window of the helper. The chain keeps
    /// processing while it is open, and edits in it reach the processor.
    pub fn open_editor(&mut self, slot: usize) -> Result<(), Error> {
        expect_done(
            self.helper
                .request(Request::OpenEditor { slot }, self.timeouts.load)?,
        )
    }

    pub fn close_editor(&mut self, slot: usize) -> Result<(), Error> {
        expect_done(
            self.helper
                .request(Request::CloseEditor { slot }, self.timeouts.control)?,
        )
    }

    /// Whether the editor of `slot` is open; the user may have closed its window.
    pub fn editor_open(&mut self, slot: usize) -> Result<bool, Error> {
        match self
            .helper
            .request(Request::EditorOpen { slot }, self.timeouts.control)?
        {
            Response::EditorOpen(open) => Ok(open),
            _ => Err(Error::Protocol),
        }
    }

    fn refresh_timing(&mut self) -> Result<(), Error> {
        if !self.is_prepared() {
            return Ok(());
        }
        match self
            .helper
            .request(Request::Timing, self.timeouts.control)?
        {
            Response::Timing { latency, tail } => {
                self.latency = latency;
                self.tail = tail;
                Ok(())
            }
            _ => Err(Error::Protocol),
        }
    }

    fn is_prepared(&self) -> bool {
        self.config.is_some()
    }
}

/// Starts a helper and loads `plugins`, restoring `states` as they load.
fn start(
    helper: &Path,
    plugins: &[PluginRef],
    states: &[PluginState],
    host: &HostIdentity,
    timeouts: Timeouts,
) -> Result<Chain, Error> {
    let mut process = Helper::spawn(helper, Mode::Host)?;
    let infos = match process.load(host, plugins, states, timeouts.load)? {
        Response::Loaded(infos) => infos,
        _ => return Err(Error::Protocol),
    };
    Ok(Chain {
        identity: host.clone(),
        helper_path: helper.to_path_buf(),
        helper: process,
        plugins: plugins.to_vec(),
        infos,
        timeouts,
        config: None,
        shared: None,
        generation: 0,
        latency: 0,
        tail: Tail::Samples(0),
    })
}

fn expect_done(response: Response) -> Result<(), Error> {
    match response {
        Response::Done => Ok(()),
        _ => Err(Error::Protocol),
    }
}
