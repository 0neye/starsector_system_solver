# Thin shim: the installer now lives in the binary itself.
# Runs `system_solver install` from this unpacked release archive.
$ErrorActionPreference = "Stop"

$binary = Join-Path $PSScriptRoot "system_solver.exe"
if (-not (Test-Path $binary)) {
    Write-Error "system_solver.exe not found next to this script; run it from inside the unpacked release archive."
    exit 1
}

& $binary install @args
exit $LASTEXITCODE
