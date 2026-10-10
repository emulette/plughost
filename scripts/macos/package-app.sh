#!/bin/sh
# Packages an executable as a background (LSUIElement) .app with the workspace version and signs
# it like the plughost helper: ad-hoc, Hardened Runtime, with helper.entitlements. Libraries
# given after the identifier go next to the executable, which loads them from there.
# usage: package-app.sh <executable> <app path> <bundle identifier> [library...]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
executable=$1
app=$2
identifier=$3
shift 3
name=$(basename "$executable")
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/../../Cargo.toml" | head -1)
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
sed -e "s/@EXECUTABLE@/$name/g" -e "s/@IDENTIFIER@/$identifier/g" -e "s/@VERSION@/$version/g" \
    "$here/Info.plist" > "$app/Contents/Info.plist"
cp "$executable" "$app/Contents/MacOS/$name"
if [ $# -gt 0 ]; then
    install_name_tool -add_rpath @executable_path "$app/Contents/MacOS/$name"
    for library in "$@"; do
        cp "$library" "$app/Contents/MacOS/"
        codesign --force --sign - "$app/Contents/MacOS/$(basename "$library")"
    done
fi
codesign --force --sign - --options runtime --entitlements "$here/helper.entitlements" "$app"
