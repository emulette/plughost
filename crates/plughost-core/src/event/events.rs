//! Events kept as fixed-size records, with their system exclusive bytes in one area, as the
//! helper's shared memory and the native formats keep them.

use std::collections::TryReserveError;
use std::fmt;

use super::{Event, EventData, MAX_BLOCK_EVENTS, MAX_BLOCK_SYSEX_BYTES, Note, NoteExpression};
use crate::InputError;

/// Events in the order they were pushed: fixed records, with the system exclusive bytes of all of
/// them in one area, so a system exclusive message costs no allocation of its own. Iterating
/// gives [`Event`]s that borrow those bytes.
///
/// [`Events::for_block`] holds what one processing block takes, at most [`MAX_BLOCK_EVENTS`]
/// events and [`MAX_BLOCK_SYSEX_BYTES`] system exclusive bytes, with room for both reserved, so
/// pushing never allocates and fails beyond them. [`Events::new`] grows without that limit, for
/// the events of a whole render.
#[derive(Default)]
pub struct Events {
    records: Vec<Record>,
    sysex: Vec<u8>,
    bounded: bool,
}

#[derive(Clone, Copy, Debug)]
struct Record {
    offset: usize,
    port: usize,
    data: RecordData,
}

/// An event's data, with system exclusive bytes as a range of the area.
#[derive(Clone, Copy, Debug)]
enum RecordData {
    Midi([u8; 3]),
    SysEx { start: usize, length: usize },
    NoteOn(Note),
    NoteOff(Note),
    Expression(NoteExpression),
}

impl Events {
    /// Events of any number, which grow as they are pushed: the input or output of a render.
    #[must_use]
    pub fn new() -> Events {
        Events::default()
    }

    /// Room for one block's events and system exclusive bytes, reserved now: pushing within it
    /// never allocates, and beyond it fails.
    #[must_use]
    pub fn for_block() -> Events {
        Events {
            records: Vec::with_capacity(MAX_BLOCK_EVENTS),
            sysex: Vec::with_capacity(MAX_BLOCK_SYSEX_BYTES),
            bounded: true,
        }
    }

    /// As [`Events::for_block`], failing instead of aborting when the room cannot be allocated.
    pub fn try_for_block() -> Result<Events, TryReserveError> {
        let mut records = Vec::new();
        records.try_reserve_exact(MAX_BLOCK_EVENTS)?;
        let mut sysex = Vec::new();
        sysex.try_reserve_exact(MAX_BLOCK_SYSEX_BYTES)?;
        Ok(Events {
            records,
            sysex,
            bounded: true,
        })
    }

    /// Appends `event`, copying its system exclusive bytes. Events of a block go in offset
    /// order; processing refuses others with [`InputError::Events`]. Fails with
    /// [`InputError::EventCapacity`], keeping the events as they are, when the events come from
    /// [`Events::for_block`] and would go beyond one block's.
    pub fn push(&mut self, event: Event<'_>) -> Result<(), InputError> {
        let bytes = match event.data {
            EventData::SysEx(bytes) => bytes.len(),
            _ => 0,
        };
        if self.bounded
            && (self.records.len() >= MAX_BLOCK_EVENTS
                || bytes > MAX_BLOCK_SYSEX_BYTES - self.sysex.len())
        {
            return Err(InputError::EventCapacity);
        }
        self.append(event);
        Ok(())
    }

    /// Appends `event` only when the room already reserved holds it, so that it allocates
    /// nothing; returns whether it did. Internal to the plughost crates; exempt from semver.
    #[doc(hidden)]
    pub fn push_within_capacity(&mut self, event: Event<'_>) -> bool {
        let bytes = match event.data {
            EventData::SysEx(bytes) => bytes.len(),
            _ => 0,
        };
        let fits = self.has_room(bytes);
        if fits {
            self.append(event);
        }
        fits
    }

    /// Appends a system exclusive event of `length` bytes that `fill` writes, for bytes that
    /// cannot be borrowed as a slice, such as memory another process shares. As
    /// [`Events::push_within_capacity`].
    pub(crate) fn push_sysex_within_capacity(
        &mut self,
        offset: usize,
        port: usize,
        length: usize,
        fill: impl FnOnce(&mut [u8]),
    ) -> bool {
        let fits = self.has_room(length);
        if fits {
            let start = self.sysex.len();
            self.sysex.resize(start + length, 0);
            fill(&mut self.sysex[start..]);
            self.records.push(Record {
                offset,
                port,
                data: RecordData::SysEx { start, length },
            });
        }
        fits
    }

