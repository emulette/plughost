//! Calls a [`BlockProcessor`] in the precision of the sample type a generic test uses.
use plughost_core::{BlockContext, MidiEvent, ParameterChange, Sample};
use plughost_formats::{BlockProcessor, Error};

pub trait ProcessSample: Sample {
    fn process(
        processor: &dyn BlockProcessor,
        context: &BlockContext,
        input: &[&[Self]],
        output: &mut [&mut [Self]],
        automation: &[ParameterChange],
        events: &[MidiEvent],
    ) -> Result<(), Error>;
}

impl ProcessSample for f32 {
    fn process(
        processor: &dyn BlockProcessor,
        context: &BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[ParameterChange],
        events: &[MidiEvent],
    ) -> Result<(), Error> {
        processor.process_audio_f32(context, input, output, automation, events, &mut Vec::new())
    }
}

impl ProcessSample for f64 {
    fn process(
        processor: &dyn BlockProcessor,
        context: &BlockContext,
        input: &[&[f64]],
        output: &mut [&mut [f64]],
        automation: &[ParameterChange],
        events: &[MidiEvent],
    ) -> Result<(), Error> {
        processor.process_audio_f64(context, input, output, automation, events, &mut Vec::new())
    }
}
