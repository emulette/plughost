//! Separate component/controller state, connected only through the host's native proxies.
use super::*;
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(super) struct Connection {
    host: Mutex<Option<ComPtr<IHostApplication>>>,
    peer: Mutex<Option<ComPtr<IConnectionPoint>>>,
    received_hello: AtomicBool,
}

impl Connection {
    pub(super) fn initialize(&self, context: *mut FUnknown) -> tresult {
        let host = unsafe { ComRef::from_raw(context) }.and_then(|host| host.cast());
        if host.is_none() {
            return kNoInterface;
        }
        *lock(&self.host) = host;
        kResultOk
    }

    pub(super) fn terminate(&self) -> tresult {
        if lock(&self.peer).is_some() {
            return kResultFalse;
        }
        *lock(&self.host) = None;
        kResultOk
    }

    pub(super) fn ready(&self) -> bool {
        self.received_hello.load(Ordering::Relaxed) && lock(&self.peer).is_some()
    }

    fn message(&self, id: &CStr) -> Option<ComPtr<IMessage>> {
        let host = lock(&self.host).clone()?;
        let mut cid = IMessage::IID.map(|byte| byte as c_char);
        let mut iid = cid;
        let mut raw = std::ptr::null_mut();
        if unsafe { host.createInstance(&mut cid, &mut iid, &mut raw) } != kResultOk {
            return None;
        }
        let message = unsafe { ComPtr::<IMessage>::from_raw(raw.cast()) }?;
        unsafe { message.setMessageID(id.as_ptr()) };
        Some(message)
    }

    fn connect(&self, other: *mut IConnectionPoint, id: &CStr) -> tresult {
        let Some(peer) = (unsafe { ComRef::from_raw(other) }).map(|peer| peer.to_com_ptr()) else {
            return kInvalidArgument;
        };
        *lock(&self.peer) = Some(peer.clone());
        let Some(message) = self.message(id) else {
            return kResultFalse;
        };
        unsafe { peer.notify(message.as_ptr()) }
    }

    fn disconnect(&self, other: *mut IConnectionPoint) -> tresult {
        let mut peer = lock(&self.peer);
        if peer.as_ref().is_none_or(|peer| peer.as_ptr() != other) {
            return kInvalidArgument;
        }
        *peer = None;
        self.received_hello.store(false, Ordering::Relaxed);
        kResultOk
    }

    pub(super) fn gain_points(&self) -> f64 {
        let result = (|| {
            let peer = lock(&self.peer).clone()?;
            let message = self.message(c"routing.gain-points")?;
            if unsafe { peer.notify(message.as_ptr()) } != kResultOk {
                return None;
            }
            let attributes = unsafe { ComRef::from_raw(message.getAttributes()) }?;
            let mut count = 0;
            (unsafe { attributes.getInt(c"count".as_ptr(), &mut count) } == kResultOk)
                .then_some(count as f64 / 100.0)
        })();
        // Failed native communication is observable in the fixture's diagnostic parameter.
        result.unwrap_or(f64::NAN)
    }
}

fn message_id<'a>(message: &'a ComRef<'_, IMessage>) -> Option<&'a CStr> {
    let id = unsafe { message.getMessageID() };
    (!id.is_null()).then(|| unsafe { CStr::from_ptr(id) })
}

impl IConnectionPointTrait for Routing {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        self.connection.connect(other, c"routing.component")
    }
    unsafe fn disconnect(&self, other: *mut IConnectionPoint) -> tresult {
        self.connection.disconnect(other)
    }
    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        let Some(message) = (unsafe { ComRef::from_raw(message) }) else {
            return kInvalidArgument;
        };
        match message_id(&message) {
            Some(id) if id == c"routing.controller" => {
                self.connection
                    .received_hello
                    .store(true, Ordering::Relaxed);
                kResultOk
            }
            Some(id) if id == c"routing.gain-points" => {
                let Some(attributes) = (unsafe { ComRef::from_raw(message.getAttributes()) })
                else {
                    return kInvalidArgument;
                };
                unsafe { attributes.setInt(c"count".as_ptr(), *lock(&self.gain_points) as i64) }
            }
            _ => kResultFalse,
        }
    }
}

impl IConnectionPointTrait for controller::Controller {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        self.connection.connect(other, c"routing.controller")
    }
    unsafe fn disconnect(&self, other: *mut IConnectionPoint) -> tresult {
        self.connection.disconnect(other)
    }
    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        let Some(message) = (unsafe { ComRef::from_raw(message) }) else {
            return kInvalidArgument;
        };
        if message_id(&message) != Some(c"routing.component") {
            return kResultFalse;
        }
        self.connection
            .received_hello
            .store(true, Ordering::Relaxed);
        kResultOk
    }
}
