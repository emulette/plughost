use super::{Plugin, Vst3Error, lock};
use plughost_core::{AudioBusInfo, AudioConfig, AudioConfiguration, BypassState, Support};
use vst3::Steinberg::Vst::{IComponentHandlerTrait, IEditControllerTrait};
use vst3::Steinberg::kResultOk;

impl Plugin {
    /// The complete request currently prepared on this native instance.
    pub fn audio_config(&self) -> Option<AudioConfig> {
        lock(&self.engine)
            .prepared
            .as_ref()
            .map(|prepared| prepared.audio_config.clone())
    }
    /// Full native bus order. Current activation is only known after host preparation.
    pub(crate) fn audio_buses(&self) -> Result<Vec<AudioBusInfo>, Vst3Error> {
        let engine = lock(&self.engine);
        let mut buses = engine.instance()?.audio_buses()?;
        if let Some(prepared) = &engine.prepared {
            for bus in &mut buses {
                bus.active = prepared
                    .audio_buses
                    .iter()
                    .find(|known| known.id == bus.id && known.direction == bus.direction)
                    .and_then(|known| known.active);
            }
        }
        Ok(buses)
    }
    /// Event buses in both directions, inputs first; each ID is the bus index.
    pub(crate) fn event_ports(&self) -> Result<Vec<plughost_core::EventPortInfo>, Vst3Error> {
        lock(&self.engine).instance()?.event_ports()
    }
    /// VST3 negotiates speaker arrangements but has no complete configuration enumerator.
    pub(crate) fn audio_configurations(&self) -> Result<Vec<AudioConfiguration>, Vst3Error> {
        lock(&self.engine).instance()?;
        Ok(Vec::new())
    }
    /// Negotiates every requested bus, deactivating omitted buses. Preparation failure
    /// may invalidate this instance; the helper stages another instance before replacement.
    /// Reserves native audio storage for the maximum block size. Larger maxima increase
    /// memory requirements; native plugin code and control callbacks may still allocate while processing.
    pub(crate) fn prepare_audio(
        &mut self,
        config: &AudioConfig,
    ) -> Result<Vec<AudioBusInfo>, Vst3Error> {
        let parameters = self.parameter_cache();
        lock(&self.engine).prepare_audio(config, &parameters)
    }
    pub(crate) fn bypass(&self) -> Result<BypassState, Vst3Error> {
        let Some(id) = self.bypass_parameter() else {
            return Ok(BypassState {
                support: Support::Unsupported,
                enabled: None,
            });
        };
        self.sync_controller();
        let value = unsafe { self.controller.getParamNormalized(id) };
        Ok(BypassState {
            support: Support::Supported,
            enabled: Some(value >= 0.5),
        })
    }
    /// Uses the plugin's native bypass parameter; no host dry path is substituted.
    pub(crate) fn set_bypass(&mut self, enabled: bool) -> Result<(), Vst3Error> {
        let id = self
            .bypass_parameter()
            .ok_or(Vst3Error::BypassUnsupported)?;
        if lock(&self.engine).prepared.is_none() {
            return Err(Vst3Error::NotPrepared);
        }
        let value = if enabled { 1.0 } else { 0.0 };
        if !self.handler.can_edit(id) {
            return Err(Vst3Error::Input(plughost_core::InputError::EventCapacity));
        }
        let result = unsafe { self.controller.setParamNormalized(id, value) };
        if result != kResultOk {
            return Err(Vst3Error::BypassEdit(result));
        }
        if unsafe { self.handler.performEdit(id, value) } != kResultOk {
            return Err(Vst3Error::Input(plughost_core::InputError::EventCapacity));
        }
        self.flush()
    }

    /// The first writable parameter flagged as bypass, as JUCE chooses it.
    fn bypass_parameter(&self) -> Option<u32> {
        let parameters = self.parameter_cache();
        (0..parameters.len())
            .find(|&index| {
                let flags = parameters.info(index).flags;
                flags.bypass && !flags.read_only
            })
            .map(|index| parameters.native_id(index))
    }
}
