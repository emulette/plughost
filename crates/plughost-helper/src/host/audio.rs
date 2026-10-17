//! Bus preparation. Preparing an already prepared chain stages every slot on a fresh instance
//! restored from the active one and commits only when every slot succeeds, so a failure leaves
//! the prepared chain and its pending audio intact. Initial setup has no old DSP to retain. A
//! slot whose plugin cannot save its state is prepared again in place instead; once one is, a
//! failure leaves the chain unprepared.
use super::*;
use crate::process;
use plughost_core::AudioBusInfo;
use plughost_core::AudioDirection;
use plughost_core::ipc::shared::Descriptor;
use plughost_formats::BlockProcessor;

impl Host {
    pub(super) fn prepare_audio(
        &mut self,
        config: RoutedChainConfig,
        memory: Descriptor,
        handle: u64,
    ) -> Response {
        let transport = match process::shared::Transport::receive(&self.transfer, memory, handle) {
            Ok(transport) => transport,
            Err(error) => {
                return Response::Failed {
                    slot: None,
                    failure: Failure::new(FailureKind::Configuration, error.to_string()),
                };
            }
        };
        if let Err(error) = config.validate(self.slots.len()) {
            return Response::Rejected { slot: None, error };
        }
        self.configure(config, transport)
    }

    fn configure(
        &mut self,
        config: RoutedChainConfig,
        transport: process::shared::Transport,
    ) -> Response {
        let staged = self.audio_config.is_some();
        // `None` prepares the active instance in place: every slot of an unprepared chain, and
        // a slot of a prepared chain whose plugin cannot save the state a fresh instance needs.
        let mut candidates: Vec<Option<Box<dyn HostedPlugin>>> = Vec::new();
        let mut reports = Vec::new();
        if staged {
            if let Some(response) = self.stage(&config, &mut candidates, &mut reports) {
                self.discard(candidates);
                return response;
            }
        } else {
            candidates.resize_with(self.slots.len(), || None);
            reports.resize(self.slots.len(), None);
        }
        // Fresh instances are all staged before any active one is changed. From then on a
        // failure cannot leave the prepared chain as it was, so it leaves the chain unprepared.
        let in_place = staged && candidates.iter().any(Option::is_none);
        let response = self.prepare_slots(config, transport, &mut candidates, reports);
        if !matches!(response, Response::AudioPrepared { .. }) {
            self.discard(candidates);
            if in_place {
                self.unprepare();
            }
        }
        response
    }

    /// Stages a fresh instance of each slot, restored from and prepared like the active one, and
    /// returns the failure that stopped it.
    fn stage(
        &mut self,
        config: &RoutedChainConfig,
        candidates: &mut Vec<Option<Box<dyn HostedPlugin>>>,
        reports: &mut Vec<Option<Vec<AudioBusInfo>>>,
    ) -> Option<Response> {
        let calls = self.calls.clone();
        for slot in 0..self.slots.len() {
            let _call = calls.enter(Caller::Main, slot);
            let bypass = match self.slots[slot].bypass() {
                Ok(bypass) => bypass.enabled,
                Err(error) => return Some(failed(slot, &error)),
            };
            let state = match self.slots[slot].save_state(plughost_core::StatePurpose::Project) {
                Ok(state) => state,
                // A CLAP restart likewise deactivates and activates the same instance.
                Err(error) if error.failure().kind == FailureKind::Unsupported => {
                    candidates.push(None);
                    reports.push(None);
                    continue;
                }
                Err(error) => return Some(failed(slot, &error)),
            };
            let mut candidate = match self.load_slot(slot, &self.references[slot], &self.identity) {
                Ok(candidate) => candidate,
                Err(error) => return Some(failed(slot, &error)),
            };
            let result = candidate
                .restore_state(&state, plughost_core::StatePurpose::Project)
                .and_then(|()| {
                    let report = prepare(config, slot, candidate.as_mut())?;
                    if let Some(enabled) = bypass {
                        candidate.set_bypass(enabled)?;
                    }
                    Ok(report)
                });
            match result {
                Ok(report) => {
                    reports.push(Some(report));
                    candidates.push(Some(candidate));
                }
                Err(error) => {
                    self.retain_diagnostics(slot, candidate.as_ref());
                    return Some(failed(slot, &error));
                }
            }
        }
        None
    }

