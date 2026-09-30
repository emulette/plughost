//! Announces the helper's calls into plugins through the application's activity mapping, so the
//! application can name the slot at fault if the helper dies or stops responding.

use std::sync::Arc;

use plughost_core::ipc::shared::{Activity, Caller};

/// The activity mapping received with the chain. Empty before any chain is loaded, when there is
/// no plugin to call.
#[derive(Clone, Default)]
pub(crate) struct Calls(Option<Arc<Activity>>);

impl Calls {
    pub fn new(activity: Activity) -> Self {
        Self(Some(Arc::new(activity)))
    }

    /// Marks `caller` as calling into the plugin in `slot` until the returned guard drops, which
    /// restores the enclosing call's slot.
    pub fn enter(&self, caller: Caller, slot: usize) -> Call<'_> {
        let activity = self.0.as_deref();
        let enclosing = activity.and_then(|activity| {
            let enclosing = activity.slot(caller);
            activity.enter(caller, slot);
            enclosing
        });
        Call {
            activity,
            caller,
            enclosing,
        }
    }
}

pub(crate) struct Call<'a> {
    activity: Option<&'a Activity>,
    caller: Caller,
    enclosing: Option<usize>,
}

impl Drop for Call<'_> {
    fn drop(&mut self) {
        if let Some(activity) = self.activity {
            match self.enclosing {
                Some(slot) => activity.enter(self.caller, slot),
                None => activity.leave(self.caller),
            }
        }
    }
}
