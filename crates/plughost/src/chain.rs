//! A chain of plugins processed in order in one helper process.

mod audio;
mod realtime;
mod recovery;
mod shared;
mod state;

use std::path::{Path, PathBuf};
use std::time::Duration;

use plughost_core::CapabilityReport;
use plughost_core::PluginRef;
use plughost_core::ipc::{Request, Response};
use plughost_core::render::Tail;
use plughost_core::{Event, HostIdentity, ParameterInfo, PluginInfo, PluginState, events_fit};

use crate::errors::Error;
use crate::helper::{Helper, Mode};

pub use recovery::ChainSnapshot;

/// How long each kind of operation may go without a response from the helper. Start from
/// [`Timeouts::default`] and change the fields that need other values. A duration too long to
/// end at a representable instant, such as [`Duration::MAX`], waits without a deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Timeouts {
    /// Loading plugins, restoring state, importing/loading presets, selecting factory presets,
    /// opening editors, preparing a slot again and preparing an already configured chain again
    /// (which reloads its plugins). Large instruments take minutes.
    pub load: Duration,
    /// Initial preparation, resetting, saving state, parameter access and other control calls.
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
/// decides whether to [`Chain::recover`], or [`Chain::recover_realtime`] for a chain with a
/// realtime endpoint.
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
    realtime: Option<realtime::Realtime>,
}

