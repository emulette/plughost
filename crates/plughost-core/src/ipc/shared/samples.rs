use super::*;
use std::ptr;

// Closed to external implementations: sample format tags alone cannot justify a pointer cast.
trait WireSample: Copy {
    const FORMAT: SampleFormat;
}
impl WireSample for f32 {
    const FORMAT: SampleFormat = SampleFormat::F32;
}
impl WireSample for f64 {
    const FORMAT: SampleFormat = SampleFormat::F64;
}

impl SharedAudio {
    /// The start of `frames` samples of one channel in a PCM slot, checked against the mapping.
    fn channel<T: WireSample>(&self, slot: usize, channel: usize, frames: usize) -> *mut T {
        let stride = self.descriptor.config.max_frames * size_of::<T>();
        let offset = slot + channel * stride;
        assert!(
            frames <= self.descriptor.config.max_frames && offset + stride <= self.layout.bytes
        );
        // In bounds by the assertion above. Slots start 8-byte aligned in the page-aligned
        // mapping and the stride is a whole number of samples, so the pointer is aligned for T.
        self.mapping.as_mut_ptr().wrapping_add(offset).cast()
    }

    fn write_pcm<T: WireSample>(&self, slot: usize, channels: &[&[T]]) {
        for (index, source) in channels.iter().enumerate() {
            let destination = self.channel::<T>(slot, index, source.len());
            // SAFETY: `channel` checked the range. The protocol gives one side a slot at a time:
            // the writer owns it until the socket message that hands it over, and the reader only
            // accesses it after receiving that message, so plain copies never race. No Rust
            // references point into the mapping, and every bit pattern is a valid float.
            unsafe { ptr::copy_nonoverlapping(source.as_ptr(), destination, source.len()) };
        }
    }

    fn read_pcm<T: WireSample>(
        &self,
        slot: usize,
        count: usize,
        frames: usize,
        scratch: &mut [Vec<T>],
    ) -> io::Result<()> {
        if self.descriptor.config.sample_format != T::FORMAT
            || scratch.len() != count
            || scratch.iter().any(|channel| channel.capacity() < frames)
        {
            return Err(invalid(BLOCK));
        }
        for (index, destination) in scratch.iter_mut().enumerate() {
            let source = self.channel::<T>(slot, index, frames);
            // SAFETY: as in `write_pcm`; the capacity check above covers `frames` elements, all
            // of which are initialized by the copy before the length is set.
            unsafe {
                ptr::copy_nonoverlapping(source.cast_const(), destination.as_mut_ptr(), frames);
                destination.set_len(frames);
            }
        }
        Ok(())
    }

    fn check_source<T: WireSample>(
        &self,
        channels: &[&[T]],
        count: usize,
        frames: usize,
    ) -> io::Result<()> {
        if self.descriptor.config.sample_format != T::FORMAT
            || frames > self.descriptor.config.max_frames
            || channels.len() != count
            || channels.iter().any(|channel| channel.len() != frames)
        {
            return Err(invalid(BLOCK));
        }
        Ok(())
    }

    fn write_input<T: WireSample>(
        &mut self,
        sequence: u64,
        frames: usize,
        input: &[&[T]],
        automation: &[AutomationEvent],
        midi: &[MidiEvent],
    ) -> io::Result<Submission> {
        self.check_source(input, self.descriptor.config.input_channels, frames)?;
        if crate::validate_event_budget(automation.len(), midi).is_err() {
            return Err(invalid(BLOCK));
        }
        self.write_pcm(self.layout.input, input);
        self.write_automation(automation);
        self.write_midi(self.layout.input_events, midi);
        Ok(Submission {
            generation: self.descriptor.generation,
            sequence,
            frames,
            automation: automation.len(),
            midi: midi.len(),
        })
    }

