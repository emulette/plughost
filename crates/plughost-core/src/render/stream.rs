use std::ops::Range;

use super::Delivery;
use crate::{MidiEvent, Sample};

/// A bounded output stage. Only the unconfirmed silence hold is retained between blocks.
pub(super) struct Output<S> {
    quiet: Vec<Vec<S>>,
    ready: Vec<Vec<S>>,
    silence: Option<(f64, usize)>,
    quiet_frames: usize,
    pub emitted: usize,
}

impl<S: Sample> Output<S> {
    pub fn new(channels: usize, silence: Option<(f64, usize)>) -> Self {
        Self {
            quiet: vec![Vec::new(); channels],
            ready: vec![Vec::new(); channels],
            silence,
            quiet_frames: 0,
            emitted: 0,
        }
    }

    /// Returns true when the silence hold has elapsed. `start` is the raw processing offset.
    pub fn accept(
        &mut self,
        block: &[Vec<S>],
        count: usize,
        start: usize,
        latency: usize,
        body: usize,
    ) -> bool {
        let first = latency.saturating_sub(start).min(count);
        let Some((threshold, hold)) = self.silence else {
            self.emit(block, first..count);
            return false;
        };
        // Output before the end of the body is never trimmed, so nothing is held yet.
        let tail = body.saturating_sub(start).clamp(first, count);
        self.emit(block, first..tail);
        // At least one quiet frame is held before the hold can elapse.
        let hold = hold.max(1);
        let quiet = |index: usize| {
            block
                .iter()
                .all(|channel| channel[index].to_f64().abs() <= threshold)
        };
        let mut index = tail;
        while index < count {
            let silent = quiet(index);
            let run = index
                + 1
                + (index + 1..count)
                    .take_while(|&i| quiet(i) == silent)
                    .count();
            if silent {
                let end = run.min(index + hold - self.quiet_frames);
                for (held, channel) in self.quiet.iter_mut().zip(block) {
                    held.extend_from_slice(&channel[index..end]);
                }
                self.quiet_frames += end - index;
                if self.quiet_frames >= hold {
                    return true;
                }
            } else {
                self.emit(block, index..run);
            }
            index = run;
        }
        false
    }

    /// Emits held quiet frames followed by the non-empty `range` of loud frames.
    fn emit(&mut self, block: &[Vec<S>], range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        for ((ready, held), channel) in self.ready.iter_mut().zip(&mut self.quiet).zip(block) {
            ready.append(held);
            ready.extend_from_slice(&channel[range.clone()]);
        }
        self.emitted += self.quiet_frames + range.len();
        self.quiet_frames = 0;
    }

    /// Delivers the ready audio with `events`, when either has something, and clears both.
    pub fn deliver(
        &mut self,
        events: &mut Vec<MidiEvent>,
        consume: &mut impl FnMut(Delivery<'_, S>),
    ) {
        if self.ready.iter().any(|channel| !channel.is_empty()) || !events.is_empty() {
            consume(Delivery {
                audio: &self.ready.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                events,
            });
            for channel in &mut self.ready {
                channel.clear();
            }
            events.clear();
        }
    }
}
