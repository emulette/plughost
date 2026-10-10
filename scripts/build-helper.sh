#!/bin/sh
# Builds the helper example as target/helper/PlughostHelper.app and prints the executable path.
# With --audit it builds the real-time audit helper there instead (see plughost_core::audit),
# with RealtimeSanitizer, which the build downloads, watching its processing thread.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
features=
if [ "${1:-}" = "--audit" ]; then
    features="--features realtime-audit"
    export RTSAN_ENABLE=1
fi
# shellcheck disable=SC2086 # $features is empty or one option and its value.
cargo build --release --locked -p plughost-helper --example helper $features --manifest-path "$root/Cargo.toml" --target-dir "$root/target" >&2
mkdir -p "$root/target/helper"
cp "$root/target/release/examples/helper" "$root/target/helper/plughost-helper"
app="$root/target/helper/PlughostHelper.app"
if [ -n "$features" ]; then
    runtime=$(ls -t "$root"/target/release/build/rtsan-standalone-sys-*/out/libclang_rt.rtsan_osx_dynamic.dylib | head -1)
    "$root/scripts/macos/package-app.sh" "$root/target/helper/plughost-helper" "$app" com.studio.plughost.helper "$runtime" >&2
else
    "$root/scripts/macos/package-app.sh" "$root/target/helper/plughost-helper" "$app" com.studio.plughost.helper >&2
fi
echo "$app/Contents/MacOS/plughost-helper"
