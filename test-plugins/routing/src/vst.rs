#![allow(non_snake_case)]
#![allow(clippy::unnecessary_cast)]
use std::ffi::{c_char, c_void};
use std::sync::{Mutex, MutexGuard};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComRef, ComWrapper, Interface, uid};
#[path = "vst_audio.rs"]
mod audio;
#[path = "vst_connection.rs"]
mod connection;
#[path = "vst_controller.rs"]
mod controller;
#[path = "vst_events.rs"]
mod events;
#[path = "factory.rs"]
mod factory;
const CID: TUID = uid(0x706c7567, 0x686f7374, 0x526f7574, 0x696e6701);
const CONTROLLER_CID: TUID = uid(0x706c7567, 0x686f7374, 0x526f7574, 0x696e6702);
const NAME: &str = "plughost test routing";
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn copy_c(s: &str, out: &mut [c_char]) {
    for (b, o) in s.bytes().chain([0]).zip(out) {
        *o = b as c_char;
    }
}
fn copy_wide(s: &str, out: &mut [TChar]) {
    for (b, o) in s.encode_utf16().chain([0]).zip(out) {
        *o = b;
    }
}
struct Routing {
    connection: connection::Connection,
    values: Mutex<[f64; 2]>,
    /// The points the last block with audio brought for the gain, reported as parameter
    /// `GAIN_POINTS` in hundredths.
    gain_points: Mutex<usize>,
    arrangement: Mutex<SpeakerArrangement>,
    active: Mutex<[[bool; 2]; 2]>,
}
impl Routing {
    fn new() -> Self {
        Self {
            connection: connection::Connection::default(),
            values: Mutex::new([1.0, 0.0]),
            gain_points: Mutex::new(0),
            arrangement: Mutex::new(SpeakerArr::kStereo),
            active: Mutex::new([[true; 2]; 2]),
        }
    }
    fn load(&self, state: *mut IBStream) -> tresult {
        load_values(&self.values, state)
    }
    fn save(&self, state: *mut IBStream) -> tresult {
        save_values(&self.values, state)
    }
}

fn load_values(values: &Mutex<[f64; 2]>, state: *mut IBStream) -> tresult {
    let Some(stream) = (unsafe { ComRef::from_raw(state) }) else {
        return kInvalidArgument;
    };
    let mut bytes = [0; 16];
    let mut read = 0;
    if unsafe { stream.read(bytes.as_mut_ptr().cast(), 16, &mut read) } != kResultOk || read != 16 {
        return kResultFalse;
    }
    let mut gain = [0; 8];
    gain.copy_from_slice(&bytes[..8]);
    let mut bypass = [0; 8];
    bypass.copy_from_slice(&bytes[8..]);
    *lock(values) = [f64::from_le_bytes(gain), f64::from_le_bytes(bypass)];
    kResultOk
}
fn save_values(values: &Mutex<[f64; 2]>, state: *mut IBStream) -> tresult {
    let Some(stream) = (unsafe { ComRef::from_raw(state) }) else {
        return kInvalidArgument;
    };
    let values = *lock(values);
    let bytes: Vec<_> = values.into_iter().flat_map(f64::to_le_bytes).collect();
    let mut written = 0;
    unsafe { stream.write(bytes.as_ptr().cast_mut().cast(), 16, &mut written) }
}
impl Class for Routing {
    type Interfaces = (IComponent, IAudioProcessor, IConnectionPoint);
}
impl IPluginBaseTrait for Routing {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        self.connection.initialize(context)
    }
    unsafe fn terminate(&self) -> tresult {
        self.connection.terminate()
    }
}
impl IComponentTrait for Routing {
    unsafe fn getControllerClassId(&self, class: *mut TUID) -> tresult {
        unsafe { *class = CONTROLLER_CID };
        kResultOk
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: i32, direction: i32) -> int32 {
        if media == MediaTypes_::kAudio as i32 {
            2
        } else if direction == BusDirections_::kInput as i32 {
            crate::EVENT_INPUTS as i32
        } else {
            1
        }
    }
    unsafe fn getBusInfo(
        &self,
        media: i32,
        direction: i32,
        index: int32,
        out: *mut BusInfo,
    ) -> tresult {
        if media == MediaTypes_::kEvent as i32 {
            return event_bus_info(direction, index, unsafe { &mut *out });
        }
        if media != MediaTypes_::kAudio as i32 || !(0..2).contains(&index) {
            return kInvalidArgument;
        }
        let out = unsafe { &mut *out };
        out.mediaType = media;
        out.direction = direction;
        out.channelCount = if index == 1 {
            lock(&self.arrangement).count_ones() as i32
        } else if direction == BusDirections_::kInput as i32 {
            1
        } else {
            2
        };
        out.busType = if index == 1 {
            BusTypes_::kMain as i32
        } else {
            BusTypes_::kAux as i32
        };
        out.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
        copy_wide(
            if index == 1 {
                "Main"
            } else if direction == BusDirections_::kInput as i32 {
                "External key"
            } else {
                "Monitor"
            },
            &mut out.name,
        );
        kResultOk
    }
    unsafe fn getRoutingInfo(
        &self,
        _input: *mut RoutingInfo,
        _output: *mut RoutingInfo,
    ) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(
        &self,
        media: i32,
        direction: i32,
        index: int32,
        state: TBool,
    ) -> tresult {
        if media == MediaTypes_::kEvent as i32 {
            return kResultOk;
        }
        if media != MediaTypes_::kAudio as i32
            || !(0..2).contains(&index)
            || !(0..2).contains(&direction)
        {
            return kInvalidArgument;
        }
        lock(&self.active)[direction as usize][index as usize] = state != 0;
        kResultOk
    }
    unsafe fn setActive(&self, state: TBool) -> tresult {
        if state == 0 || self.connection.ready() {
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        self.load(state)
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        self.save(state)
    }
}
fn event_bus_info(direction: i32, index: int32, out: &mut BusInfo) -> tresult {
    let input = direction == BusDirections_::kInput as i32;
    let count = if input { crate::EVENT_INPUTS as i32 } else { 1 };
    if !(0..count).contains(&index) {
        return kInvalidArgument;
    }
    out.mediaType = MediaTypes_::kEvent as i32;
    out.direction = direction;
    out.channelCount = 16;
    out.busType = if index == 0 {
        BusTypes_::kMain as i32
    } else {
        BusTypes_::kAux as i32
    };
    out.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
    let name = match (input, index) {
        (false, _) => "Notes out",
        (true, 0) => "Notes",
        (true, 1) => "Octave notes",
        _ => "Expressions",
    };
    copy_wide(name, &mut out.name);
    kResultOk
}
