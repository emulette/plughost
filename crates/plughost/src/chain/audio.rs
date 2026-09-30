//! Bus configuration, typed audio transport and offline render adapters.

use super::*;
use plughost_core::render::Process;
use plughost_core::{
    AudioBusConfig, AudioBusInfo, AudioBusRole, AudioConfiguration, AudioDirection,
    AudioInputRoute, AudioSource, AutomationEvent, BlockContext, BypassState, ChannelAdaptation,
    EventInputRoute, EventPortInfo, EventSource, InputError, Layout, ProcessMode,
    RoutedChainConfig, Sample, SampleFormat, SlotAudioConfig, SlotEventConfig,
};

impl Chain {
    /// Current native bus map, including auxiliary and inactive buses. IDs are scoped to the
    /// direction and selected native configuration; `index` defines channel buffer order.
    pub fn audio_buses(&mut self, slot: usize) -> Result<Vec<AudioBusInfo>, Error> {
        match self
            .helper
            .request(Request::AudioBuses { slot }, self.timeouts.control)?
        {
            Response::AudioBuses(buses) => Ok(buses),
            _ => Err(Error::Protocol),
        }
    }

    /// Native event ports in both directions, inputs first. IDs are scoped to the direction;
    /// routes name them in [`SlotEventConfig`].
    pub fn event_ports(&mut self, slot: usize) -> Result<Vec<EventPortInfo>, Error> {
        match self
            .helper
            .request(Request::EventPorts { slot }, self.timeouts.control)?
        {
            Response::EventPorts(ports) => Ok(ports),
            _ => Err(Error::Protocol),
        }
    }

    /// Enumerates selectable native configurations. An empty list does not imply that the
    /// current buses cannot be configured individually.
    pub fn audio_configurations(&mut self, slot: usize) -> Result<Vec<AudioConfiguration>, Error> {
        match self
            .helper
            .request(Request::AudioConfigurations { slot }, self.timeouts.control)?
        {
            Response::AudioConfigurations(configurations) => Ok(configurations),
            _ => Err(Error::Protocol),
        }
    }

    /// A serial chain through each slot's main buses, in f32 and [`ProcessMode::Offline`]: the
    /// chain input feeds the first slot's main input, each slot's main output feeds the next
    /// slot's main input, and `outputs` holds each slot's main output layout. `Layout::None`
    /// leaves a main bus inactive, as are all auxiliary buses. Events follow the same path: the
    /// chain's one event input reaches the first slot's first event input, each slot's first
    /// event output reaches the next slot's first event input, and the last slot's first event
    /// output is the chain's event output. A slot without such a port breaks the event path at
    /// that point. Adjust fields such as `sample_format` or `mode` before
    /// [`Chain::prepare_audio`].
    pub fn main_bus_config(
        &mut self,
        sample_rate: f64,
        max_block_size: usize,
        input: Layout,
        outputs: &[Layout],
    ) -> Result<RoutedChainConfig, Error> {
        if outputs.len() != self.plugins.len() {
            return Err(Error::Input {
                slot: None,
                error: InputError::OutputCount,
            });
        }
        let mut slots = Vec::with_capacity(outputs.len());
        let mut source = (input, AudioSource::External { bus: 0 });
        let mut event_source = Some(EventSource::External { port: 0 });
        for (slot, &output) in outputs.iter().enumerate() {
            let buses = self.audio_buses(slot)?;
            let ports = self.event_ports(slot)?;
            let first = |direction| {
                ports
                    .iter()
                    .find(|port| port.direction == direction)
                    .map(|port| port.id)
            };
            let event_input = first(AudioDirection::Input);
            let event_output = first(AudioDirection::Output);
            let events = SlotEventConfig {
                inputs: event_input
                    .zip(event_source)
                    .map(|(port, source)| EventInputRoute { port, source })
                    .into_iter()
                    .collect(),
                outputs: event_output.into_iter().collect(),
            };
            event_source = event_output.map(|port| EventSource::Previous { port });
            let main = |direction, layout: Layout| -> Result<Vec<AudioBusConfig>, Error> {
                if layout == Layout::None {
                    return Ok(Vec::new());
                }
                let bus = buses
                    .iter()
                    .find(|bus| bus.direction == direction && bus.role == AudioBusRole::Main)
                    .ok_or(Error::Input {
                        slot: Some(slot),
                        error: InputError::MainBus,
                    })?;
                Ok(vec![AudioBusConfig {
                    id: bus.id,
                    layout,
                    active: true,
                }])
            };
            let (layout, from) = source;
            let inputs = main(AudioDirection::Input, layout)?
                .into_iter()
                .map(|bus| AudioInputRoute {
                    bus,
                    source: from,
                    adaptation: ChannelAdaptation::Exact,
                })
                .collect();
            let outputs = main(AudioDirection::Output, output)?;
            source = (
                output,
                AudioSource::Previous {
                    bus: outputs.first().map_or(0, |bus| bus.id),
                },
            );
            slots.push(SlotAudioConfig {
                configuration: None,
                inputs,
                outputs,
                events,
            });
        }
        Ok(RoutedChainConfig {
            sample_rate,
            max_block_size,
            sample_format: SampleFormat::F32,
            mode: ProcessMode::Offline,
            inputs: if input == Layout::None {
                Vec::new()
            } else {
                vec![input]
            },
            event_inputs: 1,
            slots,
        })
    }

