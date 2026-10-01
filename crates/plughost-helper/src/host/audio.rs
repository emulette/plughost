//! Bus preparation. Preparing an already prepared chain stages every slot on a fresh instance
//! restored from the active one and commits only when every slot succeeds, so a failure leaves
//! the prepared chain and its pending audio intact. Initial setup has no old DSP to retain.
use super::*;
use plughost_core::ipc::shared::Descriptor;
use plughost_core::{AudioBusInfo, AudioConfig, AudioDirection, RoutedChainConfig};
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
        mut config: RoutedChainConfig,
        transport: process::shared::Transport,
    ) -> Response {
        let generation = transport.descriptor().generation;
        let staged = process::lock(&self.pipeline).audio_config.is_some();
        let calls = self.calls.clone();
        let mut candidates: Vec<Box<dyn HostedPlugin>> = Vec::new();
        let mut reports = Vec::new();
        if staged {
            for slot in 0..self.slots.len() {
                let _call = calls.enter(Caller::Main, slot);
                let bypass = match self.slots[slot].bypass() {
                    Ok(bypass) => bypass.enabled,
                    Err(error) => return failed(slot, &error),
                };
                let state = match self.slots[slot].save_state(plughost_core::StatePurpose::Project)
                {
                    Ok(state) => state,
                    Err(error) => return failed(slot, &error),
                };
                let mut candidate =
                    match plughost_formats::load(&self.references[slot], &self.identity) {
                        Ok(candidate) => candidate,
                        Err(error) => return failed(slot, &error),
                    };
                let result = candidate
                    .restore_state(&state, plughost_core::StatePurpose::Project)
                    .and_then(|()| {
                        let report = prepare(&config, slot, candidate.as_mut())?;
                        if let Some(enabled) = bypass {
                            candidate.set_bypass(enabled)?;
                        }
                        Ok(report)
                    });
                match result {
                    Ok(report) => {
                        reports.push(report);
                        candidates.push(candidate);
                    }
                    Err(error) => {
                        self.retain_diagnostics(slot, candidate.as_ref());
                        return failed(slot, &error);
                    }
                }
            }
        } else {
            for (slot, plugin) in self.slots.iter_mut().enumerate() {
                let _call = calls.enter(Caller::Main, slot);
                match prepare(&config, slot, plugin.as_mut()) {
                    Ok(report) => reports.push(report),
                    Err(error) => return failed(slot, &error),
                }
            }
        }
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
        let candidate_processors: Option<Vec<Box<dyn BlockProcessor>>> = staged.then(|| {
            candidates
                .iter()
                .map(|candidate| candidate.processor())
                .collect()
        });
        let prepared = if staged { &candidates } else { &self.slots };
        let timings = match audio_timing::chain_timings(prepared, &calls) {
            Ok(timings) => timings,
            Err((slot, error)) => return failed(slot, &error),
        };
        let prepared_slots = if staged {
            &mut candidates
        } else {
            &mut self.slots
        };
        let mut parameters = Vec::new();
        let mut event_ports = Vec::new();
        for (slot, plugin) in prepared_slots.iter_mut().enumerate() {
            let _call = calls.enter(Caller::Main, slot);
            parameters.push(parameter_infos(plugin.as_mut()));
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
            Err(failure) => {
                return Response::Failed {
                    slot: None,
                    failure,
                };
            }
        };
        let latency = plan.latency();
        let storage = process::prepared::Prepared::new(&config, &timings);
        let alignment = match audio_timing::Alignment::new(&config, plan) {
            Ok(alignment) => alignment,
            Err(failure) => {
                return Response::Failed {
                    slot: None,
                    failure,
                };
            }
        };
        let reopen: Vec<bool> = self.editors.iter().map(Option::is_some).collect();
        if let Some(processors) = candidate_processors {
            self.close_editors();
            let old = std::mem::replace(&mut self.slots, candidates);
            for (slot, plugin) in old.iter().enumerate() {
                self.retain_diagnostics(slot, plugin.as_ref());
            }
            // Retire processor handles before old plugin instances are dropped.
            process::lock(&self.pipeline).processors = processors;
            drop(old);
        }
        let mut pipeline = process::lock(&self.pipeline);
        pipeline.prepared = Some(storage);
        pipeline.transport = Some(transport);
        pipeline.parameters = parameters;
        for (slot, timing) in timings.into_iter().enumerate() {
            pipeline.changes.observe(slot, timing);
        }
        pipeline.audio_slots = config
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
        pipeline.audio_config = Some(config);
        pipeline.alignment = Some(alignment);
        pipeline.events = Some(events);
        pipeline.resync = false;
        drop(pipeline);
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

fn prepare(
    config: &RoutedChainConfig,
    slot: usize,
    plugin: &mut dyn HostedPlugin,
) -> Result<Vec<AudioBusInfo>, Error> {
    plugin.prepare_audio(&config.slot_config(slot).map_err(Error::Input)?)
}
