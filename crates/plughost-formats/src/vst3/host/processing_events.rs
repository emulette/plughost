//! Prepared COM queues share bounded point storage instead of allocating per parameter/block.
use super::*;
use crate::vst3::errors::Vst3Error;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

#[derive(Clone, Copy)]
struct Point {
    queue: usize,
    offset: i32,
    value: f64,
    /// When the point was added, which decides between points at the same offset.
    order: u32,
}
struct Points {
    /// In queue and offset order once sorted.
    values: Vec<Point>,
    limit: usize,
    /// Each queue keeps only its latest point, as JUCE's output queues do, so a plugin that
    /// reports many points (meters) never exhausts the storage.
    latest_only: bool,
    /// False after the host appended points out of order.
    sorted: bool,
    next_order: u32,
}

impl Points {
    /// Puts the points in queue and offset order. Of points at the same offset of a queue, the
    /// one added last stays. The unstable sort does not allocate.
    fn sort(&mut self) {
        if self.sorted {
            return;
        }
        let values = &mut self.values;
        values.sort_unstable_by_key(|point| (point.queue, point.offset, point.order));
        let mut kept = 0;
        for index in 0..values.len() {
            let point = values[index];
            if values
                .get(index + 1)
                .is_none_or(|next| (next.queue, next.offset) != (point.queue, point.offset))
            {
                values[kept] = point;
                kept += 1;
            }
        }
        values.truncate(kept);
        self.sorted = true;
    }

    fn point(&mut self, queue: usize, offset: i32, value: f64) -> Point {
        let order = self.next_order;
        self.next_order += 1;
        Point {
            queue,
            offset,
            value,
            order,
        }
    }
}
struct ParamValueQueue {
    id: AtomicU32,
    slot: usize,
    points: Arc<Mutex<Points>>,
}
impl Class for ParamValueQueue {
    type Interfaces = (IParamValueQueue,);
}
impl IParamValueQueueTrait for ParamValueQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.load(Ordering::Relaxed)
    }
    unsafe fn getPointCount(&self) -> int32 {
        let mut points = lock(&self.points);
        points.sort();
        let start = points
            .values
            .partition_point(|point| point.queue < self.slot);
        let end = points
            .values
            .partition_point(|point| point.queue <= self.slot);
        (end - start) as i32
    }
    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        if index < 0 || offset.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        let mut points = lock(&self.points);
        points.sort();
        let start = points
            .values
            .partition_point(|point| point.queue < self.slot);
        match points.values.get(start + index as usize) {
            Some(point) if point.queue == self.slot => {
                unsafe {
                    *offset = point.offset;
                    *value = point.value;
                }
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn addPoint(&self, offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        let mut points = lock(&self.points);
        points.sort();
        let start = points
            .values
            .partition_point(|point| point.queue < self.slot);
        let at = points
            .values
            .partition_point(|point| (point.queue, point.offset) < (self.slot, offset));
        let latest_only = points.latest_only;
        if latest_only && at > start {
            points.values[start] = points.point(self.slot, offset, value);
            if !index.is_null() {
                unsafe { *index = 0 };
            }
            return kResultOk;
        }
        if let Some(point) = points.values.get_mut(at)
            && point.queue == self.slot
            && (point.offset == offset || latest_only)
        {
            point.offset = offset;
            point.value = value;
        } else {
            if points.values.len() == points.limit {
                return kResultFalse;
            }
            let point = points.point(self.slot, offset, value);
            points.values.insert(at, point);
        }
        if !index.is_null() {
            unsafe {
                *index = (at - start) as i32;
            }
        }
        kResultOk
    }
}

pub(crate) struct ParameterChanges {
    queues: Vec<ComWrapper<ParamValueQueue>>,
    used: Mutex<Used>,
    points: Arc<Mutex<Points>>,
}

/// The queues in use, by parameter.
struct Used {
    count: usize,
    slots: HashMap<ParamID, usize>,
}
impl Class for ParameterChanges {
    type Interfaces = (IParameterChanges,);
}
impl ParameterChanges {
    /// Input changes: up to `queue_limit` parameters and `point_limit` points in all.
    pub fn new(queue_limit: usize, point_limit: usize) -> Result<Self, Vst3Error> {
        Self::with_storage(queue_limit, point_limit, false)
    }

    /// Output changes: the latest point of up to `queue_limit` parameters. Parameters beyond the
    /// limit get no queue, which the plugin sees as a failed `addParameterData`.
    pub fn latest(queue_limit: usize) -> Result<Self, Vst3Error> {
        Self::with_storage(queue_limit, queue_limit, true)
    }

    fn with_storage(
        queue_limit: usize,
        point_limit: usize,
        latest_only: bool,
    ) -> Result<Self, Vst3Error> {
        let mut values = Vec::new();
        values
            .try_reserve_exact(point_limit)
            .map_err(|_| Vst3Error::EventStorage)?;
        let points = Arc::new(Mutex::new(Points {
            values,
            limit: point_limit,
            latest_only,
            sorted: true,
            next_order: 0,
        }));
        let mut slots = HashMap::new();
        slots
            .try_reserve(queue_limit)
            .map_err(|_| Vst3Error::EventStorage)?;
        let mut queues = Vec::new();
        queues
            .try_reserve_exact(queue_limit)
            .map_err(|_| Vst3Error::EventStorage)?;
        for slot in 0..queue_limit {
            queues.push(ComWrapper::new(ParamValueQueue {
                id: AtomicU32::new(0),
                slot,
                points: Arc::clone(&points),
            }));
        }
        Ok(Self {
            queues,
            used: Mutex::new(Used { count: 0, slots }),
            points,
        })
    }
    pub fn clear(&self) {
        let mut used = lock(&self.used);
        used.count = 0;
        used.slots.clear();
        let mut points = lock(&self.points);
        points.values.clear();
        points.sorted = true;
        points.next_order = 0;
    }
    /// Adds the host's point. Points are appended and put in order when first read, so a block's
    /// points cost the same in any order; a point beyond the storage is dropped.
    pub fn push(&self, id: ParamID, offset: i32, value: f64) {
        let Some(slot) = self.slot(id) else {
            return;
        };
        let mut points = lock(&self.points);
        if points.latest_only {
            drop(points);
            unsafe { self.queues[slot].addPoint(offset, value, std::ptr::null_mut()) };
            return;
        }
        if points.values.len() == points.limit {
            return;
        }
        if points
            .values
            .last()
            .is_some_and(|last| (last.queue, last.offset) >= (slot, offset))
        {
            points.sorted = false;
        }
        let point = points.point(slot, offset, value);
        points.values.push(point);
    }
    /// The queue of parameter `id`, taken from the unused ones for a new parameter.
    fn slot(&self, id: ParamID) -> Option<usize> {
        let mut used = lock(&self.used);
        if let Some(&slot) = used.slots.get(&id) {
            return Some(slot);
        }
        if used.count == self.queues.len() {
            return None;
        }
        let slot = used.count;
        self.queues[slot].id.store(id, Ordering::Relaxed);
        used.slots.insert(id, slot);
        used.count += 1;
        Some(slot)
    }
    /// Calls `each` with every queue's parameter and last value.
    pub fn each_last_value(&self, mut each: impl FnMut(ParamID, ParamValue)) {
        let mut points = lock(&self.points);
        points.sort();
        for (index, point) in points.values.iter().enumerate() {
            if points
                .values
                .get(index + 1)
                .is_none_or(|next| next.queue != point.queue)
            {
                each(
                    self.queues[point.queue].id.load(Ordering::Relaxed),
                    point.value,
                );
            }
        }
    }
    pub fn ptr(changes: &ComWrapper<Self>) -> *mut IParameterChanges {
        changes
            .as_com_ref::<IParameterChanges>()
            .map_or(std::ptr::null_mut(), |c| c.as_ptr())
    }
}
impl IParameterChangesTrait for ParameterChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        lock(&self.used).count as i32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        let used = lock(&self.used).count;
        if index < 0 || index as usize >= used {
            return std::ptr::null_mut();
        }
        self.queues[index as usize]
            .as_com_ref::<IParamValueQueue>()
            .map_or(std::ptr::null_mut(), |q| q.as_ptr())
    }
    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        if id.is_null() {
            return std::ptr::null_mut();
        }
        let Some(slot) = self.slot(unsafe { *id }) else {
            return std::ptr::null_mut();
        };
        if !index.is_null() {
            unsafe {
                *index = slot as i32;
            }
        }
        self.queues[slot]
            .as_com_ref::<IParamValueQueue>()
            .map_or(std::ptr::null_mut(), |q| q.as_ptr())
    }
}

