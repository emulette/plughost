use std::error::Error as StdError;

use plughost::{Error, Failure, FailureKind, InputError, PresetSearchError, RenderError};

fn messages(error: &dyn StdError) -> Vec<String> {
    let mut messages = vec![error.to_string()];
    let mut cause = error.source();
    while let Some(error) = cause {
        messages.push(error.to_string());
        cause = error.source();
    }
    messages
}

#[test]
fn error_chains_keep_context_and_report_each_cause_once() {
    for (error, context) in [
        (
            Error::HelperStart(std::io::Error::other("fixture I/O cause")),
            "could not start the helper",
        ),
        (
            Error::Cache(std::io::Error::other("fixture I/O cause")),
            "could not access the scan cache",
        ),
    ] {
        assert_eq!(messages(&error), [context, "fixture I/O cause"]);
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<std::io::Error>()
                .is_some()
        );
    }
    let error = Error::Operation {
        slot: Some(2),
        failure: Failure::new(FailureKind::Load, "fixture native cause"),
    };
    assert_eq!(
        messages(&error),
        ["the operation failed (slot 2)", "fixture native cause"]
    );
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<Failure>()
            .unwrap()
            .kind,
        FailureKind::Load
    );
    let reason = InputError::ParameterValue.to_string();
    let error = Error::Input {
        slot: Some(3),
        error: InputError::ParameterValue,
    };
    assert_eq!(messages(&error), ["invalid input (slot 3)", &reason]);
    let reason = RenderError::TailDuration.to_string();
    assert_eq!(
        messages(&Error::Render(RenderError::TailDuration)),
        ["rendering failed", &reason]
    );
}

#[test]
fn preset_search_context_names_the_path_and_keeps_the_io_cause() {
    let error = PresetSearchError::Io {
        path: "presets".into(),
        source: std::io::Error::other("fixture I/O cause"),
    };
    assert_eq!(
        messages(&error),
        [
            "could not search the preset directory (presets)",
            "fixture I/O cause"
        ]
    );
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .is_some()
    );
}