    /// Whether the reserved room, and a block's budget when bounded, holds one more event
    /// carrying `bytes` system exclusive bytes.
    fn has_room(&self, bytes: usize) -> bool {
        self.records.len() < self.records.capacity()
            && bytes <= self.sysex.capacity() - self.sysex.len()
            && (!self.bounded
                || (self.records.len() < MAX_BLOCK_EVENTS
                    && bytes <= MAX_BLOCK_SYSEX_BYTES - self.sysex.len()))
    }

    fn append(&mut self, event: Event<'_>) {
        let data = match event.data {
            EventData::Midi(bytes) => RecordData::Midi(bytes),
            EventData::SysEx(bytes) => {
                let start = self.sysex.len();
                self.sysex.extend_from_slice(bytes);
                RecordData::SysEx {
                    start,
                    length: bytes.len(),
                }
            }
            EventData::NoteOn(note) => RecordData::NoteOn(note),
            EventData::NoteOff(note) => RecordData::NoteOff(note),
            EventData::Expression(expression) => RecordData::Expression(expression),
        };
        self.records.push(Record {
            offset: event.offset,
            port: event.port,
            data,
        });
    }

    /// Puts the events in offset order, keeping the order of events at one offset. Events
    /// already in order are left as they are without allocating.
    pub fn sort_by_offset(&mut self) {
        if !self.records.is_sorted_by_key(|record| record.offset) {
            self.records.sort_by_key(|record| record.offset);
        }
    }

    /// Removes every event, keeping the room reserved.
    pub fn clear(&mut self) {
        self.records.clear();
        self.sysex.clear();
    }

    /// How many events there are.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether there are no events.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The system exclusive bytes of all the events.
    pub fn sysex_bytes(&self) -> usize {
        self.sysex.len()
    }

    /// The event at `index`.
    pub fn get(&self, index: usize) -> Option<Event<'_>> {
        self.records.get(index).map(|record| self.event(record))
    }

    /// The events in order.
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            records: self.records.iter(),
            sysex: &self.sysex,
        }
    }

    fn event<'a>(&'a self, record: &Record) -> Event<'a> {
        event(record, &self.sysex)
    }

    /// Appends `other`'s events whatever this one holds. Only for events of any number.
    pub(crate) fn extend_from(&mut self, other: &Events) {
        for event in other {
            self.append(event);
        }
    }

    /// The number of the events from the first whose offset `before` does not take, as for
    /// `slice::partition_point`: with events in offset order, those before an offset.
    pub(crate) fn partition_point(&self, before: impl Fn(usize) -> bool) -> usize {
        self.records.partition_point(|record| before(record.offset))
    }

    /// The system exclusive bytes of the events in `range`.
    pub(crate) fn sysex_bytes_in(&self, range: std::ops::Range<usize>) -> usize {
        self.records[range]
            .iter()
            .map(|record| match record.data {
                RecordData::SysEx { length, .. } => length,
                _ => 0,
            })
            .sum()
    }
}

fn event<'a>(record: &Record, sysex: &'a [u8]) -> Event<'a> {
    let data = match record.data {
        RecordData::Midi(bytes) => EventData::Midi(bytes),
        RecordData::SysEx { start, length } => EventData::SysEx(&sysex[start..start + length]),
        RecordData::NoteOn(note) => EventData::NoteOn(note),
        RecordData::NoteOff(note) => EventData::NoteOff(note),
        RecordData::Expression(expression) => EventData::Expression(expression),
    };
    Event {
        offset: record.offset,
        port: record.port,
        data,
    }
}

impl Clone for Events {
    /// A copy of the events, with one block's room reserved again when they hold one block's.
    fn clone(&self) -> Events {
        let mut copy = if self.bounded {
            Events::for_block()
        } else {
            Events::new()
        };
        copy.records.extend_from_slice(&self.records);
        copy.sysex.extend_from_slice(&self.sysex);
        copy
    }
}

