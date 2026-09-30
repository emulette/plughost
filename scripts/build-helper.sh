#!/bin/sh
# Builds the helper example as target/helper/PlughostHelper.app and prints the executable path.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cargo build --release --locked -p plughost-helper --example helper --manifest-path "$root/Cargo.toml" --target-dir "$root/target" >&2
mkdir -p "$root/target/helper"
cp "$root/target/release/examples/helper" "$root/target/helper/plughost-helper"
app="$root/target/helper/PlughostHelper.app"
"$root/scripts/macos/package-app.sh" "$root/target/helper/plughost-helper" "$app" com.studio.plughost.helper >&2
echo "$app/Contents/MacOS/plughost-helper"
