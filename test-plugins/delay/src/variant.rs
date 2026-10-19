//! Which fault a copy of this binary plays. The build scripts copy the one binary under several
//! bundle names, and the name of the module the host loaded picks the variant, so the faults
//! still trigger at the native entry points (`GetPluginFactory`, the CLAP entry, `process`).

use std::sync::OnceLock;

use crate::errors;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Delay,
    CrashInProcess,
    HangInProcess,
    CrashOnScan,
    HangOnScan,
    NoopReset,
    LatencyOverflow,
    LargeState,
    StallMainThread,
    ArrangementFalse,
    Editor,
    RestartOnActivate,
    Timers,
    LatencyChange,
    ThreadCheck,
    StartFailure,
    FlushRequest,
    MpePorts,
    ContextRequired,
    MappingProbe,
    CrashOnDestroy,
    NoFactory,
    SlowUnload,
    StallInProcess,
    StallInState,
    FloatMode,
}

/// Module file name (without extension), class ID suffix, and plugin name of each variant.
const VARIANTS: [(Variant, &str, u32, &str); 26] = [
    (
        Variant::Delay,
        "plughost-test-delay",
        0x79000001,
        "plughost test delay",
    ),
    (
        Variant::CrashInProcess,
        "plughost-test-crash-in-process",
        0x79000002,
        "plughost test crash in process",
    ),
    (
        Variant::HangInProcess,
        "plughost-test-hang-in-process",
        0x79000003,
        "plughost test hang in process",
    ),
    (
        Variant::CrashOnScan,
        "plughost-test-crash-on-scan",
        0x79000004,
        "plughost test crash on scan",
    ),
    (
        Variant::HangOnScan,
        "plughost-test-hang-on-scan",
        0x79000005,
        "plughost test hang on scan",
    ),
    (
        Variant::NoopReset,
        "plughost-test-noop-reset",
        0x79000006,
        "plughost test noop reset",
    ),
    (
        Variant::LatencyOverflow,
        "plughost-test-latency-overflow",
        0x79000007,
        "plughost test latency overflow",
    ),
    (
        Variant::LargeState,
        "plughost-test-large-state",
        0x79000008,
        "plughost test large state",
    ),
    (
        Variant::StallMainThread,
        "plughost-test-stall-main-thread",
        0x79000009,
        "plughost test stall main thread",
    ),
    (
        Variant::ArrangementFalse,
        "plughost-test-arrangement-false",
        0x7900000A,
        "plughost test arrangement false",
    ),
    (
        Variant::Editor,
        "plughost-test-editor",
        0x7900000B,
        "plughost test editor",
    ),
    (
        Variant::RestartOnActivate,
        "plughost-test-restart-on-activate",
        0x7900000C,
        "plughost test restart on activate",
    ),
    (
        Variant::Timers,
        "plughost-test-timers",
        0x7900000D,
        "plughost test timers",
    ),
    (
        Variant::LatencyChange,
        "plughost-test-latency-change",
        0x7900000E,
        "plughost test latency change",
    ),
    (
        Variant::ThreadCheck,
        "plughost-test-thread-check",
        0x7900000F,
        "plughost test thread check",
    ),
    (
        Variant::StartFailure,
        "plughost-test-start-failure",
        0x79000010,
        "plughost test start failure",
    ),
    (
        Variant::FlushRequest,
        "plughost-test-flush-request",
        0x79000011,
        "plughost test flush request",
    ),
    (
        Variant::MpePorts,
        "plughost-test-mpe-ports",
        0x79000012,
        "plughost test MPE ports",
    ),
    (
        Variant::ContextRequired,
        "plughost-test-context-required",
        0x79000013,
        "plughost test context required",
    ),
    (
        Variant::MappingProbe,
        "plughost-test-mapping-probe",
        0x79000014,
        "plughost test mapping probe",
    ),
    (
        Variant::CrashOnDestroy,
        "plughost-test-crash-on-destroy",
        0x79000015,
        "plughost test crash on destroy",
    ),
    (
        Variant::NoFactory,
        "plughost-test-no-factory",
        0x79000016,
        "plughost test no factory",
    ),
    (
        Variant::SlowUnload,
        "plughost-test-slow-unload",
        0x79000017,
        "plughost test slow unload",
    ),
    (
        Variant::StallInProcess,
        "plughost-test-stall-in-process",
        0x79000018,
        "plughost test stall in process",
    ),
    (
        Variant::StallInState,
        "plughost-test-stall-in-state",
        0x79000019,
        "plughost test stall in state",
    ),
    (
        Variant::FloatMode,
        "plughost-test-float-mode",
        0x7900001A,
        "plughost test float mode",
    ),
];

impl Variant {
    fn entry(self) -> &'static (Variant, &'static str, u32, &'static str) {
        VARIANTS.iter().find(|entry| entry.0 == self).unwrap()
    }

    pub fn id(self) -> u32 {
        self.entry().2
    }

    pub fn name(self) -> &'static str {
        self.entry().3
    }
}

/// The variant named by the module this code was loaded from.
pub fn variant() -> Variant {
    static VARIANT: OnceLock<Variant> = OnceLock::new();
    *VARIANT.get_or_init(|| {
        let module = module_file_stem();
        VARIANTS
            .iter()
            .find(|entry| entry.1 == module)
            .expect(errors::UNKNOWN_VARIANT)
            .0
    })
}

#[cfg(target_os = "macos")]
fn module_file_stem() -> String {
    // SAFETY: `dladdr` fills `info` for an address inside this loaded image, and `dli_fname` is a
    // NUL-terminated path that stays valid while the image is loaded.
    let path = unsafe {
        let mut info = std::mem::zeroed::<libc::Dl_info>();
        assert_ne!(
            libc::dladdr(variant as *const std::ffi::c_void, &mut info),
            0,
            "{}",
            errors::MODULE_PATH
        );
        std::ffi::CStr::from_ptr(info.dli_fname)
            .to_string_lossy()
            .into_owned()
    };
    file_stem(&path)
}

#[cfg(target_os = "windows")]
fn module_file_stem() -> String {
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
            variant as *const u16,
            &mut module,
        );
        assert_ne!(found, 0, "{}", errors::MODULE_PATH);
        GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) as usize
    };
    file_stem(&String::from_utf16_lossy(&path[..length]))
}

fn file_stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .expect(errors::MODULE_PATH)
        .to_string_lossy()
        .into_owned()
}
