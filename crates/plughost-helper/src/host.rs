//! Host mode on the main thread: one chain of plugins, their lifecycle, state, and editors. Audio
//! goes through the processing thread (`process.rs`).

mod audio;
mod state;

use std::sync::Arc;

use plughost_core::ipc::shared::{Caller, transfer};
use plughost_core::ipc::{Request, Response};
use plughost_core::render::Tail;
use plughost_core::{Capabilities, CapabilityReport, DiagnosticBuffer, Support};
use plughost_core::{Failure, FailureKind, HostIdentity, InputError};
use plughost_core::{PluginRef, PluginState, StatePurpose};
use plughost_formats::{EditorRequest, Error, HostedPlugin};

use crate::calls::Calls;
use crate::editor::EditorWindow;
use crate::errors::WRONG_MODE;
use crate::process::audio_timing::{self, AudioTiming};
use crate::process::{self, Pipeline};

pub struct Host {
    slots: Vec<Box<dyn HostedPlugin>>,
    parameter_events: Vec<plughost_core::ParameterEventBuffer>,
    references: Vec<PluginRef>,
    identity: HostIdentity,
    retired_diagnostics: plughost_core::DiagnosticBatch,
    diagnostics: DiagnosticBuffer,
    editors: Vec<Option<EditorWindow>>,
    pipeline: process::Shared,
    transfer: transfer::Receiver,
    calls: Calls,
}

impl Host {
    pub fn new(transfer: transfer::Receiver, diagnostics: DiagnosticBuffer) -> Self {
        Self {
            slots: Vec::new(),
            parameter_events: Vec::new(),
            references: Vec::new(),
            identity: HostIdentity::default(),
            retired_diagnostics: Default::default(),
            diagnostics,
            editors: Vec::new(),
            pipeline: Default::default(),
            transfer,
            calls: Calls::default(),
        }
    }

    pub fn pipeline(&self) -> process::Shared {
        Arc::clone(&self.pipeline)
    }

    /// Handles one request. Parameter events and metadata are collected before the response
    /// leaves, so the next block is validated against metadata this request may have changed.
    pub fn handle(&mut self, request: Request) -> Response {
        let response = self.dispatch(request);
        self.collect_parameter_events();
        response
    }