    /// Drops staged instances that were not committed, keeping what they logged and naming
    /// their slot should one fail on the way out.
    fn discard(&mut self, candidates: Vec<Option<Box<dyn HostedPlugin>>>) {
        let calls = self.calls.clone();
        for (slot, candidate) in candidates.into_iter().enumerate() {
            if let Some(candidate) = candidate {
                let _call = calls.enter(Caller::Main, slot);
                self.retain_diagnostics(slot, candidate.as_ref());
                drop(candidate);
            }
        }
    }

    fn prepare_slots(
        &mut self,
        mut config: RoutedChainConfig,
        transport: process::shared::Transport,
        candidates: &mut Vec<Option<Box<dyn HostedPlugin>>>,
        mut reports: Vec<Option<Vec<AudioBusInfo>>>,
    ) -> Response {
        let generation = transport.descriptor().generation;
        let staged = self.audio_config.is_some();
        let calls = self.calls.clone();
        let mut next: Vec<&mut (dyn HostedPlugin + 'static)> = candidates
            .iter_mut()
            .zip(&mut self.slots)
            .map(|(candidate, active)| match candidate {
                Some(candidate) => candidate.as_mut(),
                None => active.as_mut(),
            })
            .collect();
        for (slot, report) in reports.iter_mut().enumerate() {
            if report.is_none() {
                let _call = calls.enter(Caller::Main, slot);
                match prepare(&config, slot, &mut *next[slot]) {
                    Ok(prepared) => *report = Some(prepared),
                    Err(error) => return failed(slot, &error),
                }
            }
        }
        // Every slot has its report now.
        let reports: Vec<Vec<AudioBusInfo>> = reports.into_iter().flatten().collect();
        for (slot, report) in reports.iter().enumerate() {
            // Flattening uses native indices, not caller vector order or numeric bus IDs.
            let request = &mut config.slots[slot];
            for direction in [AudioDirection::Input, AudioDirection::Output] {
                let requested: Vec<_> = match direction {
                    AudioDirection::Input => request.inputs.iter().map(|r| r.bus).collect(),
                    AudioDirection::Output => request.outputs.clone(),
                };
                let negotiated = requested.iter().filter(|bus| bus.active).all(|bus| {
                    report.iter().any(|info| {
                        info.direction == direction
                            && info.id == bus.id
                            && info.active == Some(true)
                            && info.layout == Some(bus.layout)
                            && info.channels as usize == bus.layout.channels()
                    })
                }) && !report.iter().any(|info| {
                    info.direction == direction
                        && info.active == Some(true)
                        && !requested.iter().any(|bus| bus.active && bus.id == info.id)
                });
                if !negotiated {
                    return Response::Failed {
                        slot: Some(slot),
                        failure: Failure::new(
                            FailureKind::Configuration,
                            crate::errors::AUDIO_NEGOTIATION,
                        ),
                    };
                }
            }
            request.inputs.sort_by_key(|route| {
                report
                    .iter()
                    .find(|info| info.direction == AudioDirection::Input && info.id == route.bus.id)
                    .map(|info| info.index)
            });
            request.outputs.sort_by_key(|bus| {
                report
                    .iter()
                    .find(|info| info.direction == AudioDirection::Output && info.id == bus.id)
                    .map(|info| info.index)
            });
        }
        let processors: Option<Vec<Box<dyn BlockProcessor>>> =
            staged.then(|| next.iter().map(|plugin| plugin.processor()).collect());
        let timings = match audio_timing::chain_timings(&next, &calls) {
            Ok(timings) => timings,
            Err((slot, error)) => return failed(slot, &error),
        };
        let mut parameters = Vec::new();
        let mut event_ports = Vec::new();
        for (slot, plugin) in next.iter_mut().enumerate() {
            let _call = calls.enter(Caller::Main, slot);
            parameters.push(parameter_infos(&mut **plugin));
            plugin.take_parameter_events();
            match plugin.event_ports() {
                Ok(ports) => event_ports.push(ports),
                Err(error) => return failed(slot, &error),
            }
        }
        let events = match process::events::EventPlan::new(&config, &event_ports) {
            Ok(plan) => {
                let buffers = plan.buffers();
                (plan, buffers)
            }
            Err(message) => {
                return Response::Failed {
                    slot: None,
                    failure: Failure::new(FailureKind::Configuration, message),
                };
            }
        };
        let (plan, tail) = match AudioTiming::plan(&config, &timings) {
            Ok(plan) => plan,
            Err((slot, failure)) => return Response::Failed { slot, failure },
        };
        let latency = plan.latency();
        let storage = process::prepared::Prepared::new(&config, &timings);
        let alignment = match audio_timing::Alignment::new(&config, plan.clone()) {
            Ok(alignment) => alignment,
            Err(failure) => {
                return Response::Failed {
                    slot: None,
                    failure,
                };
            }
        };
        let reopen: Vec<bool> = self.editors.iter().map(Option::is_some).collect();
        if staged {
            self.close_editors();
        }
        let mut old = Vec::new();
        for (slot, candidate) in std::mem::take(candidates).into_iter().enumerate() {
            if let Some(candidate) = candidate {
                let plugin = std::mem::replace(&mut self.slots[slot], candidate);
                self.retain_diagnostics(slot, plugin.as_ref());
                old.push((slot, plugin));
            }
        }
        self.audio_slots = config
            .slots
            .iter()
            .map(|slot| AudioConfig {
                sample_rate: config.sample_rate,
                max_block_size: config.max_block_size,
                sample_format: config.sample_format,
                mode: config.mode,
                configuration: slot.configuration,
                inputs: slot.inputs.iter().map(|route| route.bus).collect(),
                outputs: slot.outputs.clone(),
                events: slot.events.native(),
            })
            .collect();
        // An installed ring has the previous configuration's shape.
        self.remove_realtime();
        self.audio_config = Some(config.clone());
        self.timing = Some(plan);
        self.timing_polls = vec![super::timing::Poll::default(); self.slots.len()];
        // Retire the old processor handles, and the rest of the old state, before the old
        // instances are dropped.
        drop(
            self.processing
                .request(Command::Prepare(Box::new(process::Prepare {
                    processors,
                    prepared: Some(storage),
                    transport: Some(transport),
                    audio_config: Some(config),
                    alignment: Some(alignment),
                    events: Some(events),
                    parameters,
                    timings,
                }))),
        );
        for (slot, plugin) in old {
            let _call = calls.enter(Caller::Main, slot);
            drop(plugin);
        }
        for slot in 0..self.slots.len() {
            self.invalidate_parameters(slot);
        }
        if staged {
            self.reopen_editors(&reopen);
        }
        Response::AudioPrepared {
            generation,
            buses: reports,
            latency,
            tail,
        }
    }
}

impl Host {
    /// Forgets the prepared chain after its active instances were changed, so blocks fail as
    /// unprepared until the chain is prepared again.
    fn unprepare(&mut self) {
        self.remove_realtime();
        self.audio_config = None;
        self.audio_slots.clear();
        self.timing = None;
        self.timing_polls.clear();
        drop(
            self.processing
                .request(Command::Prepare(Box::new(process::Prepare {
                    processors: None,
                    prepared: None,
                    transport: None,
                    audio_config: None,
                    alignment: None,
                    events: None,
                    parameters: Vec::new(),
                    timings: Vec::new(),
                }))),
        );
    }
}

fn prepare(
    config: &RoutedChainConfig,
    slot: usize,
    plugin: &mut dyn HostedPlugin,
) -> Result<Vec<AudioBusInfo>, Error> {
    plugin.prepare_audio(&config.slot_config(slot).map_err(Error::Input)?)
}
