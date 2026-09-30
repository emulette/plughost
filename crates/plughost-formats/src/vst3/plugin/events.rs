use super::*;
use plughost_core::{Diagnostic, DiagnosticBatch, DiagnosticSeverity, ParameterEventBatch};

impl Plugin {
    /// Takes the controller's notifications, after delivering the values the processing thread
    /// handed back and reading a changed parameter list again.
    pub(crate) fn take_parameter_events(&mut self) -> ParameterEventBatch {
        self.sync_controller();
        self.parameter_cache();
        self.handler.events.take()
    }

    /// Reports output events the plugin produced that have no MIDI 1.0 form, which are not
    /// delivered.
    pub(crate) fn take_diagnostics(&self) -> DiagnosticBatch {
        let unconvertible = self.handler.take_unconvertible_output_events();
        DiagnosticBatch {
            records: (unconvertible > 0)
                .then(|| Diagnostic {
                    severity: DiagnosticSeverity::Warning,
                    message: super::super::errors::unconvertible_output_events(unconvertible),
                    plugin: None,
                    slot: None,
                    failure_kind: None,
                    truncated: false,
                })
                .into_iter()
                .collect(),
            dropped: 0,
        }
    }
}
