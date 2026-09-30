#!/bin/sh
# Builds the VST3 and CLAP plugins the integration tests load into target/test-plugins:
#   plughost-test-delay.{vst3,clap}  from test-plugins/delay, and its faulty variants
#                                    plughost-test-{crash,hang}-{in-process,on-scan}, -noop-reset,
#                                    -latency-overflow, -large-state, -stall-main-thread,
#                                    -arrangement-false
#                                    (the same binary; the plugin picks its variant from its
#                                    bundle name)
#   plughost-test-synth.clap         from test-plugins/synth
#   again.vst3, again-simple.vst3, adelay.vst3, note-expression-synth.vst3, host-checker.vst3,
#   utf16-name.vst3                  VST3 SDK examples (MIT); VST3_SDK_DIR reuses a complete SDK
# For macOS; use build-test-plugins.ps1 on Windows.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/target/test-plugins"
mkdir -p "$out"

# package <binary> <name> <extension>: wraps a built dylib as a signed <name>.<extension> bundle
package() {
    path="$out/$2.$3"
    rm -rf "$path"
    mkdir -p "$path/Contents/MacOS"
    cp "$1" "$path/Contents/MacOS/$2"
    cat > "$path/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$2</string>
    <key>CFBundleIdentifier</key>
    <string>com.studio.plughost.$2.$3</string>
    <key>CFBundlePackageType</key>
    <string>BNDL</string>
</dict>
</plist>
PLIST
    codesign --force --sign - "$path"
}

cargo build --release --locked -p plughost-test-delay --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
for variant in delay crash-in-process hang-in-process crash-on-scan hang-on-scan noop-reset \
    latency-overflow large-state stall-main-thread arrangement-false; do
    for extension in vst3 clap; do
        package "$root/target/release/libplughost_test_delay.dylib" "plughost-test-$variant" "$extension"
    done
done
cargo build --release --locked -p plughost-test-synth --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
package "$root/target/release/libplughost_test_synth.dylib" plughost-test-synth clap

cargo build --release --locked -p plughost-test-presets --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
package "$root/target/release/libplughost_test_presets.dylib" plughost-test-presets clap

cargo build --release --locked -p plughost-test-routing --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
for extension in vst3 clap; do
    package "$root/target/release/libplughost_test_routing.dylib" plughost-test-routing "$extension"
done

vst3sdk="$root/.vst3sdk"
sdk="${VST3_SDK_DIR:-$vst3sdk/VST_SDK/vst3sdk}"
if [ -z "${VST3_SDK_DIR:-}" ] && [ ! -f "$sdk/CMakeLists.txt" ]; then
    # A cached archive is reused only while it matches the pinned hash; anything else, such as an
    # interrupted download, is replaced.
    mkdir -p "$vst3sdk"
    archive="$vst3sdk/vst-sdk.zip"
    sha256=64965f1b74e08a6d4087a35af7a716f4dcff5852c66ad7ee13f1c47e79c1ab77
    if [ ! -f "$archive" ] || [ "$(shasum -a 256 "$archive" | cut -d ' ' -f 1)" != "$sha256" ]; then
        curl --fail --location --output "$archive.part" https://download.steinberg.net/sdk_downloads/vst-sdk_3.8.1_build-84_2026-08-11.zip
        if [ "$(shasum -a 256 "$archive.part" | cut -d ' ' -f 1)" != "$sha256" ]; then
            rm -f "$archive.part"
            echo 'The downloaded VST3 SDK archive does not match the pinned SHA256.' >&2
            exit 1
        fi
        mv "$archive.part" "$archive"
    fi
    ditto -x -k "$archive" "$vst3sdk"
fi
build="$vst3sdk/build-macos"
cmake -S "$sdk" -B "$build" -G Xcode -DSMTG_CREATE_PLUGIN_LINK=OFF -DSMTG_RUN_VST_VALIDATOR=OFF > /dev/null
xcodebuild -project "$build/vstsdk.xcodeproj" -configuration Release -target again \
    -target again-simple -target adelay -target note-expression-synth -target host-checker \
    -target utf16-name -quiet
for plugin in again again-simple adelay note-expression-synth host-checker utf16-name; do
    rm -rf "$out/$plugin.vst3"
    cp -R "$build/VST3/Release/$plugin.vst3" "$out/$plugin.vst3"
    codesign --force --sign - "$out/$plugin.vst3"
done
echo "$out"
