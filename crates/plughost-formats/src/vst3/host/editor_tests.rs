use super::*;
use vst3::Steinberg::Vst::{IMidiMapping, ViewType};

#[test]
fn only_editor_open_requests_are_accepted_and_taken_once() {
    let owner = ComWrapper::new(ComponentHandler::default());
    let handler = owner.as_com_ref::<IComponentHandler2>().unwrap();
    assert!(!owner.take_editor_requested());
    unsafe {
        assert_eq!(handler.requestOpenEditor(c"other".as_ptr()), kResultFalse);
        assert_eq!(handler.requestOpenEditor(ViewType::kEditor), kResultOk);
    }
    assert!(owner.take_editor_requested());
    assert!(!owner.take_editor_requested());
}

#[test]
fn host_advertises_the_plugin_interfaces_it_calls() {
    let host = ComWrapper::new(HostApplication {
        name: String::new(),
    });
    let support = host.as_com_ref::<IPlugInterfaceSupport>().unwrap();
    let iid = |id: [u8; 16]| id.map(|b| b as std::ffi::c_char);
    unsafe {
        assert_eq!(
            support.isPlugInterfaceSupported(&iid(IMidiMapping::IID)),
            kResultOk
        );
        assert_eq!(
            support.isPlugInterfaceSupported(&iid(IComponentHandler2::IID)),
            kResultFalse
        );
    }
}
