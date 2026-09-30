use super::*;
#[derive(Default)]
struct Factory {
    host: Mutex<Option<vst3::ComPtr<IHostApplication>>>,
}

impl Class for Factory {
    type Interfaces = (IPluginFactory3,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = unsafe { &mut *info };
        let mut name = [0; 128];
        if let Some(host) = &*lock(&self.host) {
            unsafe { host.getName(&mut name) };
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        copy_c(&String::from_utf16_lossy(&name[..end]), &mut info.vendor);
        copy_c("", &mut info.url);
        copy_c("", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;
        kResultOk
    }

    unsafe fn countClasses(&self) -> int32 {
        1
    }

    unsafe fn getClassInfo(&self, index: int32, info: *mut PClassInfo) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
        info.cid = CID;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_c("Audio Module Class", &mut info.category);
        copy_c(NAME, &mut info.name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        if unsafe { *(cid as *const TUID) } != CID {
            return kInvalidArgument;
        }
        let Some(instance) = ComWrapper::new(Routing::new()).to_com_ptr::<FUnknown>() else {
            return kResultFalse;
        };
        let ptr = instance.as_ptr();
        unsafe { ((*(*ptr).vtbl).queryInterface)(ptr, iid as *mut TUID, obj) }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: int32, info: *mut PClassInfo2) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
        info.cid = CID;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy_c("Audio Module Class", &mut info.category);
        copy_c(NAME, &mut info.name);
        info.classFlags = 0;
        copy_c("Fx|Dynamics", &mut info.subCategories);
        copy_c("", &mut info.vendor);
        copy_c("0.0.0", &mut info.version);
        copy_c("VST 3.8.0", &mut info.sdkVersion);
        kResultOk
    }
}

impl IPluginFactory3Trait for Factory {
    unsafe fn getClassInfoUnicode(&self, _index: int32, _info: *mut PClassInfoW) -> tresult {
        kNotImplemented
    }

    unsafe fn setHostContext(&self, context: *mut FUnknown) -> tresult {
        *lock(&self.host) = unsafe { ComRef::from_raw(context) }
            .and_then(|context| context.cast::<IHostApplication>());
        kResultOk
    }
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleEntry(_bundle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleExit() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn ExitDll() -> bool {
    true
}

#[unsafe(no_mangle)]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory::default())
        .to_com_ptr::<IPluginFactory>()
        .map_or(std::ptr::null_mut(), |factory| factory.into_raw())
}
