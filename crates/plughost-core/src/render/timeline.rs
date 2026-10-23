use std::collections::BTreeSet;

use crate::{AutomationEvent, InputError, Transport};

/// Replaces the transport at this render-relative sample. `None` withdraws time information.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct TransportChange {
    /// Zero-based sample offset relative to the render or session segment.
    pub offset: usize,
    /// Replacement snapshot, or `None` to withdraw transport information.
    pub transport: Option<Transport>,
}

impl TransportChange {
    /// `transport` from `offset` on; `None` removes it.
    #[must_use]
    pub const fn new(offset: usize, transport: Option<Transport>) -> TransportChange {
        TransportChange { offset, transport }
    }
}

/// Borrowed render-relative transport and automation lanes. The default has no changes.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct RenderSchedule<'a> {
    /// Transport changes in strictly increasing offset order, up to the input end.
    pub transport: &'a [TransportChange],
    /// Point edits in nondecreasing offset order, strictly inside the input.
    pub automation: &'a [AutomationEvent],
    /// Ramps may be unordered, but cannot overlap each other or point edits on the same parameter.
    pub ramps: &'a [AutomationRamp],
}

impl<'a> RenderSchedule<'a> {
    /// Transport changes, in offset order.
    #[must_use]
    pub const fn with_transport(mut self, transport: &'a [TransportChange]) -> RenderSchedule<'a> {
        self.transport = transport;
        self
    }

    /// Automation points, in offset order.
    #[must_use]
    pub const fn with_automation(
        mut self,
        automation: &'a [AutomationEvent],
    ) -> RenderSchedule<'a> {
        self.automation = automation;
        self
    }

    /// Automation ramps.
    #[must_use]
    pub const fn with_ramps(mut self, ramps: &'a [AutomationRamp]) -> RenderSchedule<'a> {
        self.ramps = ramps;
        self
    }
}

/// Linear in normalized value, inclusive endpoints. Expanded into exact sample points for one
/// bounded block at a time. Native parameter metadata quantizes discrete parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct AutomationRamp {
    /// Zero-based plugin slot receiving the ramp.
    pub slot: usize,
    /// Native parameter ID or Audio Unit address.
    pub id: u64,
    /// Inclusive first sample offset; must be less than `end`.
    pub start: usize,
    /// Inclusive last sample offset, strictly before the input end.
    pub end: usize,
    /// Finite normalized value at `start`, in `0..=1`.
    pub from: f64,
    /// Finite normalized value at `end`, in `0..=1`.
    pub to: f64,
}

impl AutomationRamp {
    /// Parameter `id` of the plugin in `slot` going linearly from `from` at `start` to `to` at
    /// `end`, both inclusive.
    #[must_use]
    pub const fn new(
        slot: usize,
        id: u64,
        start: usize,
        end: usize,
        from: f64,
        to: f64,
    ) -> AutomationRamp {
        AutomationRamp {
            slot,
            id,
            start,
            end,
            from,
            to,
        }
    }
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
                .any(|p| !crate::parameter::changes_fit(&[p.change], frames))
        {
            return Err(InputError::Automation);
        }
        for ramp in self.ramps {
            if ramp.start >= ramp.end
                || ramp.end >= frames
                || !ramp.from.is_finite()
                || !ramp.to.is_finite()
                || !(0.0..=1.0).contains(&ramp.from)
                || !(0.0..=1.0).contains(&ramp.to)
            {
                return Err(InputError::Automation);
            }
        }
        if self.ramps.is_empty() {
            return Ok(());
        }
        // Validate each target's intervals in temporal order instead of comparing every pair.
        let mut ramps: Vec<_> = self.ramps.iter().collect();
        ramps.sort_unstable_by_key(|ramp| (ramp.slot, ramp.id, ramp.start));
        if ramps.windows(2).any(|pair| {
            pair[0].slot == pair[1].slot && pair[0].id == pair[1].id && pair[0].end >= pair[1].start
        }) {
            return Err(InputError::Automation);
        }
        let mut points: Vec<_> = self.automation.iter().collect();
        points.sort_unstable_by_key(|point| (point.slot, point.change.id, point.change.offset));
        for ramp in ramps {
            let at = points.partition_point(|point| {
                (point.slot, point.change.id, point.change.offset)
                    < (ramp.slot, ramp.id, ramp.start)
            });
            if points.get(at).is_some_and(|point| {
                point.slot == ramp.slot
                    && point.change.id == ramp.id
                    && point.change.offset <= ramp.end
            }) {
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
}

/// Ramp orders and timestamps a timeline builds, kept so a session reuses their storage.
#[derive(Default)]
pub(super) struct Storage {
    starts: Vec<usize>,
    ends: Vec<usize>,
    times: Vec<usize>,
}

/// Forward-only scheduling state for one render. The active set follows the caller's ramp
/// order, including when input ramps are not sorted by start time.
pub(super) struct Timeline<'a> {
    automation: &'a [AutomationEvent],
    ramps: &'a [AutomationRamp],
    storage: &'a mut Storage,
    next_start: usize,
    next_end: usize,
    active: BTreeSet<usize>,
}

impl<'a> Timeline<'a> {
    pub(super) fn new(schedule: RenderSchedule<'a>, storage: &'a mut Storage) -> Self {
        storage.starts.clear();
        storage.starts.extend(0..schedule.ramps.len());
        storage
            .starts
            .sort_unstable_by_key(|&index| schedule.ramps[index].start);
        storage.ends.clear();
        storage.ends.extend(0..schedule.ramps.len());
        storage
            .ends
            .sort_unstable_by_key(|&index| schedule.ramps[index].end);
        Self {
            automation: schedule.automation,
            ramps: schedule.ramps,
            storage,
            next_start: 0,
            next_end: 0,
            active: BTreeSet::new(),
        }
    }

