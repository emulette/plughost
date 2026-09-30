//! The system's registered Audio Units, read without loading them, and their class IDs.

use std::ptr::NonNull;

use objc2_audio_toolbox::{
    AudioComponent, AudioComponentCopyName, AudioComponentDescription, AudioComponentFindNext,
    AudioComponentFlags, AudioComponentGetDescription, AudioComponentGetVersion,
    kAudioUnitType_Effect, kAudioUnitType_MIDIProcessor, kAudioUnitType_MusicDevice,
    kAudioUnitType_MusicEffect,
};
use objc2_core_foundation::CFString;
use plughost_core::{PluginFormat, PluginInfo, PluginKind};

use super::errors::AuError;

/// Audio effects, MIDI-controlled audio effects, instruments, and MIDI-only effects.
const TYPES: [u32; 4] = [
    kAudioUnitType_Effect,
    kAudioUnitType_MusicEffect,
    kAudioUnitType_MusicDevice,
    kAudioUnitType_MIDIProcessor,
];

/// The registered Audio Units the host can use.
pub fn components() -> Vec<PluginInfo> {
    let mut found = Vec::new();
    for component_type in TYPES {
        let mut filter = description(component_type, 0, 0);
        let mut component: AudioComponent = std::ptr::null_mut();
        loop {
            // SAFETY: the filter is a valid description; a null component starts the search.
            component = unsafe { AudioComponentFindNext(component, NonNull::from(&mut filter)) };
            if component.is_null() {
                break;
            }
            if let Some(info) = info(component) {
                found.push(info);
            }
        }
    }
    found
}

fn info(component: AudioComponent) -> Option<PluginInfo> {
    let mut description = description(0, 0, 0);
    if unsafe { AudioComponentGetDescription(component, NonNull::from(&mut description)) } != 0 {
        return None;
    }
    let mut name: *const CFString = std::ptr::null();
    unsafe { AudioComponentCopyName(component, NonNull::from(&mut name)) };
    let full_name = NonNull::new(name.cast_mut())
        // SAFETY: AudioComponentCopyName returns a +1 reference.
        .map(|name| unsafe { objc2_core_foundation::CFRetained::from_raw(name) }.to_string())
        .unwrap_or_default();
    // Registered names read "Manufacturer: Name".
    let (vendor, name) = full_name
        .split_once(": ")
        .map_or((String::new(), full_name.clone()), |(v, n)| {
            (v.to_owned(), n.to_owned())
        });
    let mut version = 0u32;
    unsafe { AudioComponentGetVersion(component, NonNull::from(&mut version)) };
    let instrument = description.componentType == kAudioUnitType_MusicDevice;
    let v3 = AudioComponentFlags(description.componentFlags)
        .contains(AudioComponentFlags::IsV3AudioUnit);
    Some(PluginInfo {
        format: PluginFormat::AudioUnit,
        class_id: class_id(&description),
        name,
        vendor,
        version: format!(
            "{}.{}.{}",
            version >> 16,
            (version >> 8) & 0xFF,
            version & 0xFF
        ),
        sdk_version: if v3 { "AUv3" } else { "AUv2" }.to_owned(),
        kind: if instrument {
            PluginKind::Instrument
        } else {
            PluginKind::Effect
        },
        categories: vec![if instrument { "Instrument" } else { "Fx" }.to_owned()],
    })
}

pub(crate) fn description(
    component_type: u32,
    subtype: u32,
    manufacturer: u32,
) -> AudioComponentDescription {
    AudioComponentDescription {
        componentType: component_type,
        componentSubType: subtype,
        componentManufacturer: manufacturer,
        componentFlags: 0,
        componentFlagsMask: 0,
    }
}

/// Type, subtype, and manufacturer codes as 24 hexadecimal digits.
pub(crate) fn class_id(description: &AudioComponentDescription) -> String {
    format!(
        "{:08X}{:08X}{:08X}",
        description.componentType, description.componentSubType, description.componentManufacturer
    )
}

pub(crate) fn parse_class_id(class_id: &str) -> Result<AudioComponentDescription, AuError> {
    let invalid = || AuError::ClassId(class_id.to_owned());
    if class_id.len() != 24 || !class_id.is_ascii() {
        return Err(invalid());
    }
    let code =
        |i: usize| u32::from_str_radix(&class_id[i * 8..i * 8 + 8], 16).map_err(|_| invalid());
    Ok(description(code(0)?, code(1)?, code(2)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_processor_registration_is_visible_without_instantiation() {
        use objc2_audio_toolbox::{
            AudioComponentPlugInInterface, AudioComponentRegister, kAudioUnitType_MIDIProcessor,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        static INSTANTIATIONS: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "C-unwind" fn factory(
            _: NonNull<AudioComponentDescription>,
        ) -> *mut AudioComponentPlugInInterface {
            INSTANTIATIONS.fetch_add(1, Ordering::Relaxed);
            std::ptr::null_mut()
        }
        let mut desc = description(
            kAudioUnitType_MIDIProcessor,
            u32::from_be_bytes(*b"PgM1"),
            u32::from_be_bytes(*b"PgHt"),
        );
        let id = class_id(&desc);
        assert!(!components().iter().any(|info| info.class_id == id));
        let component = unsafe {
            AudioComponentRegister(
                NonNull::from(&mut desc),
                &CFString::from_str("plughost: MIDI registration fixture"),
                0x010203,
                Some(factory),
            )
        };
        assert!(!component.is_null());
        let listed = components();
        let info = listed
            .iter()
            .find(|info| info.class_id == id)
            .expect("MIDI registration missing");
        assert_eq!(info.name, "MIDI registration fixture");
        assert_eq!(info.vendor, "plughost");
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.kind, PluginKind::Effect);
        assert_eq!(INSTANTIATIONS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn class_ids_round_trip() {
        // Apple's AUDelay: 'aufx' 'dely' 'appl'.
        let delay = description(0x61756678, 0x64656C79, 0x6170706C);
        let id = class_id(&delay);
        assert_eq!(id, "6175667864656C796170706C");
        let parsed = parse_class_id(&id).unwrap();
        assert_eq!(
            (
                parsed.componentType,
                parsed.componentSubType,
                parsed.componentManufacturer
            ),
            (0x61756678, 0x64656C79, 0x6170706C)
        );
        assert!(parse_class_id("aufx").is_err());
    }
}
