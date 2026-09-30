# Windows x64 assets for the same integration suite as build-test-plugins.sh.
# VST3_SDK_DIR can point to an existing complete SDK; otherwise use the official SDK archive.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$out = "$root/target/test-plugins"
New-Item -ItemType Directory -Force $out | Out-Null

# One binary; the plugin picks its variant from the name of the module the host loads.
& cargo build --release --locked -p plughost-test-delay --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
foreach ($variant in @('delay', 'crash-in-process', 'hang-in-process', 'crash-on-scan', 'hang-on-scan', 'noop-reset', 'latency-overflow', 'large-state', 'stall-main-thread', 'arrangement-false', 'restart-on-activate', 'timers')) {
    foreach ($extension in @('vst3', 'clap')) {
        Copy-Item -LiteralPath "$root/target/release/plughost_test_delay.dll" -Destination "$out/plughost-test-$variant.$extension"
    }
}
& cargo build --release --locked -p plughost-test-synth --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Copy-Item -LiteralPath "$root/target/release/plughost_test_synth.dll" -Destination "$out/plughost-test-synth.clap"

& cargo build --release --locked -p plughost-test-presets --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Copy-Item -LiteralPath "$root/target/release/plughost_test_presets.dll" -Destination "$out/plughost-test-presets.clap"

& cargo build --release --locked -p plughost-test-routing --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
foreach ($extension in @('vst3', 'clap')) {
    Copy-Item -LiteralPath "$root/target/release/plughost_test_routing.dll" -Destination "$out/plughost-test-routing.$extension"
}

$vst3sdk = "$root/.vst3sdk"
$sdk = $env:VST3_SDK_DIR
if (-not $sdk) {
    $sdk = "$vst3sdk/VST_SDK/vst3sdk"
    if (-not (Test-Path -LiteralPath "$sdk/CMakeLists.txt")) {
        New-Item -ItemType Directory -Force $vst3sdk | Out-Null
        $archive = "$vst3sdk/vst-sdk.zip"
        $url = 'https://download.steinberg.net/sdk_downloads/vst-sdk_3.8.1_build-84_2026-08-11.zip'
        # A cached archive is reused only while it matches the pinned hash; anything else, such
        # as an interrupted download, is replaced.
        $hash = '64965F1B74E08A6D4087A35AF7A716F4DCFF5852C66AD7EE13F1C47E79C1AB77'
        if (-not (Test-Path -LiteralPath $archive) -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $hash) {
            Invoke-WebRequest -Uri $url -OutFile "$archive.part"
            if ((Get-FileHash -LiteralPath "$archive.part" -Algorithm SHA256).Hash -ne $hash) {
                Remove-Item -LiteralPath "$archive.part"
                throw 'The downloaded VST3 SDK archive does not match the pinned SHA256.'
            }
            Move-Item -LiteralPath "$archive.part" -Destination $archive -Force
        }
        Expand-Archive -LiteralPath $archive -DestinationPath $vst3sdk -Force
    }
}
$build = "$vst3sdk/build-windows"
& cmake -S $sdk -B $build -A x64 -DSMTG_CREATE_PLUGIN_LINK=OFF -DSMTG_RUN_VST_VALIDATOR=OFF
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& cmake --build $build --config Release --target again again-simple adelay note-expression-synth host-checker utf16-name --parallel 4
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
foreach ($name in @('again', 'again-simple', 'adelay', 'note-expression-synth', 'host-checker', 'utf16-name')) {
    Copy-Item -LiteralPath "$build/VST3/Release/$name.vst3" -Destination $out -Recurse -Force
}
Write-Output $out
