//! State contexts and native preset files share the staged helper replacement path.
use super::*;
use plughost_core::PluginState;

impl Chain {
    /// Drains plugin notifications. On overflow, cancel gestures and requery metadata and values.
    pub fn take_parameter_events(
        &mut self,
        slot: usize,
    ) -> Result<plughost_core::ParameterEventBatch, Error> {
        match self.request(Request::ParameterEvents { slot }, self.timeouts.control)? {
            Response::ParameterEvents(batch) => Ok(batch),
            _ => Err(self.protocol_error()),
        }
    }
    /// Lists the slot's native factory presets or programs with their selection IDs.
    pub fn factory_presets(
        &mut self,
        slot: usize,
    ) -> Result<Vec<plughost_core::FactoryPreset>, Error> {
        match self.request(Request::FactoryPresets { slot }, self.timeouts.control)? {
            Response::FactoryPresets(presets) => Ok(presets),
            _ => Err(self.protocol_error()),
        }
    }
    /// Selects on a fresh instance, replacing the whole slot only on success. VST3 needs prepare first.
    pub fn select_factory_preset(
        &mut self,
        slot: usize,
        preset: &plughost_core::FactoryPresetId,
    ) -> Result<(), Error> {
        self.accept_restored(Request::SelectFactoryPreset {
            slot,
            preset: preset.clone(),
        })
    }
    /// Applies a CLAP discovery location/key on a fresh instance. The caller selects a compatible
    /// plugin using discovery metadata. Native rejection leaves the active slot unchanged.
    pub fn load_discovered_preset(
        &mut self,
        slot: usize,
        location: &plughost_core::PresetLocation,
        load_key: Option<&str>,
    ) -> Result<(), Error> {
        if !crate::presets::encodable(location) {
            return Err(Error::Input {
                slot: Some(slot),
                error: plughost_core::InputError::PathEncoding,
            });
        }
        self.accept_restored(Request::LoadDiscoveredPreset {
            slot,
            location: location.clone(),
            load_key: load_key.map(str::to_owned),
        })
    }

    /// Saves the state of the plugin in `slot`, after its pending edits reach the processor.
    pub fn save_state(
        &mut self,
        slot: usize,
        purpose: plughost_core::StatePurpose,
    ) -> Result<PluginState, Error> {
        match self.request(Request::SaveState { slot, purpose }, self.timeouts.control)? {
            Response::State(state) => Ok(state),
            _ => Err(self.protocol_error()),
        }
    }

    /// Stages state on a fresh instance in the helper, replacing the slot only after restore,
    /// preparation and timing checks succeed. Returned native failures preserve the active slot;
    /// a crash or timeout can still terminate the shared helper. Successful restore clears DSP
    /// history for this slot. An open editor is recreated and reopen failures enter diagnostics.
    /// The slot keeps its native bypass unless the state sets it, as a VST3 or CLAP state holding
    /// a bypass parameter can; the same holds for presets and factory presets. Notes the replaced
    /// instance already sent to later slots are not ended: [`Chain::reset`] ends them.
    ///
    /// While the realtime endpoint runs, the slot is replaced between quanta and the stream goes
    /// on, as it does for presets, factory presets and [`Chain::reprepare_slot`]. A replacement
    /// that changes the chain's latency resynchronizes the endpoint right after, as
    /// [`Chain::resync_realtime`] does, so that the new latency starts with a new generation;
    /// that resets every plugin.
    pub fn restore_state(
        &mut self,
        slot: usize,
        state: &PluginState,
        purpose: plughost_core::StatePurpose,
    ) -> Result<(), Error> {
        state.validate().map_err(|error| Error::Input {
            slot: Some(slot),
            error,
        })?;
        let request = Request::RestoreState {
            slot,
            state: std::borrow::Cow::Borrowed(state),
            purpose,
        };
        self.accept_restored(request)
    }

    /// Prepares the plugin in `slot` again on a fresh instance that holds the active one's
    /// project state, replacing the slot as [`Chain::restore_state`] does, including while the
    /// realtime endpoint runs. Use it after a plugin asks to be prepared again (a VST3 or CLAP
    /// restart request, reported by [`Chain::take_changes`]): the fresh instance has no pending
    /// request, nor the DSP history or notes of the replaced one. A plugin that cannot save its
    /// state, such as an invalidated Audio Unit, fails the call and stays as it is; restore a
    /// state the application saved instead.
    pub fn reprepare_slot(&mut self, slot: usize) -> Result<(), Error> {
        if self.config.is_none() {
            return Err(Error::NotPrepared);
        }
        self.accept_restored(Request::ReprepareSlot { slot })
    }

    fn accept_restored(&mut self, request: Request<'_>) -> Result<(), Error> {
        match self.request(request, self.timeouts.load)? {
            Response::StateRestored { timing } => {
                if let Some((latency, tail)) = timing {
                    let changed = latency != self.latency;
                    self.latency = latency;
                    self.tail = tail;
                    // The endpoint does not go on in its generation's timeline at another latency.
                    if changed && self.realtime_running() {
                        self.resync_realtime()?;
                    }
                }
                Ok(())
            }
            _ => Err(self.protocol_error()),
        }
    }

    /// Inspects the native container without applying it. File I/O belongs to the caller.
    pub fn inspect_preset(
        &mut self,
        slot: usize,
        bytes: &[u8],
    ) -> Result<plughost_core::PresetInfo, Error> {
        check_preset_size(slot, bytes)?;
        match self.request(
            Request::InspectPreset {
                slot,
                bytes: bytes.into(),
            },
            self.timeouts.control,
        )? {
            Response::PresetInfo(info) => Ok(info),
            _ => Err(self.protocol_error()),
        }
    }

    /// Saves native preset state as .vstpreset or .aupreset bytes. CLAP returns Unsupported.
    /// Caller-owned VST3 XML annotations are not part of the plugin state exported here.
    pub fn export_preset(&mut self, slot: usize) -> Result<Vec<u8>, Error> {
        match self.request(Request::ExportPreset { slot }, self.timeouts.control)? {
            Response::Preset(bytes) => Ok(bytes),
            _ => Err(self.protocol_error()),
        }
    }

    /// Validates the native container and class, then uses the same staged replacement as restore_state.
    pub fn import_preset(&mut self, slot: usize, bytes: &[u8]) -> Result<(), Error> {
        check_preset_size(slot, bytes)?;
        self.accept_restored(Request::ImportPreset {
            slot,
            bytes: bytes.into(),
        })
    }
}

fn check_preset_size(slot: usize, bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() > plughost_core::MAX_PRESET_BYTES {
        Err(Error::Input {
            slot: Some(slot),
            error: plughost_core::InputError::PresetSize,
        })
    } else {
        Ok(())
    }
}
