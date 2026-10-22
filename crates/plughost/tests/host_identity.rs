//! The fixture reports the identity it received from the native host API in read-only parameter
//! titles. State restoration cannot synthesize these observations.
use std::path::Path;

use plughost::{
    Chain, Error, HostIdentity, InputError, PluginFormat, ScanOutcome, Scanner, Timeouts,
};

mod support;

use support::{bundle, delay, helper, prepare, spawn_with};

fn check_lifecycle(format: PluginFormat, first_parameter: u64) {
    // The second name exactly fills VST3's 127 UTF-16 units, including surrogate pairs.
    for name in ["검증 🎛 Host".to_owned(), format!("{}x", "🎛".repeat(63))] {
        let expected = HostIdentity {
            name,
            vendor: "테스트 Vendor".to_owned(),
            version: "2.3.4-beta".to_owned(),
        };
        let mut supplied = expected.clone();
        let mut chain = spawn_with(&[delay(format)], &supplied, Timeouts::default());
        supplied.name = "changed after spawn".to_owned();
        prepare(&mut chain);
        for stage in 0..3 {
            match stage {
                1 => chain.reset().unwrap(),
                2 => chain = chain.recover(&[]).unwrap(),
                _ => {}
            }
            let parameters = chain.parameters(0).unwrap();
            let observed: Vec<&str> = (first_parameter..)
                .take(if format == PluginFormat::Clap { 3 } else { 1 })
                .map(|id| {
                    parameters
                        .iter()
                        .find(|(p, _)| p.id == id)
                        .unwrap()
                        .0
                        .title
                        .as_str()
                })
                .collect();
            let wanted = if format == PluginFormat::Clap {
                vec![
                    expected.name.as_str(),
                    expected.vendor.as_str(),
                    expected.version.as_str(),
                ]
            } else {
                vec![expected.name.as_str()]
            };
            assert_eq!(observed, wanted, "lifecycle stage {stage}");
        }
    }
}

#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn vst3_receives_host_identity_after_reset_and_recovery() {
    check_lifecycle(PluginFormat::Vst3, 3);
}

#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn clap_receives_host_identity_after_reset_and_recovery() {
    check_lifecycle(PluginFormat::Clap, 2);
}

/// The routing fixture reports the name in its factory's host context as its vendor.
#[test]
#[ignore = "needs scripts/build-helper.ps1 and scripts/build-test-plugins.ps1 (or .sh)"]
fn vst3_scans_present_the_scanner_host_identity() {
    let directory = tempfile::tempdir().unwrap();
    let identity = HostIdentity {
        name: "검증 🎛 Scan Host".to_owned(),
        ..HostIdentity::default()
    };
    let scan = Scanner::new(&helper(), &directory.path().join("cache.json"))
        .host(identity.clone())
        .retry(&bundle(PluginFormat::Vst3, "plughost-test-routing"))
        .unwrap();
    let ScanOutcome::Found(classes) = &scan.outcome else {
        panic!("{:?}", scan.outcome)
    };
    assert_eq!(classes[0].vendor, identity.name);
}

#[test]
fn invalid_host_identity_is_rejected_before_starting_a_helper() {
    let invalid = [
        HostIdentity {
            name: String::new(),
            ..HostIdentity::default()
        },
        HostIdentity {
            name: "x".repeat(128),
            ..HostIdentity::default()
        },
        HostIdentity {
            name: "🎛".repeat(64),
            ..HostIdentity::default()
        },
        HostIdentity {
            name: "host\0other".to_owned(),
            ..HostIdentity::default()
        },
        HostIdentity {
            vendor: "vendor\0other".to_owned(),
            ..HostIdentity::default()
        },
        HostIdentity {
            version: "1\0other".to_owned(),
            ..HostIdentity::default()
        },
    ];
    for host in invalid {
        assert!(matches!(
            Chain::spawn(
                Path::new("/nonexistent/helper"),
                &[],
                &host,
                Timeouts::default()
            ),
            Err(Error::Input {
                slot: None,
                error: InputError::HostIdentity
            })
        ));
        let scanner = Scanner::new(
            Path::new("/nonexistent/helper"),
            Path::new("/nonexistent/cache.json"),
        )
        .directories(Vec::new())
        .host(host);
        assert!(matches!(
            scanner.scan(&|_| {}),
            Err(Error::Input {
                slot: None,
                error: InputError::HostIdentity
            })
        ));
        assert!(matches!(
            scanner.retry(Path::new("/nonexistent/plugin.vst3")),
            Err(Error::Input {
                slot: None,
                error: InputError::HostIdentity
            })
        ));
    }
}
