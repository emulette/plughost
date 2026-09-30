//! Scans installed VST3, CLAP, and Audio Unit plugins through helper processes and prints outcomes.
//!
//! usage: scan <helper executable> <cache file> [directory...]
//!        scan <helper executable> <cache file> --discover [directory...]
//!        scan <helper executable> <cache file> --retry <bundle>
//!        scan <helper executable> <cache file> --audio-units

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use plughost::{BundleScan, ScanOutcome, Scanner};

const USAGE: &str = "usage: scan <helper executable> <cache file> [directory... | --discover [directory...] | --retry <bundle> | --audio-units]";

macro_rules! out {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

fn label(outcome: &ScanOutcome) -> String {
    match outcome {
        ScanOutcome::Found(classes) => format!("found {}", classes.len()),
        ScanOutcome::NotInspected => "not inspected".to_owned(),
        ScanOutcome::Cancelled => "cancelled".to_owned(),
        ScanOutcome::Skipped => "skipped".to_owned(),
        ScanOutcome::Blocked => "blocked by policy".to_owned(),
        ScanOutcome::UnsupportedArchitecture => "unsupported architecture".to_owned(),
        ScanOutcome::InvalidBundle => "invalid bundle".to_owned(),
        ScanOutcome::Failed(message) => format!("failed: {message}"),
        ScanOutcome::Crashed(status) => format!("crashed: {status}"),
        ScanOutcome::TimedOut => "timed out".to_owned(),
        other => format!("{other:?}"),
    }
}

fn kind(outcome: &ScanOutcome) -> &'static str {
    match outcome {
        ScanOutcome::Found(_) => "found",
        ScanOutcome::NotInspected => "not inspected",
        ScanOutcome::Cancelled => "cancelled",
        ScanOutcome::Skipped => "skipped",
        ScanOutcome::Blocked => "blocked by policy",
        ScanOutcome::UnsupportedArchitecture => "unsupported architecture",
        ScanOutcome::InvalidBundle => "invalid bundle",
        ScanOutcome::Failed(_) => "failed",
        ScanOutcome::Crashed(_) => "crashed",
        ScanOutcome::TimedOut => "timed out",
        _ => "other",
    }
}

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [helper, cache, directories @ ..] = args.as_slice() else {
        out!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut scanner = Scanner::new(helper, cache);
    if let [flag, bundle] = directories
        && flag.as_os_str() == "--retry"
    {
        return match scanner.retry(bundle) {
            Ok(scan) => {
                out!(
                    "{:?} | {} | {}",
                    scan.source,
                    label(&scan.outcome),
                    scan.path.display()
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                out!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    if let [flag] = directories
        && flag.as_os_str() == "--audio-units"
    {
        let catalog = scanner.directories(Vec::new()).discover();
        let ScanOutcome::Found(units) = &catalog.audio_units else {
            out!("{}", label(&catalog.audio_units));
            return ExitCode::FAILURE;
        };
        for unit in units {
            out!(
                "{} | {} | {} | {} | {:?}",
                unit.class_id,
                unit.vendor,
                unit.name,
                unit.sdk_version,
                unit.kind
            );
        }
        return ExitCode::SUCCESS;
    }
    let metadata_only = directories
        .first()
        .is_some_and(|flag| flag.as_os_str() == "--discover");
    let directories = if metadata_only {
        &directories[1..]
    } else {
        directories
    };
    if !directories.is_empty() {
        scanner = scanner.directories(directories.to_vec());
    }
    let started = Instant::now();
    let progress = |scan: &BundleScan| {
        out!(
            "{:?} | {}{} | {}",
            scan.source,
            label(&scan.outcome),
            if scan.excluded { " (excluded)" } else { "" },
            scan.path.display()
        );
    };
    let result = if metadata_only {
        let catalog = scanner.discover();
        for bundle in &catalog.bundles {
            progress(bundle);
        }
        Ok(catalog)
    } else {
        scanner.scan(&progress)
    };
    let catalog = match result {
        Ok(catalog) => catalog,
        Err(error) => {
            out!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_source: BTreeMap<String, usize> = BTreeMap::new();
    for scan in &catalog.bundles {
        *by_kind.entry(kind(&scan.outcome)).or_default() += 1;
        *by_source.entry(format!("{:?}", scan.source)).or_default() += 1;
    }
    out!(
        "bundles {} | plugins {} | {:.1} s",
        catalog.bundles.len(),
        catalog.plugins().len(),
        started.elapsed().as_secs_f64()
    );
    out!("outcomes {by_kind:?}");
    out!("sources {by_source:?}");
    ExitCode::SUCCESS
}