    pub fn bypass(&mut self, slot: usize) -> Result<BypassState, Error> {
        match self
            .helper
            .request(Request::Bypass { slot }, self.timeouts.control)?
        {
            Response::Bypass(state) => Ok(state),
            _ => Err(Error::Protocol),
        }
    }

    /// Sets native bypass. Unsupported plugins return an explicit failure; audio routing is
    /// unchanged and no host dry/wet substitution is performed.
    pub fn set_bypass(&mut self, slot: usize, enabled: bool) -> Result<(), Error> {
        match self
            .helper
            .request(Request::SetBypass { slot, enabled }, self.timeouts.control)?
        {
            Response::Bypass(_) => Ok(()),
            _ => Err(Error::Protocol),
        }
    }

    /// Negotiates every slot and returns its complete native bus map. Caller input channels are
    /// concatenated in `config.inputs` order; active output buses use native index order.
    /// Reconfiguration of a prepared chain stages fresh instances and commits only on success.
    /// Initial preparation can partially configure an unprepared native instance on failure.
    /// Reserves helper-owned audio storage for the configured maximum block size.
    /// External-input alignment storage is also reserved, up to a 256 MiB budget.
    /// Exceeding that budget fails preparation before replacing a prepared chain.
    /// Larger maxima and latencies increase memory requirements; processing is not allocation-free.
    /// Shared PCM/event slots have a separate 256 MiB budget per prepared generation. Candidate
    /// storage is reserved before replacing a prepared chain; both generations can coexist.
    pub fn prepare_audio(
        &mut self,
        config: &RoutedChainConfig,
    ) -> Result<Vec<Vec<AudioBusInfo>>, Error> {
        config
            .validate(self.plugins.len())
            .map_err(|error| Error::Input { slot: None, error })?;
        let candidate = self.transport_candidate(plughost_core::ipc::shared::SlotConfig {
            sample_format: config.sample_format,
            max_frames: config.max_block_size,
            input_channels: config.input_channels(),
            output_channels: config.output_channels(),
        })?;
        let memory = candidate.memory.descriptor();
        match self.prepare_transport(&candidate, |handle| Request::PrepareAudio {
            config: config.clone(),
            memory,
            handle,
        })? {
            // The generation names the shared mapping the helper adopted.
            Response::AudioPrepared {
                generation,
                buses,
                latency,
                tail,
            } if generation == memory.generation => {
                self.shared = Some(candidate);
                self.config = Some(config.clone());
                self.latency = latency;
                self.tail = tail;
                Ok(buses)
            }
            _ => Err(self.protocol_error()),
        }
    }

    fn validate_audio_block<S: Sample>(
        &self,
        context: &BlockContext,
        input: &[&[S]],
        output: &[&mut [S]],
        automation: &[AutomationEvent],
        events: &[MidiEvent],
    ) -> Result<(), Error> {
        self.validate_render_precision(S::FORMAT)?;
        let config = self.config.as_ref().ok_or(Error::NotPrepared)?;
        context
            .validate()
            .map_err(|error| Error::Input { slot: None, error })?;
        if let Some(transport) = context.transport {
            transport
                .validate_at_rate(config.sample_rate)
                .map_err(|error| Error::Input { slot: None, error })?;
        }
        plughost_core::validate_event_budget(automation.len(), events)
            .map_err(|error| Error::Input { slot: None, error })?;
        if let Some(event) = events
            .iter()
            .find(|event| event.port >= config.event_inputs)
        {
            return Err(Error::Input {
                slot: None,
                error: InputError::EventPort { port: event.port },
            });
        }
        let frames = context.frames;
        if automation.iter().any(|event| {
            event.slot >= self.plugins.len() || !plughost_core::changes_fit(&[event.change], frames)
        }) || automation
            .windows(2)
            .any(|pair| pair[0].change.offset > pair[1].change.offset)
        {
            return Err(Error::Input {
                slot: None,
                error: InputError::Automation,
            });
        }
        if input.len() != config.input_channels()
            || output.len() != config.output_channels()
            || frames > config.max_block_size
            || input.iter().any(|channel| channel.len() != frames)
            || output.iter().any(|channel| channel.len() != frames)
            || !events_fit(events, frames)
        {
            return Err(Error::Buffers);
        }
        Ok(())
    }

