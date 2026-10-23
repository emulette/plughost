//! Host mode on the main thread: one chain of plugins, their lifecycle, state, and editors. Audio
//! goes through the processing thread (`process.rs`), whose state this thread changes with
//! commands. It keeps its own copy of the prepared configuration and alignment plan, to plan
//! changes without the processing thread.

mod audio;
mod realtime;
mod state;
mod timing;

use std::sync::Arc;

use plughost_core::internal::{DiagnosticBuffer, ParameterEventBuffer};
use plughost_core::ipc::shared::{Caller, transfer};
use plughost_core::ipc::{Request, Response};
use plughost_core::render::Tail;
use plughost_core::{AudioConfig, Capabilities, CapabilityReport, Support};
use plughost_core::{Failure, FailureKind, HostIdentity, InputError, RoutedChainConfig};
use plughost_core::{PluginRef, PluginState, StatePurpose};
use plughost_formats::{EditorRequest, Error, HostedPlugin};

use crate::calls::Calls;
use crate::editor::EditorWindow;
use crate::errors::WRONG_MODE;
use crate::process::audio_timing::{self, AudioTiming};
use crate::process::{AudioThread, Command, Pipeline, Processing};

pub struct Host {
    slots: Vec<Box<dyn HostedPlugin>>,
    parameter_events: Vec<ParameterEventBuffer>,
    references: Vec<PluginRef>,
    identity: HostIdentity,
    retired_diagnostics: plughost_core::DiagnosticBatch,
    diagnostics: DiagnosticBuffer,
    /// Sends progress while a request goes through the slots one at a time.
    responder: crate::Responder,
    editors: Vec<Option<EditorWindow>>,
    processing: Arc<Processing>,
    /// The prepared routing, each slot's part of it, and the alignment planned for it.
    audio_config: Option<RoutedChainConfig>,
    audio_slots: Vec<AudioConfig>,
    routes: Vec<audio::Routes>,
    timing: Option<AudioTiming>,
    timing_polls: Vec<timing::Poll>,
    transfer: transfer::Receiver,
    calls: Calls,
    /// The application installed a realtime ring for the prepared chain.
    realtime_installed: bool,
    /// The processing thread runs the realtime worker.
    realtime_running: bool,
}

impl Host {
    pub fn new(
        transfer: transfer::Receiver,
        responder: crate::Responder,
        processing: Arc<Processing>,
    ) -> Self {
        Self {
            slots: Vec::new(),
            parameter_events: Vec::new(),
            references: Vec::new(),
            identity: HostIdentity::default(),
            retired_diagnostics: Default::default(),
            diagnostics: responder.diagnostics.clone(),
            responder,
            editors: Vec::new(),
            processing,
            audio_config: None,
            audio_slots: Vec::new(),
            routes: Vec::new(),
            timing: None,
            timing_polls: Vec::new(),
            transfer,
            calls: Calls::default(),
            realtime_installed: false,
            realtime_running: false,
        }
    }

    /// Handles one request. Parameter events and metadata are collected before the response
    /// leaves, so the next block is validated against metadata this request may have changed.
    pub fn handle(&mut self, request: Request<'_>) -> Response {
        if let Some(refused) = self.refused_while_running(&request) {
            return refused;
        }
        // A running realtime worker would stop for the timing read; changes still reach the
        // application through `take_changes`.
        let refresh_timing = !self.realtime_running
            && matches!(
                &request,
                Request::SetParameter { .. } | Request::SetParameterText { .. } | Request::Reset
            );
        let response = self.dispatch(request);
        self.collect_parameter_events();
        if refresh_timing && matches!(response, Response::Done) {
            let timing = self.timing();
            // Timing delivers pending host edits. Collect the resulting native notifications
            // before replying, as the former separate Timing request did.
            self.collect_parameter_events();
            match timing {
                // The edit or reset took effect; a slot whose timing cannot be read does not
                // undo it. The application keeps its last timing.
                Response::Failed { slot, failure } => {
                    crate::record_failure(&self.diagnostics, slot, &failure);
                    Response::Done
                }
                timing => timing,
            }
        } else {
            response
        }
    }

