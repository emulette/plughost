//! Entry points shared by parser corpus tests and coverage-guided fuzzing.
use super::{bundle, cache, moduleinfo};

/// Parses the real discovery JSON5 boundary without filesystem I/O.
pub fn moduleinfo(bytes: &[u8]) -> Option<Vec<plughost_core::PluginInfo>> {
    moduleinfo::parse(std::str::from_utf8(bytes).ok()?)
}

/// Decodes a native scan snapshot, including schema and architecture filtering.
pub fn cache(bytes: &[u8]) -> usize {
    cache::decode(bytes).bundles.len()
}

/// Reads the production Mach-O/PE header parser.
pub fn binary_header(bytes: &[u8]) -> bool {
    bundle::architectures(bytes).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bundle::Architecture::{Arm64, X86_64};

    #[test]
    fn corpus_moduleinfo_filters_non_audio_classes_and_rejects_invalid_metadata() {
        let classes = moduleinfo(include_bytes!("corpus/moduleinfo/audio.json5")).unwrap();
        assert_eq!(classes.len(), 1);
        let class = &classes[0];
        assert_eq!(class.class_id, "0123456789ABCDEF0123456789ABCDEF");
        assert_eq!(class.name, "Corpus synth");
        assert_eq!(class.vendor, "Corpus vendor");
        assert_eq!(class.categories, ["Instrument", "Synth"]);
        assert_eq!(class.kind, plughost_core::PluginKind::Instrument);
        for bytes in [
            &include_bytes!("corpus/moduleinfo/bad-cid.json5")[..],
            include_bytes!("corpus/moduleinfo/truncated.json5"),
            include_bytes!("corpus/moduleinfo/invalid-utf8"),
            include_bytes!("corpus/moduleinfo/deep.json5"),
        ] {
            assert!(moduleinfo(bytes).is_none());
        }
    }

    #[test]
    fn corpus_binary_headers_identify_slices_and_reject_truncated_tables() {
        for (bytes, expected) in [
            (
                &include_bytes!("corpus/binary_header/mach-arm64")[..],
                vec![Arm64],
            ),
            (
                &include_bytes!("corpus/binary_header/mach-fat")[..],
                vec![X86_64, Arm64],
            ),
            (
                &include_bytes!("corpus/binary_header/mach-fat64")[..],
                vec![Arm64],
            ),
            (
                &include_bytes!("corpus/binary_header/pe-amd64")[..],
                vec![X86_64],
            ),
            (
                &include_bytes!("corpus/binary_header/pe-arm64")[..],
                vec![Arm64],
            ),
        ] {
            assert!(binary_header(bytes));
            assert_eq!(bundle::architectures(bytes).unwrap(), expected);
        }
        for bytes in [
            &include_bytes!("corpus/binary_header/truncated-fat")[..],
            include_bytes!("corpus/binary_header/pe-outside"),
            include_bytes!("corpus/binary_header/mach-fat64-swapped"),
        ] {
            assert!(!binary_header(bytes));
        }
    }

    #[test]
    fn corpus_cache_checks_schema_architecture_and_native_outcome() {
        let arm = include_bytes!("corpus/cache/arm64.json");
        let x64 = include_bytes!("corpus/cache/x86_64.json");
        for (bytes, arch) in [(&arm[..], "aarch64"), (&x64[..], "x86_64")] {
            assert_eq!(cache(bytes), usize::from(std::env::consts::ARCH == arch));
            if std::env::consts::ARCH == arch {
                let decoded = cache::decode(bytes);
                let entry = decoded
                    .bundles
                    .get(std::path::Path::new("fixture.clap"))
                    .unwrap();
                assert_eq!(entry.failures, 2);
                assert_eq!(entry.outcome, super::super::ScanOutcome::TimedOut);
            }
        }
        for bytes in [
            &include_bytes!("corpus/cache/old-schema.json")[..],
            include_bytes!("corpus/cache/unknown-outcome.json"),
            include_bytes!("corpus/cache/truncated.json"),
        ] {
            assert_eq!(cache(bytes), 0);
        }
    }
}
