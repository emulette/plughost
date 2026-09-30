//! Prepared COM queues share bounded point storage instead of allocating per parameter/block.
use super::*;
use crate::vst3::errors::Vst3Error;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

#[derive(Clone, Copy)]
struct Point {
    queue: usize,
    offset: i32,
    value: f64,
}
struct Points {
    values: Vec<Point>,
    limit: usize,
    /// Each queue keeps only its latest point, as JUCE's output queues do, so a plugin that
    /// reports many points (meters) never exhausts the storage.
    latest_only: bool,
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
        let points = lock(&self.points);
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
        let points = lock(&self.points);
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
        let start = points
            .values
            .partition_point(|point| point.queue < self.slot);
        let at = points
            .values
            .partition_point(|point| (point.queue, point.offset) < (self.slot, offset));
        let latest_only = points.latest_only;
        if latest_only && at > start {
            points.values[start] = Point {
                queue: self.slot,
                offset,
                value,
            };
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
            points.values.insert(
                at,
                Point {
                    queue: self.slot,
                    offset,
                    value,
                },
            );
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
    used: Mutex<usize>,
    points: Arc<Mutex<Points>>,
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
        }));
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
            used: Mutex::new(0),
            points,
        })
    }
    pub fn clear(&self) {
        *lock(&self.used) = 0;
        lock(&self.points).values.clear();
    }
    pub fn push(&self, id: ParamID, offset: i32, value: f64) {
        let queue = unsafe { self.addParameterData(&id, std::ptr::null_mut()) };
        if let Some(queue) = unsafe { vst3::ComRef::from_raw(queue) } {
            unsafe {
                queue.addPoint(offset, value, std::ptr::null_mut());
            }
        }
    }
    /// Calls `each` with every queue's parameter and last value.
    pub fn each_last_value(&self, mut each: impl FnMut(ParamID, ParamValue)) {
        let points = lock(&self.points);
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
        *lock(&self.used) as i32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        let used = *lock(&self.used);
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
        let id = unsafe { *id };
        let mut used = lock(&self.used);
        let slot = if let Some(slot) = self.queues[..*used]
            .iter()
            .position(|queue| queue.id.load(Ordering::Relaxed) == id)
        {
            slot
        } else {
            if *used == self.queues.len() {
                return std::ptr::null_mut();
            }
            let slot = *used;
            self.queues[slot].id.store(id, Ordering::Relaxed);
            *used += 1;
            slot
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