    fn dispatch(&mut self, request: Request<'_>) -> Response {
        let slot = request.slot();
        if slot.is_some_and(|slot| slot >= self.slots.len()) {
            return Response::Rejected {
                slot,
                error: InputError::Slot,
            };
        }
        let calls = self.calls.clone();
        let _call = slot.map(|slot| calls.enter(Caller::Main, slot));
        match request {
            Request::AudioBuses { slot } => self.with_slot(slot, |plugin| {
                plugin.audio_buses().map(Response::AudioBuses)
            }),
            Request::EventPorts { slot } => self.with_slot(slot, |plugin| {
                plugin.event_ports().map(Response::EventPorts)
            }),
            Request::AudioConfigurations { slot } => self.with_slot(slot, |plugin| {
                plugin
                    .audio_configurations()
                    .map(Response::AudioConfigurations)
            }),
            Request::Bypass { slot } => {
                self.with_slot(slot, |plugin| plugin.bypass().map(Response::Bypass))
            }
            Request::SetBypass { slot, enabled } => self.with_slot(slot, |plugin| {
                plugin.set_bypass(enabled)?;
                plugin.bypass().map(Response::Bypass)
            }),
            Request::PrepareAudio {
                config,
                memory,
                handle,
            } => self.prepare_audio(config, memory, handle),
            Request::ParameterEvents { slot } => {
                self.collect_parameter_events();
                Response::ParameterEvents(self.parameter_events[slot].take())
            }
            Request::FactoryPresets { slot } => self.with_slot(slot, |plugin| {
                plugin.factory_presets().map(Response::FactoryPresets)
            }),
            Request::SelectFactoryPreset { slot, preset } => {
                self.select_factory_preset(slot, &preset)
            }
            Request::LoadDiscoveredPreset {
                slot,
                location,
                load_key,
            } => self.load_discovered_preset(slot, &location, load_key.as_deref()),
            Request::Changes => {
                self.tick();
                match self.processing.request(Command::Changes(None)) {
                    Command::Changes(Some(changes)) => Response::Changes(changes),
                    _ => unreachable!(),
                }
            }
            Request::Diagnostics => {
                let mut batch = std::mem::take(&mut self.retired_diagnostics);
                batch.append(self.diagnostics.take());
                for (slot, plugin) in self.slots.iter().enumerate() {
                    let mut messages = plugin.take_diagnostics();
                    for message in &mut messages.records {
                        message.slot = Some(slot);
                    }
                    batch.append(messages);
                }
                Response::Diagnostics(batch)
            }
            Request::Load {
                host,
                plugins,
                activity,
            } => self.load(&plugins, &host, activity),
            Request::LoadState { slot, state } => self.load_state(slot, &state),
            Request::Capabilities { slot } => self.with_slot(slot, |plugin| {
                Ok(Response::Capabilities(CapabilityReport::new(
                    plugin.capabilities()?,
                    {
                        let mut capabilities = Capabilities::default();
                        capabilities.embedded_editor = Support::Supported;
                        capabilities.bus_discovery = Support::Supported;
                        capabilities.note_input = Support::Supported;
                        capabilities.note_output = Support::Supported;
                        capabilities.sample_accurate_automation = Support::Supported;
                        capabilities.state = Support::Supported;
                        capabilities.factory_presets =
                            (plugin.info().id.format() != plughost_core::PluginFormat::Clap).into();
                        capabilities.f32 = Support::Supported;
                        capabilities.f64 = Support::Supported;
                        capabilities
                    },
                )))
            }),
            Request::Parameters { slot } => Response::Parameters(self.slots[slot].parameters()),
            Request::ValidateAutomation { targets } => {
                // Refresh native metadata before using the processing thread's indexed copy.
                self.collect_parameter_events();
                match self
                    .processing
                    .request(Command::ValidateAutomation(targets, None))
                {
                    Command::ValidateAutomation(_, Some((slot, error))) => Response::Rejected {
                        slot: Some(slot),
                        error,
                    },
                    Command::ValidateAutomation(_, None) => Response::Done,
                    _ => unreachable!(),
                }
            }
            Request::SetParameter { slot, id, value } => self.with_slot(slot, |plugin| {
                plugin.set_parameter(id, value)?;
                Ok(Response::Done)
            }),
            Request::ParameterDetails { slot, id } => self.with_slot(slot, |plugin| {
                plugin.parameter_details(id).map(Response::ParameterDetails)
            }),
            Request::ParameterChoices {
                slot,
                id,
                start,
                count,
            } => self.with_slot(slot, |plugin| {
                plugin
                    .parameter_choices(id, start, count)
                    .map(Response::ParameterChoices)
            }),
            Request::ParameterText { slot, id, value } => self.with_slot(slot, |plugin| {
                plugin
                    .parameter_text(id, value)
                    .map(Response::ParameterText)
            }),
            Request::ParameterFromText { slot, id, text } => self.with_slot(slot, |plugin| {
                plugin
                    .parameter_from_text(id, &text)
                    .map(Response::ParameterValue)
            }),
            Request::ParameterToPlain { slot, id, value } => self.with_slot(slot, |plugin| {
                plugin
                    .parameter_to_plain(id, value)
                    .map(Response::ParameterValue)
            }),
            Request::ParameterToNormalized { slot, id, plain } => self.with_slot(slot, |plugin| {
                plugin
                    .parameter_to_normalized(id, plain)
                    .map(Response::ParameterValue)
            }),
            Request::SetParameterText { slot, id, text } => self.with_slot(slot, |plugin| {
                let value = plugin.parameter_from_text(id, &text)?;
                plugin.set_parameter(id, value)?;
                Ok(Response::Done)
            }),
            Request::SaveState { slot, purpose } => self.with_slot(slot, |plugin| {
                plugin.save_state(purpose).map(Response::State)
            }),
            Request::RestoreState {
                slot,
                state,
                purpose,
            } => self.restore_state(slot, &state, purpose),
            Request::InspectPreset { slot, bytes } => self.with_slot(slot, |plugin| {
                plughost_formats::preset::decode(plugin.info().id.format(), &bytes)
                    .map(|preset| Response::PresetInfo(preset.info))
            }),
            Request::ExportPreset { slot } => self.with_slot(slot, |plugin| {
                plughost_formats::preset::supported(plugin.info().id.format())?;
                let state = plugin.save_state(plughost_core::StatePurpose::Preset)?;
                plughost_formats::preset::encode(&state).map(Response::Preset)
            }),
            Request::ImportPreset { slot, bytes } => self.import_preset(slot, &bytes),
            Request::ReprepareSlot { slot } => self.reprepare_slot(slot),
            Request::Reset => self.reset(),
            Request::Timing => self.timing(),
            Request::OpenEditor { slot } => self.open_editor(slot),
            Request::CloseEditor { slot } => {
                if let Some(window) = self.editors[slot].take() {
                    window.close(self.slots[slot].as_mut());
                }
                Response::Done
            }
            Request::EditorOpen { slot } => {
                self.tick();
                Response::EditorOpen(self.editors[slot].is_some())
            }
            Request::RealtimeInstall { ring, tokens } => self.install_realtime(ring, &tokens),
            Request::RealtimeStart => self.start_realtime(),
            Request::RealtimeStop => self.stop_realtime(),
            Request::RealtimeResync => self.resync_realtime(),
            Request::PresetProviders { .. }
            | Request::DiscoverPresets { .. }
            | Request::Scan { .. }
            | Request::ListAudioUnits
            | Request::Process { .. }
            | Request::Shutdown => Response::Failed {
                slot: None,
                failure: Failure::new(FailureKind::Protocol, WRONG_MODE),
            },
        }
    }

