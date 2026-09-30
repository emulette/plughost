#!/bin/sh
# Packages an executable as a background (LSUIElement) .app and signs it like the plughost
# helper: ad-hoc, Hardened Runtime, with helper.entitlements.
# usage: package-app.sh <executable> <app path> <bundle identifier>
set -eu
here=$(cd "$(dirname "$0")" && pwd)
executable=$1
app=$2
identifier=$3
name=$(basename "$executable")
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
sed -e "s/@EXECUTABLE@/$name/g" -e "s/@IDENTIFIER@/$identifier/g" "$here/Info.plist" > "$app/Contents/Info.plist"
cp "$executable" "$app/Contents/MacOS/$name"
codesign --force --sign - --options runtime --entitlements "$here/helper.entitlements" "$app"
