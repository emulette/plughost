use super::*;
use plughost_core::PluginKind;

fn class(format: PluginFormat, id: &str) -> PluginInfo {
    PluginInfo::new(PluginId::new(format, id), id, PluginKind::Effect)
}

fn bundle(path: &str, outcome: ScanOutcome) -> BundleScan {
    BundleScan {
        path: path.into(),
        outcome,
        source: Source::Cache,
        failures: 0,
        excluded: false,
    }
}

const DELAY: &str = "706C7567686F737444656C6179000001";

fn catalog() -> Catalog {
    Catalog {
        bundles: vec![
            bundle(
                "first/delay.vst3",
                ScanOutcome::Found(vec![class(PluginFormat::Vst3, DELAY)]),
            ),
            bundle("blocked/delay.vst3", ScanOutcome::Blocked),
            bundle(
                "second/delay.vst3",
                ScanOutcome::Found(vec![class(PluginFormat::Vst3, &DELAY.to_ascii_lowercase())]),
            ),
            bundle(
                "second/delay.clap",
                ScanOutcome::Found(vec![class(PluginFormat::Clap, "com.example.delay")]),
            ),
        ],
        audio_units: ScanOutcome::Found(vec![class(
            PluginFormat::AudioUnit,
            "6175667864656C796170706C",
        )]),
    }
}

#[test]
fn one_class_is_listed_once_from_the_highest_priority_bundle() {
    let catalog = catalog();
    let plugins: Vec<PluginRef> = catalog
        .plugins()
        .into_iter()
        .map(|(plugin, _)| plugin)
        .collect();
    assert_eq!(
        plugins,
        [
            PluginRef::vst3("first/delay.vst3", DELAY),
            PluginRef::clap("second/delay.clap", "com.example.delay"),
            PluginRef::audio_unit("6175667864656C796170706C"),
        ]
    );
}

#[test]
fn a_stored_id_resolves_to_where_the_class_is_installed() {
    let catalog = catalog();
    assert_eq!(
        catalog.resolve(&PluginId::new(
            PluginFormat::Vst3,
            DELAY.to_ascii_lowercase()
        )),
        Some(PluginRef::vst3("first/delay.vst3", DELAY))
    );
    assert_eq!(
        catalog.resolve(&PluginId::new(
            PluginFormat::AudioUnit,
            "6175667864656c796170706c"
        )),
        Some(PluginRef::audio_unit("6175667864656C796170706C"))
    );
    // CLAP IDs keep their case, so another case names another class.
    assert_eq!(
        catalog.resolve(&PluginId::new(PluginFormat::Clap, "com.example.Delay")),
        None
    );
}

#[test]
fn a_bundle_without_classes_has_no_references() {
    assert!(
        bundle("blocked/delay.vst3", ScanOutcome::Blocked)
            .plugins()
            .is_empty()
    );
}