    /// Runs between requests: serves the plugins' main-thread requests, collects their parameter
    /// events, applies their editor requests, and follows editor windows the user closed or
    /// resized. A failure to open a requested editor is reported as a diagnostic.
    pub fn tick(&mut self) {
        self.collect_parameter_events();
        self.observe_timing();
        let calls = self.calls.clone();
        for (slot, (plugin, editor)) in self.slots.iter_mut().zip(&mut self.editors).enumerate() {
            let _call = calls.enter(Caller::Main, slot);
            match (plugin.take_editor_request(), editor.as_mut()) {
                (Some(EditorRequest::Open), None) => match EditorWindow::open(plugin.as_mut()) {
                    Ok(window) => *editor = Some(window),
                    Err(failure) => crate::record_failure(&self.diagnostics, Some(slot), &failure),
                },
                (Some(EditorRequest::Open | EditorRequest::Show), Some(window)) => {
                    window.show(plugin.as_mut());
                }
                (Some(EditorRequest::Hide), Some(window)) => window.hide(plugin.as_mut()),
                (Some(EditorRequest::Closed), Some(_)) => {
                    if let Some(window) = editor.take() {
                        window.close(plugin.as_mut());
                    }
                }
                _ => {}
            }
            if let Some(window) = editor
                && !window.tick(plugin.as_mut())
            {
                *editor = None;
            }
        }
    }

