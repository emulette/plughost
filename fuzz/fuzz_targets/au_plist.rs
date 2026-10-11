#![no_main]
#[cfg(not(target_os = "macos"))]
compile_error!("The AU property-list target requires macOS.");
use libfuzzer_sys::fuzz_target;
use plughost_formats::au::{from_preset, inspect_preset, to_preset};

fuzz_target!(|bytes: &[u8]| {
    objc2::rc::autoreleasepool(|_| {
        let inspected = inspect_preset(bytes);
        let decoded = from_preset(bytes);
        // Identity inspection does not guarantee that every opaque plist value can be encoded.
        if let Ok(state) = decoded {
            let info = inspected.unwrap();
            assert_eq!(inspect_preset(&state).unwrap(), info);
            if let Ok(xml) = to_preset(&state) {
                assert_eq!(inspect_preset(&xml).unwrap(), info);
            }
        }
    });
});
