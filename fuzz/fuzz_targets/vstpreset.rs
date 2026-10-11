#![no_main]
use libfuzzer_sys::fuzz_target;
use plughost_formats::vst3::Preset;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(preset) = Preset::from_bytes(bytes) {
        let encoded = preset.to_bytes().unwrap();
        assert_eq!(Preset::from_bytes(&encoded).unwrap(), preset);
    }
});