    /// Serves the plugins' main-thread requests and drains their parameter events. A reported
    /// metadata change refreshes the processing thread's copy (see [`Pipeline::parameters`]).
    fn collect_parameter_events(&mut self) {
        let calls = self.calls.clone();
        for (slot, plugin) in self.slots.iter_mut().enumerate() {
            let _call = calls.enter(Caller::Main, slot);
            plugin.idle();
            let batch = plugin.take_parameter_events();
            if batch.resync_required
                || batch
                    .events
                    .contains(&plughost_core::ParameterEvent::MetadataChanged)
            {
                let parameters = parameter_infos(plugin.as_mut());
                self.processing
                    .request(Command::Parameters(slot, parameters));
            }
            self.parameter_events[slot].append(batch);
        }
    }

    fn invalidate_parameters(&self, slot: usize) {
        self.parameter_events[slot].take();
        self.parameter_events[slot].append(plughost_core::ParameterEventBatch::new(
            vec![
                plughost_core::ParameterEvent::MetadataChanged,
                plughost_core::ParameterEvent::ValuesChanged,
            ],
            true,
            0,
        ));
    }

    fn with_slot(
        &mut self,
        slot: usize,
        call: impl FnOnce(&mut dyn HostedPlugin) -> Result<Response, Error>,
    ) -> Response {
        call(self.slots[slot].as_mut()).unwrap_or_else(|error| rejected_or_failed(slot, &error))
    }

    fn open_editor(&mut self, slot: usize) -> Response {
        let (plugin, editor) = (&mut self.slots[slot], &mut self.editors[slot]);
        if let Some(window) = editor {
            window.show(plugin.as_mut());
            return Response::Done;
        }
        match EditorWindow::open(plugin.as_mut()) {
            Ok(window) => {
                *editor = Some(window);
                Response::Done
            }
            Err(failure) => Response::Failed {
                slot: Some(slot),
                failure,
            },
        }
    }

