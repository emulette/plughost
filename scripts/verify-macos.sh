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
run cargo test --workspace --locked -- --include-ignored
run cargo fmt --all -- --check
run cargo clippy --workspace --all-targets --locked -- -D warnings
export RUSTDOCFLAGS='-D warnings'
run cargo doc --workspace --no-deps --locked
unset RUSTDOCFLAGS
run cargo deny --locked --all-features check licenses bans sources
