//! State contexts and native preset files share the staged helper replacement path.
use super::*;
use plughost_core::PluginState;

impl Chain {
    /// Drains plugin notifications. On overflow, cancel gestures and requery metadata and values.
    pub fn take_parameter_events(
        &mut self,
        slot: usize,
    ) -> Result<plughost_core::ParameterEventBatch, Error> {
        match self
            .helper
            .request(Request::ParameterEvents { slot }, self.timeouts.control)?
        {
            Response::ParameterEvents(batch) => Ok(batch),
            _ => Err(Error::Protocol),
        }
    }
    pub fn factory_presets(
        &mut self,
        slot: usize,
    ) -> Result<Vec<plughost_core::FactoryPreset>, Error> {
        match self
            .helper
            .request(Request::FactoryPresets { slot }, self.timeouts.control)?
        {
            Response::FactoryPresets(presets) => Ok(presets),
            _ => Err(Error::Protocol),
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
        match self
            .helper
            .request(Request::SaveState { slot, purpose }, self.timeouts.control)?
        {
            Response::State(state) => Ok(state),
            _ => Err(Error::Protocol),
        }
    }

    /// Stages state on a fresh instance in the helper, replacing the slot only after restore,
    /// preparation and timing checks succeed. Returned native failures preserve the active slot;
    /// a crash or timeout can still terminate the shared helper. Successful restore clears DSP
    /// history for this slot. An open editor is recreated and reopen failures enter diagnostics.
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
            state: state.clone(),
            purpose,
        };
        self.accept_restored(request)
    }

    fn accept_restored(&mut self, request: Request) -> Result<(), Error> {
        match self.helper.request(request, self.timeouts.load)? {
            Response::StateRestored { timing } => {
                if let Some((latency, tail)) = timing {
                    self.latency = latency;
                    self.tail = tail;
                }
                Ok(())
            }
            _ => Err(Error::Protocol),
        }
    }

    /// Inspects the native container without applying it. File I/O belongs to the caller.
    pub fn inspect_preset(
        &mut self,
        slot: usize,
        bytes: &[u8],
    ) -> Result<plughost_core::PresetInfo, Error> {
        check_preset_size(slot, bytes)?;
        match self.helper.request(
            Request::InspectPreset {
                slot,
                bytes: bytes.to_vec(),
            },
            self.timeouts.control,
        )? {
            Response::PresetInfo(info) => Ok(info),
            _ => Err(Error::Protocol),
        }
    }

    /// Saves native preset state as .vstpreset or .aupreset bytes. CLAP returns Unsupported.
    /// Caller-owned VST3 XML annotations are not part of the plugin state exported here.
    pub fn export_preset(&mut self, slot: usize) -> Result<Vec<u8>, Error> {
        match self
            .helper
            .request(Request::ExportPreset { slot }, self.timeouts.control)?
        {
            Response::Preset(bytes) => Ok(bytes),
            _ => Err(Error::Protocol),
        }
    }

    /// Validates the native container and class, then uses the same staged replacement as restore_state.
    pub fn import_preset(&mut self, slot: usize, bytes: &[u8]) -> Result<(), Error> {
        check_preset_size(slot, bytes)?;
        self.accept_restored(Request::ImportPreset {
            slot,
            bytes: bytes.to_vec(),
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