    /// Receives the activity mapping first: its transfer must be consumed even when the request
    /// is rejected. Invalid input leaves the current chain loaded.
    fn load(&mut self, plugins: &[PluginRef], host: &HostIdentity, activity: u64) -> Response {
        self.remove_realtime();
        // SAFETY: the application transferred its activity mapping once, for this request.
        match unsafe { self.transfer.receive_activity(activity) } {
            Ok(activity) => {
                self.calls = Calls::new(activity);
                self.processing.request(Command::Calls(self.calls.clone()));
            }
            Err(error) => {
                return Response::Failed {
                    slot: None,
                    failure: Failure::new(FailureKind::Protocol, error.to_string()),
                };
            }
        }
        if let Err(error) = host.validate() {
            return Response::Rejected { slot: None, error };
        }
        if plugins.is_empty() {
            return Response::Rejected {
                slot: None,
                error: InputError::EmptyChain,
            };
        }
        self.close_editors();
        // Retire the processors before the instances they process are dropped.
        drop(self.processing.request(Command::Load(Box::new(Pipeline {
            calls: self.calls.clone(),
            ..Pipeline::default()
        }))));
        self.audio_config = None;
        self.audio_slots.clear();
        self.routes.clear();
        self.timing = None;
        self.timing_polls.clear();
        self.parameter_events.clear();
        self.slots.clear();
        self.references.clear();
        self.retired_diagnostics = Default::default();
        let calls = self.calls.clone();
        let mut infos = Vec::new();
        for (slot, plugin) in plugins.iter().enumerate() {
            let _call = calls.enter(Caller::Main, slot);
            let loaded = match self.load_slot(slot, plugin, host) {
                Ok(loaded) => loaded,
                Err(error) => {
                    self.slots.clear();
                    return failed(slot, &error);
                }
            };
            infos.push(loaded.info().clone());
            self.slots.push(loaded);
            self.progress();
        }
        self.parameter_events = self.slots.iter().map(|_| Default::default()).collect();
        self.references = plugins.to_vec();
        self.identity = host.clone();
        self.editors = self.slots.iter().map(|_| None).collect();
        self.processing.request(Command::Processors(
            self.slots.iter().map(|plugin| plugin.processor()).collect(),
        ));
        Response::Loaded(infos)
    }

    /// Tells the application one slot of the request is done, renewing its deadline for the next.
    /// A failed write means the application disconnected, which ends the helper anyway.
    fn progress(&self) {
        let _ = self.responder.send(&Response::Progress);
    }

    /// Loads an instance for `slot`, whose audio-thread calls run on the processing thread.
    fn load_slot(
        &self,
        slot: usize,
        plugin: &PluginRef,
        host: &HostIdentity,
    ) -> Result<Box<dyn HostedPlugin>, Error> {
        // SAFETY: plugin code runs only in this helper process, so whatever it does stays in it.
        let mut loaded = unsafe { plughost_formats::load(plugin, host) }?;
        loaded.set_audio_thread(AudioThread::new(&self.processing, slot));
        Ok(loaded)
    }

    /// Restores a project state into a slot's newly loaded, unprepared plugin.
    fn load_state(&mut self, slot: usize, state: &PluginState) -> Response {
        let plugin = &mut self.slots[slot];
        if !state::same_class(plugin.info(), state) {
            return state::mismatch(slot);
        }
        match plugin.restore_state(state, StatePurpose::Project) {
            Ok(()) => Response::Done,
            Err(error) => failed(slot, &error),
        }
    }