impl std::fmt::Debug for Chain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chain")
            .field("helper", &self.helper.monitor())
            .field("plugins", &self.infos)
            .field("config", &self.config)
            .field("prepared", &self.shared.is_some())
            .field("latency", &self.latency)
            .field("tail", &self.tail)
            .finish_non_exhaustive()
    }
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
    /// Native bypass returns only when a state holds it, as a VST3 or CLAP bypass parameter does;
    /// Audio Unit bypass never does. [`Chain::snapshot`] saves the states and the bypass together;
    /// apply the bypass with [`Chain::set_bypass`] before switching to the recovered chain, or let
    /// [`Chain::recover_realtime`] do it. The realtime endpoint stays with this chain: it is not
    /// moved to the recovered one.
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

    /// Loaded plugin metadata in zero-based chain slot order.
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
        match self.request(Request::Diagnostics, self.timeouts.control)? {
            Response::Diagnostics(batch) => Ok(batch),
            _ => Err(self.protocol_error()),
        }
    }

    /// Queries the loaded plugin without opening its editor or saving state. Results are a
    /// fresh snapshot, separate from scan/cache metadata. Re-query after configuration changes.
    pub fn capabilities(&mut self, slot: usize) -> Result<CapabilityReport, Error> {
        match self.request(Request::Capabilities { slot }, self.timeouts.control)? {
            Response::Capabilities(report) => Ok(report),
            _ => Err(self.protocol_error()),
        }
    }

    /// Drains coalesced timing notifications. This is a pull channel independent of diagnostics;
    /// an idle consumer cannot backpressure processing or shutdown. A recovered chain has its own
    /// stream.
    pub fn take_changes(&mut self) -> Result<plughost_core::ChangeBatch, Error> {
        match self.request(Request::Changes, self.timeouts.control)? {
            Response::Changes(batch) => Ok(batch),
            _ => Err(self.protocol_error()),
        }
    }

    /// Explicitly reactivates the saved configuration once. A repeated plugin restart request
    /// remains visible and never causes an automatic retry loop. DSP history may be discarded.
    /// [`Chain::reprepare_slot`] prepares one slot again, also while the realtime endpoint runs.
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
        match self.request(Request::Parameters { slot }, self.timeouts.control)? {
            Response::Parameters(parameters) => Ok(parameters),
            _ => Err(self.protocol_error()),
        }
    }

    /// Reads current native range, default availability and group identity without editing.
    pub fn parameter_details(
        &mut self,
        slot: usize,
        id: u64,
    ) -> Result<plughost_core::ParameterDetails, Error> {
        match self.request(
            Request::ParameterDetails { slot, id },
            self.timeouts.control,
        )? {
            Response::ParameterDetails(details) => Ok(details),
            _ => Err(self.protocol_error()),
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
        match self.request(
            Request::ParameterChoices {
                slot,
                id,
                start,
                count,
            },
            self.timeouts.control,
        )? {
            Response::ParameterChoices(page) => Ok(page),
            _ => Err(self.protocol_error()),
        }
    }

    /// Changes a parameter the way an edit in the plugin's editor does.
    pub fn set_parameter(&mut self, slot: usize, id: u64, value: f64) -> Result<(), Error> {
        self.request_edit(Request::SetParameter { slot, id, value })
    }

    /// Uses the native formatter without changing the parameter.
    pub fn parameter_text(&mut self, slot: usize, id: u64, value: f64) -> Result<String, Error> {
        match self.request(
            Request::ParameterText { slot, id, value },
            self.timeouts.control,
        )? {
            Response::ParameterText(text) => Ok(text),
            _ => Err(self.protocol_error()),
        }
    }

    /// Parses and edits in one helper operation; rejected text does not queue an edit.
    pub fn set_parameter_text(&mut self, slot: usize, id: u64, text: &str) -> Result<(), Error> {
        plughost_core::validate_parameter_text(text).map_err(|error| Error::Input {
            slot: Some(slot),
            error,
        })?;
        self.request_edit(Request::SetParameterText {
            slot,
            id,
            text: text.to_owned(),
        })
    }

    /// Parses native display text into a normalized value without editing the parameter.
    pub fn parameter_from_text(&mut self, slot: usize, id: u64, text: &str) -> Result<f64, Error> {
        plughost_core::validate_parameter_text(text).map_err(|error| Error::Input {
            slot: Some(slot),
            error,
        })?;
        match self.request(
            Request::ParameterFromText {
                slot,
                id,
                text: text.to_owned(),
            },
            self.timeouts.control,
        )? {
            Response::ParameterValue(value) => Ok(value),
            _ => Err(self.protocol_error()),
        }
    }

    /// Converts a normalized value to the plugin's native parameter units.
    pub fn parameter_to_plain(&mut self, slot: usize, id: u64, value: f64) -> Result<f64, Error> {
        match self.request(
            Request::ParameterToPlain { slot, id, value },
            self.timeouts.control,
        )? {
            Response::ParameterValue(value) => Ok(value),
            _ => Err(self.protocol_error()),
        }
    }

    /// Converts native parameter units to a normalized value without editing it.
    pub fn parameter_to_normalized(
        &mut self,
        slot: usize,
        id: u64,
        plain: f64,
    ) -> Result<f64, Error> {
        match self.request(
            Request::ParameterToNormalized { slot, id, plain },
            self.timeouts.control,
        )? {
            Response::ParameterValue(value) => Ok(value),
            _ => Err(self.protocol_error()),
        }
    }

    /// Clears every plugin's internal state (tails, delay lines), keeping its settings. Refused
    /// while the realtime endpoint runs; [`Chain::resync_realtime`] resets the plugins under it.
    pub fn reset(&mut self) -> Result<(), Error> {
        self.refuse_while_realtime()?;
        self.request_timing(Request::Reset).map(|_| ())
    }

    /// Opens the editor of the plugin in `slot` in a window of the helper. The chain keeps
    /// processing while it is open, and edits in it reach the processor.
    pub fn open_editor(&mut self, slot: usize) -> Result<(), Error> {
        self.request_done(Request::OpenEditor { slot }, self.timeouts.load)
    }

    /// Closes the helper-owned editor window for this slot, if open.
    pub fn close_editor(&mut self, slot: usize) -> Result<(), Error> {
        self.request_done(Request::CloseEditor { slot }, self.timeouts.control)
    }

    /// Whether the editor of `slot` is open; the user may have closed its window.
    pub fn editor_open(&mut self, slot: usize) -> Result<bool, Error> {
        match self.request(Request::EditorOpen { slot }, self.timeouts.control)? {
            Response::EditorOpen(open) => Ok(open),
            _ => Err(self.protocol_error()),
        }
    }

    fn request(&mut self, request: Request<'_>, timeout: Duration) -> Result<Response, Error> {
        let result = self.helper.request(request, timeout);
        if matches!(
            result,
            Err(Error::Protocol | Error::Crashed { .. } | Error::TimedOut { .. })
        ) {
            self.shared = None;
        }
        result
    }

    fn request_done(&mut self, request: Request<'_>, timeout: Duration) -> Result<(), Error> {
        match self.request(request, timeout)? {
            Response::Done => Ok(()),
            _ => Err(self.protocol_error()),
        }
    }

    /// Reads the chain's latency and tail from the helper, and whether a plugin announced a
    /// latency that is not applied yet.
    fn read_timing(&mut self) -> Result<bool, Error> {
        if !self.is_prepared() {
            return Ok(false);
        }
        self.request_timing(Request::Timing)
    }

    /// Sends a parameter edit, after which the helper reads the timing again, except under a
    /// running realtime endpoint, whose timing changes arrive through [`Chain::take_changes`].
    fn request_edit(&mut self, request: Request<'_>) -> Result<(), Error> {
        if self.realtime_running() {
            return self.request_done(request, self.timeouts.control);
        }
        self.request_timing(request).map(|_| ())
    }

    fn request_timing(&mut self, request: Request<'_>) -> Result<bool, Error> {
        match self.request(request, self.timeouts.control)? {
            Response::Timing {
                latency,
                tail,
                latency_changed,
            } => {
                self.latency = latency;
                self.tail = tail;
                Ok(latency_changed)
            }
            _ => Err(self.protocol_error()),
        }
    }

    fn is_prepared(&self) -> bool {
        self.shared.is_some()
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
    for (slot, plugin) in plugins.iter().enumerate() {
        if plugin
            .bundle
            .as_ref()
            .is_some_and(|path| path.to_str().is_none())
        {
            return Err(Error::Input {
                slot: Some(slot),
                error: plughost_core::InputError::PathEncoding,
            });
        }
    }
    let mut process = Helper::spawn(helper, Mode::Host)?;
    let infos = match process.load(host, plugins, timeouts.load)? {
        Response::Loaded(infos) if infos.len() == plugins.len() => infos,
        _ => return Err(Error::Protocol),
    };
    for (slot, state) in states.iter().enumerate() {
        let request = Request::LoadState {
            slot,
            state: std::borrow::Cow::Borrowed(state),
        };
        expect_done(process.request(request, timeouts.load)?)?;
    }
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
        realtime: None,
    })
}

fn expect_done(response: Response) -> Result<(), Error> {
    match response {
        Response::Done => Ok(()),
        _ => Err(Error::Protocol),
    }
}
