use super::*;
use clack_extensions::audio_ports::{AudioPortInfo, AudioPortInfoBuffer};
use clack_extensions::audio_ports_activation::SampleSize;
use clack_extensions::audio_ports_config::AudioPortsConfigBuffer;
use plughost_core::{
    AudioBusInfo, AudioBusRole, AudioConfig, AudioConfiguration, AudioDirection, BypassState,
    Support,
};
impl Plugin {
    /// The explicit full-bus configuration, absent before preparation or after legacy prepare.
    pub fn audio_config(&self) -> Option<AudioConfig> {
        lock(&self.engine)
            .prepared
            .as_ref()
            .and_then(|prepared| prepared.audio_config.clone())
    }

    pub(crate) fn audio_buses(&mut self) -> Result<Vec<AudioBusInfo>, ClapError> {
        self.read_audio_buses()
    }
    fn read_audio_buses(&mut self) -> Result<Vec<AudioBusInfo>, ClapError> {
        if self
            .shared()
            .audio_ports_reset
            .swap(false, Ordering::Relaxed)
        {
            self.audio_active.clear();
        }
        let Some(extension) = self.shared().extensions().audio_ports else {
            return Ok(Vec::new());
        };
        let surround = self.shared().extensions().surround;
        let handle = self.instance.plugin_handle();
        let mut result = Vec::new();
        let mut buffer = AudioPortInfoBuffer::new();
        for input in [true, false] {
            // Callers receive at most this many buses per direction.
            let count = extension.count(&handle, input);
            if count as usize > plughost_core::MAX_AUDIO_BUSES {
                return Err(ClapError::AudioMetadata);
            }
            for index in 0..count {
                let info = extension
                    .get(&handle, index, input, &mut buffer)
                    .ok_or(ClapError::AudioMetadata)?;
                let mut bus = bus_info(&info, index, input);
                bus.active = Some(
                    self.audio_active
                        .get(&(input, bus.id))
                        .copied()
                        .unwrap_or(true),
                );
                if info.port_type == Some(AudioPortType::SURROUND)
                    && let Some(surround) = surround
                {
                    let mut storage = [0; 64];
                    let map = surround.get_channel_map(&handle, input, index, &mut storage);
                    let channels: Vec<_> = (0..map.channel_count())
                        .filter_map(|i| map.get(i))
                        .collect();
                    bus.layout =
                        [Layout::Surround51, Layout::Surround71]
                            .into_iter()
                            .find(|layout| {
                                super::surround_orders(*layout)
                                    .is_some_and(|orders| orders.contains(&channels.as_slice()))
                            });
                }
                result.push(bus);
            }
        }
        Ok(result)
    }
    pub(crate) fn audio_configurations(&mut self) -> Result<Vec<AudioConfiguration>, ClapError> {
        let extensions = self.shared().extensions();
        let extension = extensions.audio_config.ok_or(ClapError::AudioUnsupported)?;
        let info = extensions
            .audio_config_info
            .ok_or(ClapError::AudioUnsupported)?;
        let handle = self.instance.plugin_handle();
        let mut buffer = AudioPortsConfigBuffer::new();
        let mut result = Vec::new();
        for index in 0..extension.count(&handle) {
            let config = extension
                .get(&handle, index, &mut buffer)
                .ok_or(ClapError::AudioMetadata)?;
            let buses = |input: bool, count: u32| -> Result<Vec<AudioBusInfo>, ClapError> {
                let mut buffer = AudioPortInfoBuffer::new();
                let mut result = Vec::new();
                for index in 0..count {
                    let port = info
                        .get(&handle, config.id, index, input, &mut buffer)
                        .ok_or(ClapError::AudioMetadata)?;
                    result.push(bus_info(&port, index, input));
                }
                Ok(result)
            };
            result.push(AudioConfiguration {
                id: u64::from(config.id.get()),
                name: String::from_utf8_lossy(config.name).into_owned(),
                inputs: buses(true, config.input_port_count)?,
                outputs: buses(false, config.output_port_count)?,
            });
        }
        Ok(result)
    }
    /// Reserves native audio storage for the maximum block size. Larger maxima increase
    /// memory requirements; native plugin code and control callbacks may still allocate while processing.
    pub(crate) fn prepare(&mut self, config: &ProcessConfig) -> Result<(), ClapError> {
        config.validate().map_err(ClapError::Input)?;
        self.deactivate();
        let mut buses = self.read_audio_buses()?;
        for bus in &mut buses {
            if bus.active == Some(false) {
                let activation = self
                    .shared()
                    .extensions()
                    .audio_activation
                    .ok_or(ClapError::AudioUnsupported)?;
                let mut handle = self
                    .instance
                    .inactive_plugin_handle()
                    .ok_or(ClapError::AudioConfiguration)?;
                let size = match config.sample_format {
                    SampleFormat::F32 => SampleSize::Float32,
                    SampleFormat::F64 => SampleSize::Float64,
                };
                if !activation.set_active_audio_inactive(
                    &mut handle,
                    bus.direction == AudioDirection::Input,
                    bus.index,
                    true,
                    size,
                ) {
                    return Err(ClapError::AudioConfiguration);
                }
                self.audio_active
                    .insert((bus.direction == AudioDirection::Input, bus.id), true);
                bus.active = Some(true);
            }
        }
        let main_input = main_bus(&buses, AudioDirection::Input, config.input)?;
        let main_output = main_bus(&buses, AudioDirection::Output, config.output)?;
        let events = self.main_event_config()?;
        self.activate_audio(*config, buses, None, &events, main_input, main_output)
    }
    /// Prepares all requested buses and reserves native audio storage for the maximum block size.
    /// A failure after validation may leave this instance unprepared. Processing may still allocate.
    pub(crate) fn prepare_audio(
        &mut self,
        config: &AudioConfig,
    ) -> Result<Vec<AudioBusInfo>, ClapError> {
        config.validate().map_err(ClapError::Input)?;
        self.deactivate();
        if let Some(id) = config.configuration {
            let id = u32::try_from(id)
                .ok()
                .and_then(ClapId::from_raw)
                .ok_or(ClapError::AudioConfiguration)?;
            let extension = self
                .shared()
                .extensions()
                .audio_config
                .ok_or(ClapError::AudioUnsupported)?;
            extension
                .select(&self.instance.plugin_handle(), id)
                .map_err(|_| ClapError::AudioConfiguration)?;
            self.audio_active.clear();
        }
        let mut buses = self.read_audio_buses()?;
        for (direction, requests) in [
            (AudioDirection::Input, &config.inputs),
            (AudioDirection::Output, &config.outputs),
        ] {
            if requests.iter().any(|r| {
                !buses
                    .iter()
                    .any(|b| b.direction == direction && b.id == r.id)
            }) {
                return Err(ClapError::AudioConfiguration);
            }
            for bus in buses.iter_mut().filter(|b| b.direction == direction) {
                let request = requests.iter().find(|r| r.id == bus.id);
                let active = request.is_some_and(|r| r.active);
                if let Some(request) = request
                    && (request.active || request.layout != Layout::None)
                    && bus.layout != Some(request.layout)
                {
                    return Err(ClapError::AudioConfiguration);
                }
                // Without port activation every port stays natively active; an omitted port then
                // receives host silence and its output is discarded, as with main-bus preparation.
                if let Some(activation) = self.shared().extensions().audio_activation {
                    let mut handle = self
                        .instance
                        .inactive_plugin_handle()
                        .ok_or(ClapError::AudioConfiguration)?;
                    let size = if !active {
                        SampleSize::Unspecified
                    } else {
                        match config.sample_format {
                            SampleFormat::F32 => SampleSize::Float32,
                            SampleFormat::F64 => SampleSize::Float64,
                        }
                    };
                    if !activation.set_active_audio_inactive(
                        &mut handle,
                        direction == AudioDirection::Input,
                        bus.index,
                        active,
                        size,
                    ) {
                        return Err(ClapError::AudioConfiguration);
                    }
                }
                self.audio_active
                    .insert((direction == AudioDirection::Input, bus.id), active);
                bus.active = Some(active);
            }
        }
        let main_input = buses
            .iter()
            .find(|b| b.direction == AudioDirection::Input && b.role == AudioBusRole::Main)
            .map(|b| b.index as usize);
        let main_output = buses
            .iter()
            .find(|b| b.direction == AudioDirection::Output && b.role == AudioBusRole::Main)
            .map(|b| b.index as usize);
        let main_layout = |direction| {
            buses
                .iter()
                .find(|b| b.direction == direction && b.role == AudioBusRole::Main)
                .and_then(|b| b.layout)
                .unwrap_or(Layout::None)
        };
        let scalar = ProcessConfig {
            sample_rate: config.sample_rate,
            max_block_size: config.max_block_size,
            sample_format: config.sample_format,
            mode: config.mode,
            input: main_layout(AudioDirection::Input),
            output: main_layout(AudioDirection::Output),
        };
        self.activate_audio(
            scalar,
            buses.clone(),
            Some(config.clone()),
            &config.events,
            main_input,
            main_output,
        )?;
        Ok(buses)
    }
    fn activate_audio(
        &mut self,
        config: ProcessConfig,
        buses: Vec<AudioBusInfo>,
        audio_config: Option<AudioConfig>,
        events: &plughost_core::EventConfig,
        main_input: Option<usize>,
        main_output: Option<usize>,
    ) -> Result<(), ClapError> {
        if config.sample_format == SampleFormat::F64
            && buses
                .iter()
                .any(|b| b.active != Some(false) && b.f64 != Support::Supported)
        {
            return Err(ClapError::SampleFormatUnsupported(config.sample_format));
        }
        // Host storage is reserved for every channel of every port.
        if buses
            .iter()
            .any(|bus| bus.channels as usize > plughost_core::MAX_AUDIO_CHANNELS)
        {
            return Err(ClapError::AudioMetadata);
        }
        let buffers =
            buffers::AudioBuffers::new(&buses, config.sample_format, config.max_block_size)?;
        let input_events = input_events::InputBuffer::new()?;
        let event_inputs = self.event_inputs(events)?;
        let extensions = self.shared().extensions();
        let (render, latency_extension) = (extensions.render, extensions.latency);
        let handle = self.instance.plugin_handle();
        if let Some(render) = render {
            if config.mode == ProcessMode::Offline && render.has_realtime_requirement(&handle) {
                return Err(ClapError::RealtimeOnly);
            }
            let mode = match config.mode {
                ProcessMode::Offline => RenderMode::Offline,
                ProcessMode::Realtime => RenderMode::Realtime,
            };
            if render.set(&handle, mode).is_err() {
                return Err(ClapError::AudioConfiguration);
            }
        }
        self.shared()
            .restart_requested
            .store(false, Ordering::Relaxed);
        let processor = self
            .instance
            .activate(
                |_, _| super::super::host::AudioThread,
                PluginAudioConfiguration {
                    sample_rate: config.sample_rate,
                    min_frames_count: 1,
                    max_frames_count: config.max_block_size as u32,
                },
            )
            .map_err(|e| ClapError::Activate(e.to_string()))?;
        let latency = latency_extension.map_or(0, |e| e.get(&self.instance.plugin_handle()));
        self.shared()
            .latency_changed
            .store(false, Ordering::Relaxed);
        let parameters = self.parameter_cache();
        let inputs = buses
            .iter()
            .filter(|b| b.direction == AudioDirection::Input)
            .map(|b| b.channels as usize)
            .collect();
        let outputs = buses
            .iter()
            .filter(|b| b.direction == AudioDirection::Output)
            .map(|b| b.channels as usize)
            .collect();
        let mut engine = lock(&self.engine);
        engine.processor = Some(processor.into());
        engine.prepared = Some(Prepared {
            input_events,
            buffers,
            config,
            audio_config,
            buses,
            inputs,
            outputs,
            main_input,
            main_output,
            event_inputs,
            latency,
            steady_time: 0,
            parameters,
        });
        Ok(())
    }
    fn bypass_parameter(&mut self) -> Result<Option<u64>, ClapError> {
        let mut result = None;
        for parameter in self.parameter_cache().iter() {
            let flags = parameter.info.flags;
            if flags.bypass && !flags.read_only {
                if result.is_some() {
                    return Err(ClapError::AudioMetadata);
                }
                result = Some(parameter.info.id);
            }
        }
        Ok(result)
    }
    pub(crate) fn bypass(&mut self) -> Result<BypassState, ClapError> {
        self.flush();
        let Some(id) = self.bypass_parameter()? else {
            return Ok(BypassState {
                support: Support::Unsupported,
                enabled: None,
            });
        };
        let enabled = self
            .parameters()
            .iter()
            .find(|(info, _)| info.id == id)
            .map(|(_, value)| *value >= 0.5)
            .ok_or(ClapError::AudioMetadata)?;
        Ok(BypassState {
            support: Support::Supported,
            enabled: Some(enabled),
        })
    }
    pub(crate) fn set_bypass(&mut self, enabled: bool) -> Result<(), ClapError> {
        let id = self
            .bypass_parameter()?
            .ok_or(ClapError::BypassUnsupported)?;
        self.set_parameter(id, f64::from(u8::from(enabled)))?;
        self.flush();
        Ok(())
    }
}
fn bus_info(info: &AudioPortInfo<'_>, index: u32, input: bool) -> AudioBusInfo {
    let layout = match (info.channel_count, info.port_type) {
        (0, _) => Some(Layout::None),
        (1, Some(kind)) if kind == AudioPortType::MONO => Some(Layout::Mono),
        (2, Some(kind)) if kind == AudioPortType::STEREO => Some(Layout::Stereo),
        _ => None,
    };
    AudioBusInfo {
        id: u64::from(info.id.get()),
        index,
        name: String::from_utf8_lossy(info.name).into_owned(),
        direction: if input {
            AudioDirection::Input
        } else {
            AudioDirection::Output
        },
        role: if info.flags.contains(AudioPortFlags::IS_MAIN) {
            AudioBusRole::Main
        } else {
            AudioBusRole::Auxiliary
        },
        layout,
        channels: info.channel_count,
        active: Some(true),
        f32: Support::Supported,
        f64: info.flags.contains(AudioPortFlags::SUPPORTS_64BITS).into(),
    }
}
fn main_bus(
    buses: &[AudioBusInfo],
    direction: AudioDirection,
    layout: Layout,
) -> Result<Option<usize>, ClapError> {
    if layout == Layout::None {
        return Ok(None);
    }
    let mut main = buses
        .iter()
        .filter(|b| b.direction == direction && b.role == AudioBusRole::Main);
    let bus = main.next();
    if main.next().is_some() {
        return Err(ClapError::AudioMetadata);
    }
    match bus {
        Some(bus) if bus.layout == Some(layout) => Ok(Some(bus.index as usize)),
        bus => Err(ClapError::LayoutRefused {
            requested: layout,
            plugin_channels: bus.map_or(0, |b| b.channels),
        }),
    }
}