    /// Resets every slot with the plugin's native reset; open editors stay open.
    fn reset(&mut self) -> Response {
        let calls = self.calls.clone();
        for slot in 0..self.slots.len() {
            let _call = calls.enter(Caller::Main, slot);
            let plugin = &mut self.slots[slot];
            if let Err(error) = plugin.reset() {
                return self.failed_reset(slot, &error);
            }
            let parameters = parameter_infos(plugin.as_mut());
            plugin.take_parameter_events();
            self.processing
                .request(Command::Parameters(slot, parameters));
            self.invalidate_parameters(slot);
        }
        let Some(config) = &self.audio_config else {
            // Native reset ended every note the chain forwarded.
            self.processing.request(Command::Reset(None));
            return Response::Done;
        };
        // Native reset activates the plugins again, which applies a latency they changed.
        let timings = match audio_timing::chain_timings(&self.slots, &calls) {
            Ok(timings) => timings,
            Err((slot, error)) => return self.failed_reset(slot, &error),
        };
        // Native reset discarded DSP history, so the previous alignment is unusable.
        let planned = AudioTiming::new(config, &timings).and_then(|plan| {
            let alignment = audio_timing::Alignment::new(config, plan.clone())?;
            Ok((plan, alignment))
        });
        let (alignment, response) = match planned {
            Ok((plan, alignment)) => {
                self.timing = Some(plan);
                (Some(alignment), Response::Done)
            }
            Err(failure) => {
                self.timing = None;
                (
                    None,
                    Response::Failed {
                        slot: None,
                        failure,
                    },
                )
            }
        };
        drop(self.processing.request(Command::Reset(alignment)));
        response
    }

    /// Ends a reset that failed at `slot`. The slots before it were reset natively, so the
    /// alignment history no longer matches them: the chain processes nothing until it is
    /// prepared again, as when the reset timing cannot be planned.
    fn failed_reset(&mut self, slot: usize, error: &Error) -> Response {
        self.timing = None;
        drop(self.processing.request(Command::Reset(None)));
        failed(slot, error)
    }

    fn timing(&mut self) -> Response {
        let Some(config) = &self.audio_config else {
            return Response::Timing {
                latency: 0,
                tail: Tail::Samples(0),
                latency_changed: false,
            };
        };
        let timings = match audio_timing::chain_timings(&self.slots, &self.calls) {
            Ok(timings) => timings,
            Err((slot, error)) => return failed(slot, &error),
        };
        for (slot, &timing) in timings.iter().enumerate() {
            if !self.processing.observe(slot, timing) {
                self.timing_polls[slot].retry();
            }
        }
        // A slot that waits to be prepared again fails the blocks it would process, not this read.
        match AudioTiming::new(config, &timings) {
            Ok(plan) => Response::Timing {
                latency: plan.latency(),
                tail: plan.current_tail(&timings),
                latency_changed: timings.iter().any(|timing| timing.latency_changed),
            },
            Err(failure) => Response::Failed {
                slot: None,
                failure,
            },
        }
    }

    fn close_editors(&mut self) {
        let calls = self.calls.clone();
        for (slot, (plugin, editor)) in self.slots.iter_mut().zip(&mut self.editors).enumerate() {
            if let Some(window) = editor.take() {
                let _call = calls.enter(Caller::Main, slot);
                window.close(plugin.as_mut());
            }
        }
    }

    /// Reopens the editors that were open before their instances changed. A failure is reported
    /// as a diagnostic; it does not undo the change.
    fn reopen_editors(&mut self, reopen: &[bool]) {
        let calls = self.calls.clone();
        for (slot, &reopen) in reopen.iter().enumerate() {
            if reopen {
                let _call = calls.enter(Caller::Main, slot);
                match EditorWindow::open(self.slots[slot].as_mut()) {
                    Ok(window) => self.editors[slot] = Some(window),
                    Err(failure) => crate::record_failure(&self.diagnostics, Some(slot), &failure),
                }
            }
        }
    }
}

fn failed(slot: usize, error: &Error) -> Response {
    Response::Failed {
        slot: Some(slot),
        failure: error.failure(),
    }
}

/// A plugin error answering a request for `slot`: invalid input is rejected, the rest failed.
fn rejected_or_failed(slot: usize, error: &Error) -> Response {
    match error.input_error() {
        Some(error) => Response::Rejected {
            slot: Some(slot),
            error,
        },
        None => failed(slot, error),
    }
}

fn parameter_infos(plugin: &mut dyn HostedPlugin) -> crate::process::Parameters {
    crate::process::Parameters::new(
        plugin
            .parameters()
            .into_iter()
            .map(|(info, _)| info)
            .collect(),
    )
}
