# Windows counterpart of build-helper.sh. Prints the helper executable path. With -Audit it builds
# the real-time audit helper instead, which counts the host's allocations on its processing
# thread (see plughost_core::audit).
param([switch]$Audit)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$features = if ($Audit) { @('--features', 'realtime-audit') } else { @() }
& cargo build --release --locked -p plughost-helper --example helper @features --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force "$root/target/helper" | Out-Null
Copy-Item -LiteralPath "$root/target/release/examples/helper.exe" -Destination "$root/target/helper/plughost-helper.exe"
Write-Output "$root/target/helper/plughost-helper.exe"
