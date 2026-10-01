//! State, presets and factory programs are applied to a fresh instance of the slot's plugin,
//! which replaces the active one only after the change, preparation and timing succeed. Native
//! loaders can change an instance before reporting failure and cannot roll back, so this staging
//! is what lets a failed change leave the active slot, its parameters and its DSP history intact.
use super::*;
use plughost_core::{PluginFormat, PluginInfo, PluginState};

impl Host {
    pub(super) fn import_preset(&mut self, slot: usize, bytes: &[u8]) -> Response {
        let info = self.slots[slot].info();
        let preset = match plughost_formats::preset::decode(info.format, bytes) {
            Ok(preset) => preset,
            Err(error) => return rejected_or_failed(slot, &error),
        };
        let state = PluginState {
            format: info.format,
            class_id: preset.info.class_id,
            name: info.name.clone(),
            vendor: info.vendor.clone(),
            version: info.version.clone(),
            component: preset.component,
            controller: preset.controller,
        };
        self.restore_state(slot, &state, plughost_core::StatePurpose::Preset)
    }

    pub(super) fn restore_state(
        &mut self,
        slot: usize,
        state: &PluginState,
        purpose: plughost_core::StatePurpose,
    ) -> Response {
        if !same_class(self.slots[slot].info(), state) {
            return mismatch(slot);
        }
        self.replace_candidate(slot, false, |candidate| {
            candidate.restore_state(state, purpose)
        })
    }

    pub(super) fn select_factory_preset(
        &mut self,
        slot: usize,
        preset: &plughost_core::FactoryPresetId,
    ) -> Response {
        self.replace_candidate(slot, true, |candidate| {
            candidate.select_factory_preset(preset)
        })
    }

    pub(super) fn load_discovered_preset(
        &mut self,
        slot: usize,
        location: &plughost_core::PresetLocation,
        load_key: Option<&str>,
    ) -> Response {
        self.replace_candidate(slot, false, |candidate| {
            candidate.load_discovered_preset(location, load_key)
        })
    }

