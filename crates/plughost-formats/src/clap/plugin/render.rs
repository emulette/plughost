use super::{ClapError, Plugin, lock};
use plughost_core::render::{Process, Tail};
use plughost_core::{AudioDirection, AutomationEvent, BlockContext, Event, InputError, Sample};
impl<S: Sample> Process<S> for Plugin {
    type Error = ClapError;
    fn sample_rate(&self) -> f64 {
        self.config().map_or(0.0, |c| c.sample_rate)
    }
    fn max_block_size(&self) -> usize {
        self.config().map_or(0, |c| c.max_block_size)
    }
    fn input_channels(&self) -> usize {
        channel_count(self, AudioDirection::Input)
    }
    fn output_channels(&self) -> usize {
        channel_count(self, AudioDirection::Output)
    }
    fn latency(&self) -> Result<u32, ClapError> {
        Plugin::latency(self)
    }
    fn tail(&self) -> Result<Tail, ClapError> {
        Plugin::tail(self)
    }
    fn validate_automation(&mut self, automation: &[AutomationEvent]) -> Result<(), ClapError> {
        let parameters = self.parameter_cache();
        for event in automation {
            if event.slot != 0 {
                return Err(ClapError::Input(InputError::Slot));
            }
            let parameter = parameters.get(event.change.id).ok_or(ClapError::Input(
                InputError::UnknownParameter {
                    id: event.change.id,
                },
            ))?;
            parameter
                .info
                .automation_value(event.change.value)
                .map_err(ClapError::Input)?;
        }
        Ok(())
    }
    fn process(
        &mut self,
        context: &BlockContext,
        input: &[&[S]],
        output: &mut [&mut [S]],
        automation: &[AutomationEvent],
        events: &[Event],
        produced: &mut Vec<Event>,
    ) -> Result<(), ClapError> {
        if automation.iter().any(|event| event.slot != 0) {
            return Err(ClapError::Input(InputError::Slot));
        }
        let changes: Vec<_> = automation.iter().map(|event| event.change).collect();
        self.processor()
            .process(context, input, output, &changes, events, produced)
    }
}
fn channel_count(plugin: &Plugin, direction: AudioDirection) -> usize {
    let engine = lock(&plugin.engine);
    let Some(prepared) = &engine.prepared else {
        return 0;
    };
    if prepared.audio_config.is_some() {
        prepared
            .buses
            .iter()
            .filter(|b| b.direction == direction && b.active == Some(true))
            .map(|b| b.channels as usize)
            .sum()
    } else {
        match direction {
            AudioDirection::Input => prepared.main_input.map_or(0, |i| prepared.inputs[i]),
            AudioDirection::Output => prepared.main_output.map_or(0, |i| prepared.outputs[i]),
        }
    }
}
