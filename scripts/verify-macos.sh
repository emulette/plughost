#!/bin/sh
# macOS counterpart of verify-windows.ps1: builds the test assets and helper, then runs every test,
# formatting, Clippy, documentation and dependency check. Stops at the first failure.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
run() {
    echo "> $*"
    "$@"
}
run scripts/build-test-plugins.sh
run scripts/build-helper.sh
# Benchmarks are ignored tests too; they run on their own, in release mode. Moving a dialog in
# from another display needs that display, which PLUGHOST_TEST_DISPLAY names; CI runners have one.
skip_display=
if [ -z "${PLUGHOST_TEST_DISPLAY:-}" ]; then
    echo "PLUGHOST_TEST_DISPLAY is unset: skipping a_dialog_opened_on_another_display_moves_into_the_region"
    skip_display="--skip a_dialog_opened_on_another_display_moves_into_the_region"
fi
# shellcheck disable=SC2086 # the skip is two words or none
run cargo test --workspace --locked -- --include-ignored --skip benchmark $skip_display
run cargo fmt --all -- --check
run cargo clippy --workspace --all-targets --locked -- -D warnings
export RUSTDOCFLAGS='-D warnings'
run cargo doc --workspace --no-deps --locked
unset RUSTDOCFLAGS
run cargo deny --locked --all-features check licenses bans sources
