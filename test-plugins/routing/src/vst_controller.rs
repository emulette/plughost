use super::*;
impl IEditControllerTrait for Routing {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        self.load(state)
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        self.load(state)
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        self.save(state)
    }
    unsafe fn getParameterCount(&self) -> int32 {
        2
    }
    unsafe fn getParameterInfo(&self, index: int32, info: *mut ParameterInfo) -> tresult {
        if !(0..2).contains(&index) {
            return kInvalidArgument;
        }
        let info = unsafe { &mut *info };
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
        kResultOk
    }
    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }
    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}