    fn read_input<T: WireSample>(
        &mut self,
        submission: Submission,
        scratch: &mut [Vec<T>],
        automation: &mut Vec<AutomationEvent>,
        midi: &mut Vec<MidiEvent>,
    ) -> io::Result<()> {
        self.check_submission(submission)?;
        self.read_pcm(
            self.layout.input,
            self.descriptor.config.input_channels,
            submission.frames,
            scratch,
        )?;
        self.read_automation(submission.automation, automation)?;
        self.read_midi(self.layout.input_events, submission.midi, midi)
    }

    fn write_output<T: WireSample>(
        &mut self,
        submission: Submission,
        output: &[&[T]],
        events: &[MidiEvent],
    ) -> io::Result<()> {
        self.check_submission(submission)?;
        self.check_source(
            output,
            self.descriptor.config.output_channels,
            submission.frames,
        )?;
        if crate::validate_event_budget(0, events).is_err() {
            return Err(invalid(BLOCK));
        }
        self.write_pcm(self.layout.output, output);
        self.write_midi(self.layout.output_events, events);
        Ok(())
    }

    fn read_output<T: WireSample>(
        &mut self,
        submission: Submission,
        output: &mut [&mut [T]],
        event_count: usize,
        events: &mut Vec<MidiEvent>,
    ) -> io::Result<()> {
        self.check_submission(submission)?;
        self.read_midi(self.layout.output_events, event_count, events)?;
        if self.descriptor.config.sample_format != T::FORMAT
            || output.len() != self.descriptor.config.output_channels
            || output
                .iter()
                .any(|channel| channel.len() != submission.frames)
        {
            return Err(invalid(BLOCK));
        }
        for (index, destination) in output.iter_mut().enumerate() {
            let source = self.channel::<T>(self.layout.output, index, destination.len());
            // SAFETY: as in `write_pcm`; `channel` checked the range of `destination.len()` samples.
            unsafe {
                ptr::copy_nonoverlapping(
                    source.cast_const(),
                    destination.as_mut_ptr(),
                    destination.len(),
                )
            };
        }
        Ok(())
    }
}

macro_rules! samples {
    ($sample:ty, $write_input:ident, $read_input:ident, $write_output:ident, $read_output:ident) => {
        impl SharedAudio {
            /// Writes one input slot. The connection must finish or reject the previous request
            /// before reusing this slot, and sends the returned submission to the peer.
            pub fn $write_input(
                &mut self,
                sequence: u64,
                frames: usize,
                input: &[&[$sample]],
                automation: &[AutomationEvent],
                midi: &[MidiEvent],
            ) -> io::Result<Submission> {
                self.write_input(sequence, frames, input, automation, midi)
            }
            /// Copies a received input into preallocated local storage. Events are decoded, not
            /// validated. Scratch may be modified on error and must not be passed to DSP unless
            /// this succeeds.
            pub fn $read_input(
                &mut self,
                submission: Submission,
                scratch: &mut [Vec<$sample>],
                automation: &mut Vec<AutomationEvent>,
                midi: &mut Vec<MidiEvent>,
            ) -> io::Result<()> {
                self.read_input(submission, scratch, automation, midi)
            }
            /// Writes the complete output and output events for a received submission before
            /// the peer is answered; the answer carries the event count.
            pub fn $write_output(
                &mut self,
                submission: Submission,
                output: &[&[$sample]],
                events: &[MidiEvent],
            ) -> io::Result<()> {
                self.write_output(submission, output, events)
            }
            /// Copies a complete output into one buffer of `submission.frames` samples per output
            /// channel, and `event_count` output events into `events`, whose capacity must hold
            /// them. The audio buffers are unchanged on error.
            pub fn $read_output(
                &mut self,
                submission: Submission,
                output: &mut [&mut [$sample]],
                event_count: usize,
                events: &mut Vec<MidiEvent>,
            ) -> io::Result<()> {
                self.read_output(submission, output, event_count, events)
            }
        }
    };
}
samples!(
    f32,
    write_input_f32,
    read_input_f32,
    write_output_f32,
    read_output_f32
);
samples!(
    f64,
    write_input_f64,
    read_input_f64,
    write_output_f64,
    read_output_f64
);
