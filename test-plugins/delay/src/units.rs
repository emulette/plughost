//! Real VST3 group metadata, including a deliberately cyclic hierarchy after a failed state load.
use super::*;
use vst3::Steinberg::Vst::{IUnitInfoTrait, ProgramListInfo, UnitInfo};

impl IUnitInfoTrait for Delay {
    unsafe fn getUnitCount(&self) -> i32 {
        2
    }
    unsafe fn getUnitInfo(&self, index: i32, info: *mut UnitInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        match index {
            0 => {
                info.id = 0;
                info.parentUnitId = -1;
                copy_wide("Effects", &mut info.name);
            }
            1 => {
                info.id = 7;
                info.parentUnitId = if *lock(&self.gain) < 0.0 { 7 } else { 0 };
                copy_wide("Delay", &mut info.name);
            }
            _ => return kInvalidArgument,
        }
        info.programListId = if info.id == 7 { 100 } else { -1 };
        kResultOk
    }
    unsafe fn getProgramListCount(&self) -> i32 {
        1
    }
    unsafe fn getProgramListInfo(&self, index: i32, info: *mut ProgramListInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        if index != 0 {
            return kInvalidArgument;
        }
        info.id = 100;
        info.programCount = 3;
        copy_wide("Factory delay", &mut info.name);
        kResultOk
    }
    unsafe fn getProgramName(&self, list: i32, index: i32, name: *mut String128) -> tresult {
        let Some(name) = (unsafe { name.as_mut() }) else {
            return kInvalidArgument;
        };
        if list != 100 {
            return kInvalidArgument;
        }
        let Some(program) = ["Unity", "Quarter", "Half"].get(index as usize) else {
            return kInvalidArgument;
        };
        copy_wide(program, name);
        kResultOk
    }
    unsafe fn getProgramInfo(
        &self,
        _list: i32,
        _index: i32,
        _attribute: *const c_char,
        _value: *mut String128,
    ) -> tresult {
        kInvalidArgument
    }
    unsafe fn hasProgramPitchNames(&self, _list: i32, _index: i32) -> tresult {
        kResultFalse
    }
    unsafe fn getProgramPitchName(
        &self,
        _list: i32,
        _index: i32,
        _pitch: i16,
        _name: *mut String128,
    ) -> tresult {
        kInvalidArgument
    }
    unsafe fn getSelectedUnit(&self) -> i32 {
        0
    }
    unsafe fn selectUnit(&self, id: i32) -> tresult {
        if id == 0 || id == 7 {
            kResultOk
        } else {
            kInvalidArgument
        }
    }
    unsafe fn getUnitByBus(
        &self,
        _kind: i32,
        _dir: i32,
        _bus: i32,
        _channel: i32,
        _id: *mut i32,
    ) -> tresult {
        kNotImplemented
    }
    unsafe fn setUnitProgramData(&self, _list: i32, _index: i32, _data: *mut IBStream) -> tresult {
        kNotImplemented
    }
}