impl PartialEq for Events {
    /// Events are equal when they hold equal events in the same order, whatever their room.
    fn eq(&self, other: &Events) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<'a> PartialEq<[Event<'a>]> for Events {
    fn eq(&self, other: &[Event<'a>]) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter().copied())
    }
}

impl<'a, const N: usize> PartialEq<[Event<'a>; N]> for Events {
    fn eq(&self, other: &[Event<'a>; N]) -> bool {
        *self == other[..]
    }
}

impl<'a> PartialEq<Vec<Event<'a>>> for Events {
    fn eq(&self, other: &Vec<Event<'a>>) -> bool {
        *self == other[..]
    }
}

impl fmt::Debug for Events {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<'a> FromIterator<Event<'a>> for Events {
    /// Events of any number, as [`Events::new`] holds.
    fn from_iter<I: IntoIterator<Item = Event<'a>>>(events: I) -> Events {
        let mut collected = Events::new();
        for event in events {
            collected.append(event);
        }
        collected
    }
}

impl<'a> IntoIterator for &'a Events {
    type Item = Event<'a>;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

/// The events of an [`Events`], in order.
#[derive(Clone, Debug)]
pub struct Iter<'a> {
    records: std::slice::Iter<'a, Record>,
    sysex: &'a [u8],
}

impl<'a> Iterator for Iter<'a> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        self.records.next().map(|record| event(record, self.sysex))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.records.size_hint()
    }

    fn nth(&mut self, n: usize) -> Option<Event<'a>> {
        self.records.nth(n).map(|record| event(record, self.sysex))
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.records
            .next_back()
            .map(|record| event(record, self.sysex))
    }
}

impl ExactSizeIterator for Iter<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_keep_their_system_exclusive_bytes_apart_and_in_order() {
        let mut events = Events::new();
        events.push(Event::note_on(0, 0, 60, 100)).unwrap();
        events
            .push(Event::sysex(3, &[0xF0, 1, 0xF7]).on_port(1))
            .unwrap();
        events.push(Event::sysex(5, &[0xF0, 2, 3, 0xF7])).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events.sysex_bytes(), 7);
        assert_eq!(
            events.iter().collect::<Vec<_>>(),
            [
                Event::note_on(0, 0, 60, 100),
                Event::sysex(3, &[0xF0, 1, 0xF7]).on_port(1),
                Event::sysex(5, &[0xF0, 2, 3, 0xF7]),
            ]
        );
        assert_eq!(events.get(2), Some(Event::sysex(5, &[0xF0, 2, 3, 0xF7])));
        assert_eq!(events, events.iter().collect::<Events>());
        events.clear();
        assert!(events.is_empty() && events.sysex_bytes() == 0);
    }

    #[test]
    fn a_block_takes_its_budget_without_allocating_and_refuses_more() {
        let mut events = Events::for_block();
        let (records, sysex) = (events.records.capacity(), events.sysex.capacity());
        let message = [0xF0, 0x7D, 0xF7];
        for _ in 0..MAX_BLOCK_EVENTS {
            events.push(Event::sysex(0, &message[..])).unwrap();
        }
        assert_eq!(
            events.push(Event::note_on(0, 0, 60, 1)),
            Err(InputError::EventCapacity)
        );
        assert_eq!(events.len(), MAX_BLOCK_EVENTS);
        events.clear();
        let mut large = vec![0; MAX_BLOCK_SYSEX_BYTES];
        (large[0], large[MAX_BLOCK_SYSEX_BYTES - 1]) = (0xF0, 0xF7);
        events.push(Event::sysex(0, &large)).unwrap();
        assert_eq!(
            events.push(Event::sysex(0, &message[..])),
            Err(InputError::EventCapacity)
        );
        assert!(!events.push_within_capacity(Event::sysex(0, &message[..])));
        assert_eq!(
            (events.records.capacity(), events.sysex.capacity()),
            (records, sysex)
        );
        // Unbounded events grow, but push within their room only without allocating.
        let mut events = Events::new();
        assert!(!events.push_within_capacity(Event::note_on(0, 0, 60, 1)));
        events.push(Event::note_on(0, 0, 60, 1)).unwrap();
    }
}
