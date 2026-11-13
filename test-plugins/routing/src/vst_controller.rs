use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

/// The bypass parameter.
const BYPASS: ParamID = 1;
/// Read-only: how many points the last block with audio brought for the gain, in hundredths.
const GAIN_POINTS: ParamID = 2;
/// Read-only, in the `routing-strict` bundle: how many times the host wrote the bypass
/// parameter with `setParamNormalized`, in hundredths.
const BYPASS_WRITES: ParamID = 3;

pub(super) struct Controller {
    pub(super) connection: connection::Connection,
    values: Mutex<[f64; 2]>,
    bypass_writes: AtomicU32,
}

impl Controller {
    pub(super) fn new() -> Self {
        Self {
            connection: connection::Connection::default(),
            // The host must copy component state; sharing one Rust object can hide this step.
            values: Mutex::new([0.0, 1.0]),
            bypass_writes: AtomicU32::new(0),
        }
    }
}

/// Whether this copy of the fixture is the `routing-strict` bundle, by the module it was loaded
/// from.
fn strict() -> bool {
    static STRICT: OnceLock<bool> = OnceLock::new();
    *STRICT.get_or_init(|| {
        std::path::Path::new(&module_path())
            .file_stem()
            .is_some_and(|stem| stem == "plughost-test-routing-strict")
    })
}

#[cfg(target_os = "macos")]
fn module_path() -> String {
    // SAFETY: `dladdr` fills `info` for an address inside this loaded image, and `dli_fname` is a
    // NUL-terminated path that stays valid while the image is loaded.
    unsafe {
        let mut info = std::mem::zeroed::<libc::Dl_info>();
        assert_ne!(
            libc::dladdr(strict as *const std::ffi::c_void, &mut info),
            0,
            "{}",
            crate::errors::MODULE_PATH
        );
        std::ffi::CStr::from_ptr(info.dli_fname)
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(target_os = "windows")]
fn module_path() -> String {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };
    let mut path = [0u16; 1024];
    // SAFETY: the address is inside this module, and the buffer length is passed along.
    let length = unsafe {
        let mut module = std::ptr::null_mut();
        let found = GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            strict as *const u16,
            &mut module,
        );
        assert_ne!(found, 0, "{}", crate::errors::MODULE_PATH);
        GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) as usize
    };
    String::from_utf16_lossy(&path[..length])
}
impl Class for Controller {
    type Interfaces = (IEditController, IConnectionPoint);
}
impl IPluginBaseTrait for Controller {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        self.connection.initialize(context)
    }
    unsafe fn terminate(&self) -> tresult {
        self.connection.terminate()
    }
}
impl IEditControllerTrait for Controller {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        if !self.connection.ready() {
            return kResultFalse;
        }
        load_values(&self.values, state)
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        load_values(&self.values, state)
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        save_values(&self.values, state)
    }
    unsafe fn getParameterCount(&self) -> int32 {
        if strict() { 4 } else { 3 }
    }
    unsafe fn getParameterInfo(&self, index: int32, info: *mut ParameterInfo) -> tresult {
        if !(0..unsafe { self.getParameterCount() }).contains(&index) {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
        if index == GAIN_POINTS as i32 || index == BYPASS_WRITES as i32 {
            info.id = index as ParamID;
            copy_wide(
                if index == GAIN_POINTS as i32 {
                    "Gain points"
                } else {
                    "Bypass writes"
                },
                &mut info.title,
            );
            info.shortTitle = info.title;
            info.units = [0; 128];
            info.stepCount = 0;
            info.defaultNormalizedValue = 0.0;
            info.unitId = 0;
            info.flags = ParameterInfo_::ParameterFlags_::kIsReadOnly as i32;
            return kResultOk;
        }
        info.id = index as u32;
        copy_wide(if index == 0 { "Gain" } else { "Bypass" }, &mut info.title);
        info.shortTitle = info.title;
        info.units = [0; 128];
        info.stepCount = index;
        info.defaultNormalizedValue = if index == 0 { 1.0 } else { 0.0 };
        info.unitId = 0;
        info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate as i32
            | if index == 1 {
                ParameterInfo_::ParameterFlags_::kIsBypass as i32
            } else {
                0
            };
        kResultOk
    }
    unsafe fn getParamStringByValue(
        &self,
        _id: ParamID,
        value: ParamValue,
        text: *mut String128,
    ) -> tresult {
        copy_wide(&value.to_string(), unsafe { &mut *text });
        kResultOk
    }
    unsafe fn getParamValueByString(
        &self,
        _id: ParamID,
        _text: *mut TChar,
        _value: *mut ParamValue,
    ) -> tresult {
        kNotImplemented
    }
    unsafe fn normalizedParamToPlain(&self, _id: ParamID, value: ParamValue) -> ParamValue {
        value
    }
    unsafe fn plainParamToNormalized(&self, _id: ParamID, value: ParamValue) -> ParamValue {
        value
    }
    unsafe fn getParamNormalized(&self, id: ParamID) -> ParamValue {
        if id == GAIN_POINTS {
            return self.connection.gain_points();
        }
        if id == BYPASS_WRITES {
            return f64::from(self.bypass_writes.load(Ordering::Relaxed)) / 100.0;
        }
        lock(&self.values)
            .get(id as usize)
            .copied()
            .unwrap_or_default()
    }
    unsafe fn setParamNormalized(&self, id: ParamID, value: ParamValue) -> tresult {
        let mut values = lock(&self.values);
        let Some(slot) = values.get_mut(id as usize) else {
            return kInvalidArgument;
        };
        *slot = value;
        if !strict() {
            return kResultOk;
        }
        if id == BYPASS {
            self.bypass_writes.fetch_add(1, Ordering::Relaxed);
        }
        // The value is taken, but reported as refused.
        kResultFalse
    }
    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }
    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}
