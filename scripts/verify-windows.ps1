#requires -Version 7.0
# Windows counterpart of the macOS verification commands in README.md. Stops at the first failure.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

function Invoke-Step {
    param([string]$Program, [string[]]$Arguments)
    Write-Output "> $Program $($Arguments -join ' ')"
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Push-Location $root
try {
    Invoke-Step 'pwsh' @('-NoProfile', '-File', "$PSScriptRoot/build-test-plugins.ps1")
    Invoke-Step 'pwsh' @('-NoProfile', '-File', "$PSScriptRoot/build-helper.ps1")
    Invoke-Step 'cargo' @('test', '--workspace', '--locked', '--', '--include-ignored')
    Invoke-Step 'cargo' @('fmt', '--all', '--', '--check')
    Invoke-Step 'cargo' @('clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings')
    $env:RUSTDOCFLAGS = '-D warnings'
    Invoke-Step 'cargo' @('doc', '--workspace', '--no-deps', '--locked')
    Remove-Item Env:RUSTDOCFLAGS
    Invoke-Step 'cargo' @('deny', '--locked', '--all-features', 'check', 'licenses', 'bans', 'sources')
}
finally {
    Pop-Location
}