    fn advance(&mut self, time: usize) {
        while let Some(&index) = self.storage.starts.get(self.next_start) {
            if self.ramps[index].start > time {
                break;
            }
            self.active.insert(index);
            self.next_start += 1;
        }
        while let Some(&index) = self.storage.ends.get(self.next_end) {
            if self.ramps[index].end >= time {
                break;
            }
            self.active.remove(&index);
            self.next_end += 1;
        }
    }

    /// A timestamp is indivisible: reject an impossible group before rendering earlier audio.
    pub(super) fn validate_capacity(&mut self, midi: &crate::Events) -> Result<(), InputError> {
        let Storage {
            starts,
            ends,
            times,
        } = &mut *self.storage;
        times.clear();
        times.extend(
            self.automation
                .iter()
                .map(|point| point.change.offset)
                .chain(midi.iter().map(|event| event.offset))
                .chain(self.ramps.iter().map(|ramp| ramp.start)),
        );
        times.sort_unstable();
        times.dedup();
        for &time in times.iter() {
            let points = self.automation.partition_point(|p| p.change.offset <= time)
                - self.automation.partition_point(|p| p.change.offset < time);
            let notes = midi.partition_point(|offset| offset < time)
                ..midi.partition_point(|offset| offset <= time);
            let ramps = starts.partition_point(|&index| self.ramps[index].start <= time)
                - ends.partition_point(|&index| self.ramps[index].end < time);
            crate::event::validate_event_count(points, notes.len())?;
            if midi.sysex_bytes_in(notes.clone()) > crate::MAX_BLOCK_SYSEX_BYTES {
                return Err(InputError::EventCapacity);
            }
            crate::event::validate_event_count(points + notes.len(), ramps)?;
        }
        Ok(())
    }

    /// Fills a bounded block, shortening it before an entire timestamp would exceed the count or
    /// system exclusive budget. MIDI from index `pending` on is counted here and copied by the
    /// render loop after the final end is known.
    pub(super) fn fill_block(
        &mut self,
        start: usize,
        end: usize,
        midi: &crate::Events,
        pending: usize,
        output: &mut Vec<AutomationEvent>,
    ) -> usize {
        output.clear();
        let mut point = self.automation.partition_point(|p| p.change.offset < start);
        let mut note = pending;
        let mut sysex = 0;
        let mut cursor = start;
        while cursor < end {
            self.advance(cursor);
            let next_ramp = if self.active.is_empty() {
                self.storage
                    .starts
                    .get(self.next_start)
                    .map(|&index| self.ramps[index].start)
            } else {
                Some(cursor)
            };
            let next = self
                .automation
                .get(point)
                .map(|p| p.change.offset)
                .into_iter()
                .chain(midi.get(note).map(|event| event.offset))
                .chain(next_ramp)
                .min();
            let Some(time) = next.filter(|&time| time < end) else {
                break;
            };
            let point_end = self.automation.partition_point(|p| p.change.offset <= time);
            let note_end = midi.partition_point(|offset| offset <= time);
            self.advance(time);
            let added = point_end - point + self.active.len();
            let added_sysex = midi.sysex_bytes_in(note..note_end);
            if output.len() + added + note_end - pending > crate::MAX_BLOCK_EVENTS
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
            output.extend(
                self.active
                    .iter()
                    .map(|&index| &self.ramps[index])
                    .map(|ramp| AutomationEvent {
                        slot: ramp.slot,
                        change: crate::ParameterChange {
                            id: ramp.id,
                            offset: time - start,
                            value: if time == ramp.end {
                                ramp.to
                            } else {
                                (ramp.from
                                    + (ramp.to - ramp.from) * (time - ramp.start) as f64
                                        / (ramp.end - ramp.start) as f64)
                                    .clamp(ramp.from.min(ramp.to), ramp.from.max(ramp.to))
                            },
                        },
                    }),
            );
            point = point_end;
            note = note_end;
            cursor = time + 1;
        }
        end
    }
}
