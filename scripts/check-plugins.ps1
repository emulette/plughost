# Runs the compatibility check for each class of each VST3 or CLAP bundle given, one process per
# class with a bounded lifetime. Logs go to target/check/results.
[CmdletBinding(PositionalBinding = $false)]
param(
    [Parameter(Mandatory, Position = 0, ValueFromRemainingArguments)]
    [string[]]$Plugins,
    [ValidateRange(1, 86400)]
    [int]$TimeoutSeconds = 180
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
& cargo build --release --locked -p plughost-formats --example check --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$results = "$root/target/check/results"
New-Item -ItemType Directory -Force $results | Out-Null
$failed = $false

# Runs the check binary with a bounded lifetime and returns its exit code, timeout, and output.
function Invoke-Check {
    param([string[]]$Arguments)
    $info = [System.Diagnostics.ProcessStartInfo]::new()
    $info.FileName = "$root/target/release/examples/check.exe"
    $info.Arguments = ($Arguments | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::Start($info)
    # Drain both pipes while it runs, including verbose third-party plugin logging.
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $timedOut = -not $process.WaitForExit($TimeoutSeconds * 1000)
    if ($timedOut) { $process.Kill() }
    $process.WaitForExit()
    $result = [pscustomobject]@{
        ExitCode = $process.ExitCode
        TimedOut = $timedOut
        Output   = $stdout.GetAwaiter().GetResult()
        Log      = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
    }
    $process.Dispose()
    $result
}

foreach ($bundle in $Plugins) {
    $name = [System.IO.Path]::GetFileName($bundle)
    # Listing loads the module, so it runs under the same timeout.
    $listing = Invoke-Check @($bundle, '--classes')
    if ($listing.TimedOut -or $listing.ExitCode -ne 0) {
        $failed = $true
        Write-Output "$name | exit $($listing.ExitCode) | timeout $($listing.TimedOut) | result: FAILED listing classes"
        continue
    }
    for ($index = 0; $index -lt [int]$listing.Output.Trim(); $index++) {
        $check = Invoke-Check @($bundle, "$index")
        [System.IO.File]::WriteAllText("$results/$name#$index.log", $check.Log)
        $result = ($check.Log -split '?
' | Where-Object { $_ -match '^result:' } | Select-Object -Last 1)
        if ($check.TimedOut -or $check.ExitCode -ne 0) { $failed = $true }
        Write-Output "$name#$index | exit $($check.ExitCode) | timeout $($check.TimedOut) | $result"
    }
}
if ($failed) { exit 1 }
