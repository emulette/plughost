use super::*;
use clack_host::events::spaces::CoreEventSpace;
use plughost_core::{ParameterEvent, ParameterEventBatch, ParameterEventBuffer};

impl Plugin {
    /// Drains the plugin's parameter events. A rescan of parameter info among them also refreshes
    /// the processing thread's parameter list.
    pub(crate) fn take_parameter_events(&mut self) -> ParameterEventBatch {
        self.parameter_cache();
        self.shared().parameter_events.take()
    }
}

pub(super) struct ParameterOutput<'a> {
    pub parameters: &'a ParameterCache,
    pub queue: &'a ParameterEventBuffer,
}

impl clack_host::events::io::OutputEventBuffer for ParameterOutput<'_> {
    fn try_push(
        &mut self,
        event: &clack_host::events::UnknownEvent,
    ) -> Result<(), clack_host::events::io::TryPushError> {
        let parameters = self.parameters;
        let queue = self.queue;
        let notification = match event.as_core_event() {
            Some(CoreEventSpace::ParamValue(value)) => {
                // Per-note expression is not the global parameter value exposed by this API.
                if !value.pckn().matches_all() {
                    return Ok(());
                }
                let Some(id) = value.param_id().map(|id| u64::from(id.get())) else {
                    return Ok(());
                };
                let Some(parameter) = parameters.get(id) else {
                    queue.record(ParameterEvent::ValuesChanged);
                    return Ok(());
                };
                let normalized = parameter.normalized(value.value());
                if !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
                    return Ok(());
                }
                ParameterEvent::Value { id, normalized }
            }
            Some(CoreEventSpace::ParamGestureBegin(value)) => {
                let Some(id) = value.param_id() else {
                    return Ok(());
                };
                ParameterEvent::BeginEdit {
                    id: u64::from(id.get()),
                }
            }
            Some(CoreEventSpace::ParamGestureEnd(value)) => {
                let Some(id) = value.param_id() else {
                    return Ok(());
                };
                ParameterEvent::EndEdit {
                    id: u64::from(id.get()),
                }
            }
            _ => return Ok(()),
        };
        queue.record(notification);
        Ok(())
    }
}