    fn dispatch(&mut self, request: Request) -> Response {
        let slot = match &request {
            Request::AudioBuses { slot }
            | Request::EventPorts { slot }
            | Request::AudioConfigurations { slot }
            | Request::Bypass { slot }
            | Request::SetBypass { slot, .. }
            | Request::ParameterEvents { slot }
            | Request::FactoryPresets { slot }
            | Request::SelectFactoryPreset { slot, .. }
            | Request::LoadDiscoveredPreset { slot, .. }
            | Request::Parameters { slot }
            | Request::Capabilities { slot }
            | Request::SetParameter { slot, .. }
            | Request::ParameterDetails { slot, .. }
            | Request::ParameterChoices { slot, .. }
            | Request::ParameterText { slot, .. }
            | Request::ParameterFromText { slot, .. }
            | Request::ParameterToPlain { slot, .. }
            | Request::ParameterToNormalized { slot, .. }
            | Request::SetParameterText { slot, .. }
            | Request::InspectPreset { slot, .. }
            | Request::ExportPreset { slot }
            | Request::ImportPreset { slot, .. }
            | Request::SaveState { slot, .. }
            | Request::RestoreState { slot, .. }
            | Request::LoadState { slot, .. }
            | Request::OpenEditor { slot }
            | Request::CloseEditor { slot }
            | Request::EditorOpen { slot } => Some(*slot),
            _ => None,
        };
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
                let mut pipeline = process::lock(&self.pipeline);
                for slot in 0..pipeline.processors.len() {
                    let processor = pipeline.processors[slot].as_ref();
                    let timing = {
                        let _call = calls.enter(Caller::Main, slot);
                        audio_timing::plugin_timing(processor)
                    };
                    match timing {
                        Ok(timing) => pipeline.changes.observe(slot, timing),
                        Err(error) if processor.restart_required() => {
                            return failed(slot, &error);
                        }
                        Err(_) => {}
                    }
                }
                Response::Changes(pipeline.changes.take())
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
                Ok(Response::Capabilities(CapabilityReport {
                    plugin: plugin.capabilities()?,
                    host: Capabilities {
                        embedded_editor: Support::Supported,
                        bus_discovery: Support::Supported,
                        note_input: Support::Supported,
                        note_output: Support::Supported,
                        sample_accurate_automation: Support::Supported,
                        state: Support::Supported,
                        factory_presets: (plugin.info().format
                            != plughost_core::PluginFormat::Clap)
                            .into(),
                        f32: Support::Supported,
                        f64: Support::Supported,
                    },
                }))
            }),
            Request::Parameters { slot } => Response::Parameters(self.slots[slot].parameters()),
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
                plughost_formats::preset::decode(plugin.info().format, &bytes)
                    .map(|preset| Response::PresetInfo(preset.info))
            }),
            Request::ExportPreset { slot } => self.with_slot(slot, |plugin| {
                if !plughost_formats::preset::supports(plugin.info().format) {
                    return Err(Error::PresetUnsupported);
                }
                let state = plugin.save_state(plughost_core::StatePurpose::Preset)?;
                plughost_formats::preset::encode(&state).map(Response::Preset)
            }),
            Request::ImportPreset { slot, bytes } => self.import_preset(slot, &bytes),
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
                if let Some(cached) = process::lock(&self.pipeline).parameters.get_mut(slot) {
                    *cached = parameters;
                }
            }
            self.parameter_events[slot].append(batch);
        }
    }

    fn invalidate_parameters(&self, slot: usize) {
        self.parameter_events[slot].take();
        self.parameter_events[slot].append(plughost_core::ParameterEventBatch {
            events: vec![
                plughost_core::ParameterEvent::MetadataChanged,
                plughost_core::ParameterEvent::ValuesChanged,
            ],
            resync_required: true,
            dropped: 0,
        });
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
        // SAFETY: the application transferred its activity mapping once, for this request.
        match unsafe { self.transfer.receive_activity(activity) } {
            Ok(activity) => {
                self.calls = Calls::new(activity);
                process::lock(&self.pipeline).calls = self.calls.clone();
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
        *process::lock(&self.pipeline) = Pipeline {
            calls: self.calls.clone(),
            ..Pipeline::default()
        };
        self.parameter_events.clear();
        self.slots.clear();
        self.references.clear();
        self.retired_diagnostics = Default::default();
        let calls = self.calls.clone();
        let mut infos = Vec::new();
        for (slot, plugin) in plugins.iter().enumerate() {
            let _call = calls.enter(Caller::Main, slot);
            let loaded = match plughost_formats::load(plugin, host) {
                Ok(loaded) => loaded,
                Err(error) => {
                    self.slots.clear();
                    return failed(slot, &error);
                }
            };
            infos.push(loaded.info().clone());
            self.slots.push(loaded);
        }
        self.parameter_events = self.slots.iter().map(|_| Default::default()).collect();
        self.references = plugins.to_vec();
        self.identity = host.clone();
        self.editors = self.slots.iter().map(|_| None).collect();
        process::lock(&self.pipeline).processors =
            self.slots.iter().map(|plugin| plugin.processor()).collect();
        Response::Loaded(infos)
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
                return failed(slot, &error);
            }
            let parameters = parameter_infos(plugin.as_mut());
            plugin.take_parameter_events();
            if let Some(cached) = process::lock(&self.pipeline).parameters.get_mut(slot) {
                *cached = parameters;
            }
            self.invalidate_parameters(slot);
        }
        {
            let mut pipeline = process::lock(&self.pipeline);
            // Native reset ended every note the chain forwarded.
            pipeline.resync = false;
            if let Some(config) = pipeline.audio_config.clone() {
                let timings =
                    match audio_timing::chain_timings(&pipeline.processors, &calls, Caller::Main) {
                        Ok(timings) => timings,
                        Err((slot, error)) => return failed(slot, &error),
                    };
                // Native reset discarded DSP history, so the previous alignment is unusable.
                pipeline.alignment = None;
                match AudioTiming::new(&config, &timings)
                    .and_then(|plan| audio_timing::Alignment::new(&config, plan))
                {
                    Ok(alignment) => pipeline.alignment = Some(alignment),
                    Err(failure) => {
                        return Response::Failed {
                            slot: None,
                            failure,
                        };
                    }
                }
            }
        }
        Response::Done
    }

    fn timing(&self) -> Response {
        let pipeline = process::lock(&self.pipeline);
        let Some(config) = &pipeline.audio_config else {
            return Response::Timing {
                latency: 0,
                tail: Tail::Samples(0),
            };
        };
        let timings =
            match audio_timing::chain_timings(&pipeline.processors, &self.calls, Caller::Main) {
                Ok(timings) => timings,
                Err((slot, error)) => return failed(slot, &error),
            };
        match AudioTiming::plan(config, &timings) {
            Ok((plan, tail)) => Response::Timing {
                latency: plan.latency(),
                tail,
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

fn parameter_infos(plugin: &mut dyn HostedPlugin) -> Vec<plughost_core::ParameterInfo> {
    plugin
        .parameters()
        .into_iter()
        .map(|(info, _)| info)
        .collect()
}
