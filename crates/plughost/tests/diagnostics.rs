//! Real CLAP callbacks are collected in bounded queues and explicitly drained over IPC.
use plughost::{DiagnosticSeverity, FailureKind, PluginFormat};

mod support;

use support::{delay, prepare, spawn};

#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn plugin_severity_identity_overflow_and_reset_survive_ipc() {
    let plugin = delay(PluginFormat::Clap);
    let mut chain = spawn(std::slice::from_ref(&plugin));
    let batch = chain.take_diagnostics().unwrap();
    assert_eq!(batch.dropped, 0);
    assert_eq!(
        batch.records.iter().map(|r| r.severity).collect::<Vec<_>>(),
        vec![
            DiagnosticSeverity::Debug,
            DiagnosticSeverity::Info,
            DiagnosticSeverity::Warning,
            DiagnosticSeverity::Error,
            DiagnosticSeverity::Fatal,
            DiagnosticSeverity::HostMisbehaving,
            DiagnosticSeverity::PluginMisbehaving,
            DiagnosticSeverity::Error,
            DiagnosticSeverity::Error,
        ]
    );
    assert!(
        batch
            .records
            .iter()
            .take(7)
            .all(|r| r.plugin.as_ref() == Some(&plugin)
                && r.slot == Some(0)
                && r.failure_kind.is_none())
    );
    // Clack logs the two deliberately rejected GUI requests through the native host logger.
    for record in &batch.records[7..] {
        assert_eq!(record.plugin.as_ref(), Some(&plugin));
        assert_eq!(record.slot, Some(0));
        assert_eq!(record.failure_kind, None);
    }
    // The rejected GUI visibility requests were not accepted by the host.
    let parameters = chain.parameters(0).unwrap();
    for id in [5, 6] {
        assert_eq!(parameters.iter().find(|(p, _)| p.id == id).unwrap().1, 0.0);
    }
    prepare(&mut chain);
    let input = [1.0; 512];
    let (mut left, mut right) = ([0.0; 512], [0.0; 512]);
    for _ in 0..3 {
        chain
            .process_audio_f32(
                &plughost_core::BlockContext::new(left.len()),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[],
                &mut Vec::new(),
            )
            .unwrap();
    }
    assert_eq!(left[511], 1.0);
    // Reset retains the buffer, including overflow counts.
    chain.reset().unwrap();
    let batch = chain.take_diagnostics().unwrap();
    assert_eq!(batch.records.len(), plughost_core::DIAGNOSTIC_CAPACITY);
    assert!(batch.dropped > 0);
    assert!(
        batch
            .records
            .iter()
            .all(|r| r.message == "fixture process log")
    );
    assert!(chain.take_diagnostics().unwrap().records.is_empty());

    let error = chain.open_editor(0).unwrap_err();
    assert_eq!(error.kind(), FailureKind::Unsupported);
    let batch = chain.take_diagnostics().unwrap();
    assert_eq!(batch.records.len(), 1);
    assert_eq!(
        batch.records[0].failure_kind,
        Some(FailureKind::Unsupported)
    );
    assert_eq!(batch.records[0].slot, Some(0));
    assert_eq!(batch.records[0].severity, DiagnosticSeverity::Error);
}