/// Input events for one block, which the host keeps within the limit.
pub(crate) struct EventList {
    events: Mutex<Vec<Event>>,
    limit: usize,
}
impl Class for EventList {
    type Interfaces = (IEventList,);
}
impl EventList {
    pub fn new(limit: usize) -> Result<Self, Vst3Error> {
        let mut events = Vec::new();
        events
            .try_reserve_exact(limit)
            .map_err(|_| Vst3Error::EventStorage)?;
        Ok(Self {
            events: Mutex::new(events),
            limit,
        })
    }
    pub fn clear(&self) {
        lock(&self.events).clear();
    }
    pub fn push(&self, mut event: Event) {
        unsafe {
            self.addEvent(&mut event);
        }
    }
    pub fn ptr(list: &ComWrapper<Self>) -> *mut IEventList {
        list.as_com_ref::<IEventList>()
            .map_or(std::ptr::null_mut(), |l| l.as_ptr())
    }
}
impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        lock(&self.events).len() as i32
    }
    unsafe fn getEvent(&self, index: int32, event: *mut Event) -> tresult {
        match lock(&self.events).get(index as usize) {
            Some(value) if !event.is_null() => {
                unsafe {
                    *event = *value;
                }
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn addEvent(&self, event: *mut Event) -> tresult {
        if event.is_null() {
            return kInvalidArgument;
        }
        let mut events = lock(&self.events);
        if events.len() == self.limit {
            return kResultFalse;
        }
        events.push(unsafe { *event });
        kResultOk
    }
}

#[cfg(test)]
#[path = "processing_events_tests.rs"]
mod tests;
