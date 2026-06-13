$ErrorActionPreference = "Stop"

$python = $null
foreach ($candidate in @("py -3", "python", "python3")) {
    $parts = $candidate.Split(" ")
    $exe = $parts[0]
    $extra = @()
    if ($parts.Count -gt 1) {
        $extra = $parts[1..($parts.Count - 1)]
    }

    $cmd = Get-Command $exe -ErrorAction SilentlyContinue
    if ($cmd) {
        $python = @($cmd.Source) + $extra
        break
    }
}

if (-not $python) {
    Write-Error "Python 3 is required. Install it from https://www.python.org/downloads/windows/ or the Microsoft Store, then rerun this installer."
    exit 1
}

$script = Join-Path $PSScriptRoot "install.py"
if ($python.Count -gt 1) {
    & $python[0] @($python[1..($python.Count - 1)]) $script @args
} else {
    & $python[0] $script @args
}
exit $LASTEXITCODE