    fn validate_render_precision(&self, format: SampleFormat) -> Result<(), Error> {
        let prepared = self
            .config
            .as_ref()
            .map(|config| config.sample_format)
            .ok_or(Error::NotPrepared)?;
        if prepared != format {
            return Err(Error::Input {
                slot: None,
                error: InputError::AudioSampleFormat,
            });
        }
        Ok(())
    }

    fn validate_render_automation(&mut self, automation: &[AutomationEvent]) -> Result<(), Error> {
        let mut metadata = std::collections::HashMap::new();
        for event in automation {
            if let std::collections::hash_map::Entry::Vacant(entry) = metadata.entry(event.slot) {
                entry.insert(self.parameters(event.slot)?);
            }
            let info = metadata[&event.slot]
                .iter()
                .find(|(info, _)| info.id == event.change.id)
                .ok_or(Error::Input {
                    slot: Some(event.slot),
                    error: InputError::UnknownParameter {
                        id: event.change.id,
                    },
                })?;
            info.0
                .automation_value(event.change.value)
                .map_err(|error| Error::Input {
                    slot: Some(event.slot),
                    error,
                })?;
        }
        Ok(())
    }
}

// Shared transport retains the prepared native precision.
macro_rules! audio_process {
    ($sample:ty, $method:ident, $shared:ident) => {
        impl Chain {
            /// Processes one block through the chain. `input` holds one slice per external input
            /// channel, concatenated in `config.inputs` order, and `output` one per active output
            /// channel of the last slot, all `context.frames` long. `events` name the chain's
            /// event inputs by port and follow the prepared event routes; they are in offset
            /// order, inside the block. Automation addresses slots independently of event
            /// routing. Automation plus events may contain at most [`crate::MAX_BLOCK_EVENTS`]
            /// entries and [`crate::MAX_BLOCK_SYSEX_BYTES`] of system exclusive data per block.
            /// Invalid input is rejected before any slot or delay history advances, and `output`
            /// and `produced` change only after the helper has completed the block. `produced` is
            /// replaced with the chain's output events, by event output port.
            ///
            /// Events are delivered in the block they are submitted or produced in, without
            /// alignment for plugin latency. When a plugin produces, or routing gathers, more
            /// events than one block allows, the block fails and processing is refused until
            /// [`Chain::reset`], which also ends notes that slots still hold.
            pub fn $method(
                &mut self,
                context: &BlockContext,
                input: &[&[$sample]],
                output: &mut [&mut [$sample]],
                automation: &[AutomationEvent],
                events: &[MidiEvent],
                produced: &mut Vec<MidiEvent>,
            ) -> Result<(), Error> {
                self.validate_audio_block(context, input, output, automation, events)?;
                self.$shared(context, input, output, automation, events, produced)
            }
        }
    };
}
audio_process!(f32, process_audio_f32, process_shared_f32);
audio_process!(f64, process_audio_f64, process_shared_f64);

macro_rules! render_process {
    ($sample:ty, $method:ident) => {
        impl Process<$sample> for Chain {
            type Error = Error;
            fn sample_rate(&self) -> f64 {
                self.config
                    .as_ref()
                    .map_or(0.0, |config| config.sample_rate)
            }
            fn max_block_size(&self) -> usize {
                self.config
                    .as_ref()
                    .map_or(0, |config| config.max_block_size)
            }
            fn input_channels(&self) -> usize {
                self.config
                    .as_ref()
                    .map_or(0, RoutedChainConfig::input_channels)
            }
            fn output_channels(&self) -> usize {
                self.config
                    .as_ref()
                    .map_or(0, RoutedChainConfig::output_channels)
            }
            fn latency(&self) -> Result<u32, Error> {
                Ok(self.latency)
            }
            fn tail(&self) -> Result<Tail, Error> {
                Ok(self.tail)
            }
            fn validate_automation(&mut self, automation: &[AutomationEvent]) -> Result<(), Error> {
                self.validate_render_precision(<$sample as Sample>::FORMAT)?;
                self.validate_render_automation(automation)
            }
            fn process(
                &mut self,
                context: &BlockContext,
                input: &[&[$sample]],
                output: &mut [&mut [$sample]],
                automation: &[AutomationEvent],
                events: &[MidiEvent],
                produced: &mut Vec<MidiEvent>,
            ) -> Result<(), Error> {
                Chain::$method(self, context, input, output, automation, events, produced)
            }
        }
    };
}
render_process!(f32, process_audio_f32);
render_process!(f64, process_audio_f64);
