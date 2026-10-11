//! Fuzzes bytes written by a helper before the real shared-output validation/copy boundary.
use super::*;

const FRAMES: usize = 8;
const HEADER: usize = 40;
const PCM_BYTES: usize = 128;
const RECORD_BYTES: usize = 64 * EVENT_BYTES;

/// Input: five little-endian words (precision, generation, frames, event count, port count),
/// 128 PCM bytes, 64 raw event records, then the SysEx arena. Missing payload bytes are zero.
/// Invalid output must leave caller PCM untouched in both sample precisions.
pub fn output(bytes: &[u8]) -> bool {
    if bytes.len() < HEADER {
        return false;
    }
    let word = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    let format = if word(0) & 1 == 0 {
        SampleFormat::F32
    } else {
        SampleFormat::F64
    };
    let descriptor = Descriptor {
        generation: 7,
        config: SlotConfig {
            sample_format: format,
            max_frames: FRAMES,
            input_channels: 0,
            output_channels: 2,
        },
    };
    let mut memory = SharedAudio::new(descriptor).unwrap();
    let submission = Submission {
        generation: word(8),
        sequence: 1,
        frames: word(16) as usize,
        automation: 0,
        events: 0,
    };
    let count = word(24) as usize;
    let ports = word(32) as usize;
    let payload = &bytes[HEADER..];
    let mut pcm = [0u8; PCM_BYTES];
    let copied = payload.len().min(PCM_BYTES);
    pcm[..copied].copy_from_slice(&payload[..copied]);
    let width = if format == SampleFormat::F32 { 4 } else { 8 };
    // SAFETY: the harness is the sole owner of this mapping. Each copy is bounded by the
    // production layout's allocated region; no references or concurrent native writers exist.
    unsafe {
        std::ptr::copy_nonoverlapping(
            pcm.as_ptr(),
            memory.memory.as_ptr().add(memory.layout.output),
            2 * FRAMES * width,
        );
        if let Some(records) = payload.get(PCM_BYTES..) {
            let copied = records.len().min(RECORD_BYTES);
            std::ptr::copy_nonoverlapping(
                records.as_ptr(),
                memory
                    .memory
                    .as_ptr()
                    .add(memory.layout.output_events.slots),
                copied,
            );
        }
        if let Some(sysex) = payload.get(PCM_BYTES + RECORD_BYTES..) {
            let copied = sysex.len().min(MAX_BLOCK_SYSEX_BYTES);
            std::ptr::copy_nonoverlapping(
                sysex.as_ptr(),
                memory
                    .memory
                    .as_ptr()
                    .add(memory.layout.output_events.sysex),
                copied,
            );
        }
    }
    let frames = submission.frames.min(FRAMES);
    let mut events = Vec::with_capacity(MAX_BLOCK_EVENTS);
    macro_rules! read {
        ($sample:ty, $method:ident) => {{
            let sentinel: $sample = -123.25;
            let mut left = [sentinel; FRAMES];
            let mut right = [sentinel; FRAMES];
            let result = memory.$method(
                submission,
                &mut [&mut left[..frames], &mut right[..frames]],
                count,
                ports,
                &mut events,
            );
            for (channel, output) in [left, right].iter().enumerate() {
                for (frame, &sample) in output.iter().enumerate() {
                    let expected = if result.is_ok() && frame < frames {
                        let at = (channel * FRAMES + frame) * width;
                        <$sample>::from_ne_bytes(pcm[at..at + width].try_into().unwrap())
                    } else {
                        sentinel
                    };
                    assert_eq!(sample.to_bits(), expected.to_bits());
                }
            }
            result.is_ok()
        }};
    }
    let accepted = match format {
        SampleFormat::F32 => read!(f32, read_output_f32),
        SampleFormat::F64 => read!(f64, read_output_f64),
    };
    if accepted {
        assert_eq!(events.len(), count);
        assert!(crate::events_fit(&events, frames));
        assert!(events.iter().all(|event| event.port < ports));
        assert!(crate::validate_event_budget(0, &events).is_ok());
    }
    accepted
}

#[cfg(test)]
mod tests {
    use super::output;

    #[test]
    fn corpus_helper_output_checks_raw_events_before_copying_pcm() {
        for bytes in [
            &include_bytes!("../corpus/shared_output/f32-midi")[..],
            include_bytes!("../corpus/shared_output/f64-note"),
            include_bytes!("../corpus/shared_output/sysex"),
        ] {
            assert!(output(bytes));
        }
        for bytes in [
            &include_bytes!("../corpus/shared_output/wrong-generation")[..],
            include_bytes!("../corpus/shared_output/excessive-frames"),
            include_bytes!("../corpus/shared_output/excessive-events"),
            include_bytes!("../corpus/shared_output/unknown-kind"),
            include_bytes!("../corpus/shared_output/late-event"),
            include_bytes!("../corpus/shared_output/invalid-port"),
            include_bytes!("../corpus/shared_output/invalid-note-id"),
            include_bytes!("../corpus/shared_output/nonfinite-velocity"),
            include_bytes!("../corpus/shared_output/sysex-outside"),
        ] {
            assert!(!output(bytes));
        }
    }
}