    /// Factory program changes need an active processor for the native parameter flush. State
    /// and discovered presets are applied before activation. Neither path prepares twice.
    fn replace_candidate(
        &mut self,
        slot: usize,
        prepare_first: bool,
        change: impl FnOnce(&mut dyn HostedPlugin) -> Result<(), Error>,
    ) -> Response {
        let mut candidate = match plughost_formats::load(&self.references[slot], &self.identity) {
            Ok(candidate) => candidate,
            Err(error) => return failed(slot, &error),
        };
        let (audio_config, routing) = {
            let pipeline = process::lock(&self.pipeline);
            (
                pipeline.audio_slots.get(slot).cloned(),
                pipeline.audio_config.clone(),
            )
        };
        let prepare = |candidate: &mut dyn HostedPlugin| match &audio_config {
            Some(config) => candidate.prepare_audio(config).map(|_| ()),
            None => Ok(()),
        };
        let changed = if prepare_first {
            prepare(candidate.as_mut()).and_then(|()| change(candidate.as_mut()))
        } else {
            change(candidate.as_mut()).and_then(|()| prepare(candidate.as_mut()))
        };
        if let Err(error) = changed {
            return self.reject_candidate(slot, candidate.as_ref(), slot, error.failure());
        }
        let processor = candidate.processor();
        let mut retimed = None;
        if let Some(routing) = &routing {
            let current = match candidate.timing() {
                Ok(current) => current,
                Err(error) => {
                    return self.reject_candidate(slot, candidate.as_ref(), slot, error.failure());
                }
            };
            let calls = self.calls.clone();
            let mut timings = match audio_timing::chain_timings(&self.slots, &calls) {
                Ok(timings) => timings,
                Err((index, error)) => {
                    return self.reject_candidate(slot, candidate.as_ref(), index, error.failure());
                }
            };
            let pipeline = process::lock(&self.pipeline);
            // The active chain must still match its alignment before one slot is retimed.
            let checked = pipeline
                .alignment
                .as_ref()
                .ok_or_else(|| {
                    Failure::new(
                        FailureKind::NotPrepared,
                        plughost_core::messages::CHAIN_NOT_PREPARED,
                    )
                })
                .and_then(|alignment| alignment.timing.tail(&timings).map(|_| alignment));
            timings[slot] = current;
            let replacements = checked.and_then(|alignment| {
                let (plan, tail) = AudioTiming::plan(routing, &timings)?;
                Ok((
                    alignment
                        .delays32
                        .as_ref()
                        .map(|bank| bank.replacement(slot, &plan))
                        .transpose()?,
                    alignment
                        .delays64
                        .as_ref()
                        .map(|bank| bank.replacement(slot, &plan))
                        .transpose()?,
                    plan,
                    tail,
                ))
            });
            drop(pipeline);
            match replacements {
                Ok(replacements) => retimed = Some((replacements, current)),
                Err(failure) => {
                    return self.reject_candidate(slot, candidate.as_ref(), slot, failure);
                }
            }
        }
        let parameters = parameter_infos(candidate.as_mut());
        // Initialization and staging callbacks may describe intermediate values. Replacement
        // publishes one resynchronization boundary after the final candidate has been installed.
        candidate.take_parameter_events();
        // All fallible native state/setup/timing work has completed. Editor failure after a
        // successful replacement is diagnosed separately and does not undo the restored state.
        let reopen = self.editors[slot].is_some();
        if let Some(window) = self.editors[slot].take() {
            window.close(self.slots[slot].as_mut());
        }
        let mut timing = None;
        let old = {
            let mut pipeline = process::lock(&self.pipeline);
            pipeline.processors[slot] = processor;
            if let Some(((replacement32, replacement64, plan, tail), current)) = retimed {
                pipeline.parameters[slot] = parameters;
                pipeline.changes.observe(slot, current);
                timing = Some((plan.latency(), tail));
                let alignment = pipeline.alignment.as_mut().unwrap();
                if let (Some(bank), Some(replacement)) = (&mut alignment.delays32, replacement32) {
                    bank.replace_slot(replacement);
                }
                if let (Some(bank), Some(replacement)) = (&mut alignment.delays64, replacement64) {
                    bank.replace_slot(replacement);
                }
                alignment.timing = plan;
            }
            std::mem::replace(&mut self.slots[slot], candidate)
        };
        self.retain_diagnostics(slot, old.as_ref());
        self.invalidate_parameters(slot);
        drop(old);
        let mut reopen_slots = vec![false; self.slots.len()];
        reopen_slots[slot] = reopen;
        self.reopen_editors(&reopen_slots);
        Response::StateRestored { timing }
    }

    fn reject_candidate(
        &mut self,
        slot: usize,
        candidate: &dyn HostedPlugin,
        failed_slot: usize,
        failure: Failure,
    ) -> Response {
        self.retain_diagnostics(slot, candidate);
        Response::Failed {
            slot: Some(failed_slot),
            failure,
        }
    }

    pub(super) fn retain_diagnostics(&mut self, slot: usize, plugin: &dyn HostedPlugin) {
        let mut batch = plugin.take_diagnostics();
        for record in &mut batch.records {
            record.slot = Some(slot);
        }
        self.retired_diagnostics.append(batch);
    }
}

/// Whether `state` was saved by the plugin class `info` describes.
pub(super) fn same_class(info: &PluginInfo, state: &PluginState) -> bool {
    let class = if info.format == PluginFormat::Vst3 {
        info.class_id.eq_ignore_ascii_case(&state.class_id)
    } else {
        info.class_id == state.class_id
    };
    info.format == state.format && class
}

pub(super) fn mismatch(slot: usize) -> Response {
    Response::Failed {
        slot: Some(slot),
        failure: Failure::new(
            FailureKind::StateMismatch,
            plughost_core::messages::STATE_MISMATCH,
        ),
    }
}
