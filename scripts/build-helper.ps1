# Windows counterpart of build-helper.sh. Prints the helper executable path.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
& cargo build --release --locked -p plughost-helper --example helper --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force "$root/target/helper" | Out-Null
Copy-Item -LiteralPath "$root/target/release/examples/helper.exe" -Destination "$root/target/helper/plughost-helper.exe"
Write-Output "$root/target/helper/plughost-helper.exe"
