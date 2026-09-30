#!/bin/sh
# Runs the compatibility check (crates/plughost-formats/examples/check.rs) for each class of each
# VST3 or CLAP bundle, and each Audio Unit (au:<class ID>) given, one process per class with a
# timeout. The check
# binary is packaged and signed like the helper: a minimal LSUIElement .app, Hardened Runtime,
# disable-library-validation, and allow-unsigned-executable-memory. Logs go to
# target/check/results. Exits non-zero when any check fails, crashes, or times out.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cargo build --release --locked -p plughost-formats --example check --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
mkdir -p "$root/target/check"
cp "$root/target/release/examples/check" "$root/target/check/plughost-check"
app="$root/target/check/PlughostCheck.app"
"$root/scripts/macos/package-app.sh" "$root/target/check/plughost-check" "$app" com.studio.plughost.check
results="$root/target/check/results"
timeout="${CHECK_TIMEOUT:-180}"
mkdir -p "$results"
failed=0

# check <label> <arguments...>: runs one check process and reports its result. POSIX sh has no
# local variables, so the label must not reuse the caller's `name`.
check() {
    label=$1
    shift
    log="$results/$label.log"
    status=0
    perl -e 'alarm shift; exec @ARGV' "$timeout" "$app/Contents/MacOS/plughost-check" "$@" > "$log" 2>&1 || status=$?
    result=$(grep '^result:' "$log" | tail -1)
    case $status in
        0 | 1) ;;
        142) result="result: TIMEOUT after ${timeout}s" ;;
        *) result="result: CRASHED (exit $status) ${result}" ;;
    esac
    [ "$status" -eq 0 ] || failed=1
    printf '%s | %s
' "$label" "$result"
}

for bundle in "$@"; do
    name=$(basename "$bundle" | tr ':' '-')
    case $bundle in
        au:*) check "$name" "$bundle"; continue ;;
    esac
    # Listing loads the module, so it runs under the same timeout.
    count=$(perl -e 'alarm shift; exec @ARGV' "$timeout" "$app/Contents/MacOS/plughost-check" "$bundle" --classes 2> /dev/null) || {
        failed=1
        printf '%s | result: FAILED listing classes
' "$name"
        continue
    }
    index=0
    while [ "$index" -lt "$count" ]; do
        check "$name#$index" "$bundle" "$index"
        index=$((index + 1))
    done
done
exit "$failed"
