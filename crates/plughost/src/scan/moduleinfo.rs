//! The `Contents/Resources/moduleinfo.json` fast path: class information without loading the
//! binary. The SDK reads these files as JSON5, and so does this. A file that does not parse is
//! ignored during discovery, leaving native inspection to an explicit scan.

use std::fs;
use std::path::Path;

use plughost_core::{PluginFormat, PluginInfo, PluginKind};
use serde_json::Value;

const AUDIO_MODULE_CLASS: &str = "Audio Module Class";

pub fn read(bundle: &Path) -> Option<Vec<PluginInfo>> {
    if !bundle.extension()?.eq_ignore_ascii_case("vst3") {
        return None;
    }
    let path = bundle
        .join("Contents")
        .join("Resources")
        .join("moduleinfo.json");
    let text = fs::read_to_string(path).ok()?;
    parse(&text)
}

fn parse(text: &str) -> Option<Vec<PluginInfo>> {
    let root: Value = json5::from_str(text).ok()?;
    let factory_vendor = root["Factory Info"]["Vendor"].as_str().unwrap_or_default();
    root["Classes"]
        .as_array()?
        .iter()
        .filter(|class| class["Category"] == AUDIO_MODULE_CLASS)
        .map(|class| {
            let text = |key: &str| class[key].as_str().map(str::to_owned);
            let categories: Vec<String> = class["Sub Categories"]
                .as_array()
                .map(|all| {
                    all.iter()
                        .filter_map(|c| c.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let class_id = text("CID")?.to_ascii_uppercase();
            if class_id.len() != 32 || !class_id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            Some({
                let mut plugin_info = PluginInfo::new(
                    PluginFormat::Vst3,
                    class_id,
                    text("Name")?,
                    if categories.iter().any(|c| c == "Instrument") {
                        PluginKind::Instrument
                    } else {
                        PluginKind::Effect
                    },
                );
                plugin_info.vendor = text("Vendor")
                    .filter(|vendor| !vendor.is_empty())
                    .unwrap_or_else(|| factory_vendor.to_owned());
                plugin_info.version = text("Version").unwrap_or_default();
                plugin_info.sdk_version = text("SDKVersion").unwrap_or_default();
                plugin_info.categories = categories;
                plugin_info
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDK_STYLE: &str = r#"{
      "Name": "dxLevel",
      // written by the SDK's moduleinfotool
      "Factory Info": { "Vendor": "accentize", "URL": "a, b // not a comment", },
      "Classes": [
        {
          "CID": "abcdef019182faeb4163747a44784c76",
          "Category": "Audio Module Class",
          "Name": "dxLevel",
          "Vendor": "",
          "Version": "1.0.1",
          "SDKVersion": "VST 3.7.8",
          "Sub Categories": [ "Fx", "RoomFx", ],
        },
        { "CID": "ABCDEF011234ABCD4163747A44784C76", "Category": "Component Controller Class", "Name": "x", },
      ],
    }"#;

    #[test]
    fn sdk_style_files_parse_to_audio_classes() {
        let classes = parse(SDK_STYLE).unwrap();
        assert_eq!(classes.len(), 1);
        let class = &classes[0];
        assert_eq!(class.class_id, "ABCDEF019182FAEB4163747A44784C76");
        assert_eq!(class.name, "dxLevel");
        assert_eq!(class.vendor, "accentize");
        assert_eq!(class.categories, ["Fx", "RoomFx"]);
        assert_eq!(class.kind, PluginKind::Effect);
    }

    #[test]
    fn json5_syntax_beyond_comments_and_trailing_commas_is_accepted() {
        let classes = parse(
            r#"{ Classes: [{
                CID: 'abcdef019182faeb4163747a44784c76',
                Category: 'Audio Module Class',
                Name: 'x',
            }] }"#,
        )
        .unwrap();
        assert_eq!(classes[0].class_id, "ABCDEF019182FAEB4163747A44784C76");
    }

    #[test]
    fn unreadable_files_are_not_used() {
        assert_eq!(parse("{ Classes: [ }"), None);
        assert_eq!(
            parse(
                r#"{"Classes": [{"CID": "short", "Category": "Audio Module Class", "Name": "x"}]}"#
            ),
            None
        );
    }
}
