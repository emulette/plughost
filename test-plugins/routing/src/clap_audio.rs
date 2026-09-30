use super::*;
use clack_plugin::process::audio::SampleType;
pub struct Processor<'a> {
    pub(super) shared: &'a Shared,
}
impl<'a> PluginAudioProcessor<'a, Shared, MainThread<'a>> for Processor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main: &MainThread<'a>,
        shared: &'a Shared,
        config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        if config.sample_rate == 12345.0 {
            lock(&shared.values)[0] = 0.0;
            return Err(PluginError::Message(crate::errors::SAMPLE_RATE));
        }
        Ok(Self { shared })
    }
    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        super::events::route(events.input, events.output);
        let frames = audio.frames_count() as usize;
        let values = crate::frame_values(
            *lock(&self.shared.values),
            frames,
            events.input.iter().filter_map(|event| {
                if let Some(CoreEventSpace::ParamValue(value)) = event.as_core_event()
                    && let Some(id) = value.param_id()
                    && id.get() < 2
                {
                    Some((
                        value.header().time() as usize,
                        id.get() as usize,
                        value.value(),
                    ))
                } else {
                    None
                }
            }),
        );
        self.shared.apply(events.input);
        let active = *lock(&self.shared.active);
        let mut inputs: Vec<Vec<Vec<f64>>> = Vec::new();
        for (index, port) in audio.input_ports().enumerate() {
            let captured = match port.channels()? {
                SampleType::F32(channels) => channels
                    .iter()
                    .map(|s| s.iter().map(|v| f64::from(*v)).collect())
                    .collect(),
                SampleType::F64(channels) | SampleType::Both(_, channels) => {
                    channels.iter().map(|s| s.to_vec()).collect()
                }
            };
            inputs.push(captured);
            if !active[0][index] {
                for channel in &mut inputs[index] {
                    channel.fill(0.0);
                }
            }
        }
        let read = |bus: usize, channel: usize, frame: usize| {
            inputs
                .get(bus)
                .and_then(|p| p.get(channel))
                .map_or(0.0, |s| s[frame])
        };
        for (bus, mut port) in audio.output_ports().enumerate() {
            let value = |channel: usize, frame: usize| {
                let key = read(0, 0, frame);
                let main = read(1, channel, frame);
                if !active[1][bus] {
                    0.0
                } else if bus == 0 {
                    if channel == 0 { read(1, 0, frame) } else { key }
                } else if values[frame][1] >= 0.5 {
                    main
                } else {
                    main * values[frame][0] * (1.0 - key.abs().min(1.0))
                }
            };
            match port.channels()? {
                SampleType::F32(mut channels) => {
                    for (channel, samples) in channels.iter_mut().enumerate() {
                        for (frame, sample) in samples.iter_mut().enumerate() {
                            *sample = value(channel, frame) as f32;
                        }
                    }
                }
                SampleType::F64(mut channels) | SampleType::Both(_, mut channels) => {
                    for (channel, samples) in channels.iter_mut().enumerate() {
                        for (frame, sample) in samples.iter_mut().enumerate() {
                            *sample = value(channel, frame);
                        }
                    }
                }
            }
        }
        Ok(ProcessStatus::Continue)
    }
}
impl PluginAudioProcessorParams for Processor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        self.shared.apply(input);
    }
}
