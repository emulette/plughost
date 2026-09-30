use crate::{AutomationEvent, InputError, Transport};

/// Replaces the transport at this render-relative sample. `None` withdraws time information.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportChange {
    pub offset: usize,
    pub transport: Option<Transport>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RenderSchedule<'a> {
    pub transport: &'a [TransportChange],
    pub automation: &'a [AutomationEvent],
    pub ramps: &'a [AutomationRamp],
}

/// Linear in normalized value, inclusive endpoints. Expanded into exact sample points for one
/// bounded block at a time. Native parameter metadata quantizes discrete parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutomationRamp {
    pub slot: usize,
    pub id: u64,
    pub start: usize,
    pub end: usize,
    pub from: f64,
    pub to: f64,
}

impl RenderSchedule<'_> {
    pub(super) fn validate(&self, frames: usize, end: usize, rate: f64) -> Result<(), InputError> {
        if self
            .transport
            .windows(2)
            .any(|p| p[0].offset >= p[1].offset)
            || self.transport.iter().any(|p| p.offset > frames)
        {
            return Err(InputError::Transport);
        }
        for (index, point) in self.transport.iter().enumerate() {
            if let Some(value) = point.transport {
                let until = self.transport.get(index + 1).map_or(end, |p| p.offset);
                value.advanced(until - point.offset, rate)?;
            }
        }
        if self
            .automation
            .windows(2)
            .any(|p| p[0].change.offset > p[1].change.offset)
            || self
                .automation
                .iter()
                .any(|p| !crate::changes_fit(&[p.change], frames))
        {
            return Err(InputError::Automation);
        }
        for (index, ramp) in self.ramps.iter().enumerate() {
            if ramp.start >= ramp.end
                || ramp.end >= frames
                || !ramp.from.is_finite()
                || !ramp.to.is_finite()
                || !(0.0..=1.0).contains(&ramp.from)
                || !(0.0..=1.0).contains(&ramp.to)
                || self.automation.iter().any(|point| {
                    point.slot == ramp.slot
                        && point.change.id == ramp.id
                        && (ramp.start..=ramp.end).contains(&point.change.offset)
                })
                || self.ramps[..index].iter().any(|other| {
                    other.slot == ramp.slot
                        && other.id == ramp.id
                        && other.start <= ramp.end
                        && ramp.start <= other.end
                })
            {
                return Err(InputError::Automation);
            }
        }
        Ok(())
    }

    pub(super) fn transport_at(
        &self,
        offset: usize,
        rate: f64,
    ) -> Result<Option<Transport>, InputError> {
        let count = self
            .transport
            .partition_point(|point| point.offset <= offset);
        count
            .checked_sub(1)
            .and_then(|index| {
                let point = self.transport[index];
                point
                    .transport
                    .map(|value| value.advanced(offset - point.offset, rate))
            })
            .transpose()
    }

    pub(super) fn next_transport(&self, offset: usize) -> Option<usize> {
        self.transport
            .get(
                self.transport
                    .partition_point(|point| point.offset <= offset),
            )
            .map(|point| point.offset)
    }

    /// A timestamp is indivisible: reject an impossible group before rendering earlier audio.
    pub(super) fn validate_capacity(&self, midi: &[crate::MidiEvent]) -> Result<(), InputError> {
        let mut times: Vec<_> = self
            .automation
            .iter()
            .map(|point| point.change.offset)
            .chain(midi.iter().map(|event| event.offset))
            .chain(self.ramps.iter().map(|ramp| ramp.start))
            .collect();
        times.sort_unstable();
        times.dedup();
        for time in times {
            let points = self.automation.partition_point(|p| p.change.offset <= time)
                - self.automation.partition_point(|p| p.change.offset < time);
            let notes = &midi[midi.partition_point(|e| e.offset < time)
                ..midi.partition_point(|e| e.offset <= time)];
            let ramps = self
                .ramps
                .iter()
                .filter(|r| (r.start..=r.end).contains(&time))
                .count();
            crate::validate_event_budget(points, notes)?;
            crate::validate_event_count(points + notes.len(), ramps)?;
        }
        Ok(())
    }

    /// Fills a bounded block, shortening it before an entire timestamp would exceed the count or
    /// system exclusive budget. MIDI is counted here and copied by the render loop after the final
    /// end is known.
    pub(super) fn fill_block(
        &self,
        start: usize,
        end: usize,
        midi: &[crate::MidiEvent],
        output: &mut Vec<AutomationEvent>,
    ) -> usize {
        output.clear();
        let mut point = self.automation.partition_point(|p| p.change.offset < start);
        let mut note = 0;
        let mut sysex = 0;
        let mut cursor = start;
        while cursor < end {
            let next = self
                .automation
                .get(point)
                .map(|p| p.change.offset)
                .into_iter()
                .chain(midi.get(note).map(|e| e.offset))
                .chain(
                    self.ramps
                        .iter()
                        .filter_map(|ramp| (cursor <= ramp.end).then_some(cursor.max(ramp.start))),
                )
                .min();
            let Some(time) = next.filter(|&time| time < end) else {
                break;
            };
            let point_end = self.automation.partition_point(|p| p.change.offset <= time);
            let note_end = midi.partition_point(|e| e.offset <= time);
            let active = || {
                self.ramps
                    .iter()
                    .filter(|r| (r.start..=r.end).contains(&time))
            };
            let added = point_end - point + active().count();
            let added_sysex = crate::sysex_bytes(&midi[note..note_end]);
            if output.len() + added + note_end > crate::MAX_BLOCK_EVENTS
                || sysex + added_sysex > crate::MAX_BLOCK_SYSEX_BYTES
            {
                return time;
            }
            sysex += added_sysex;
            output.extend(
                self.automation[point..point_end]
                    .iter()
                    .map(|point| AutomationEvent {
                        slot: point.slot,
                        change: crate::ParameterChange {
                            offset: time - start,
                            ..point.change
                        },
                    }),
            );
            output.extend(active().map(|ramp| AutomationEvent {
                slot: ramp.slot,
                change: crate::ParameterChange {
                    id: ramp.id,
                    offset: time - start,
                    value: ramp.from
                        + (ramp.to - ramp.from) * (time - ramp.start) as f64
                            / (ramp.end - ramp.start) as f64,
                },
            }));
            point = point_end;
            note = note_end;
            cursor = time + 1;
        }
        end
    }
}
