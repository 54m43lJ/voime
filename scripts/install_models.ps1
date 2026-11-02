[CmdletBinding()]
param(
    [ValidateSet('sensevoice', 'firered')]
    [string]$Profile = 'sensevoice',
    [string]$ModelDir = (Join-Path $env:LOCALAPPDATA 'voime\models'),
    [string]$CacheDir = (Join-Path $env:LOCALAPPDATA 'voime\cache'),
    [string]$SourceDir = '',
    [string]$SupportDir = '',
    [ValidateSet('v1', 'v2', 'auto')]
    [string]$AedVersion = 'v1',
    [string]$Python = '',
    [switch]$Activate
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot

if (-not $Python) {
    $command = Get-Command python -ErrorAction SilentlyContinue
    if ($command) {
        $Python = $command.Source
    }
    else {
        throw 'Python 3.10+ was not found; install Python or pass -Python <path>'
    }
}

& $Python -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else 1)'
if ($LASTEXITCODE -ne 0) {
    throw 'Python 3.10+ is required'
}

$arguments = @(
    (Join-Path $Root 'tools\install_models.py'),
    '--profile', $Profile,
    '--model-root', $ModelDir,
    '--cache-dir', $CacheDir
)
if ($SourceDir) {
    $arguments += @('--source-dir', $SourceDir)
}
if ($SupportDir) {
    $arguments += @('--support-dir', $SupportDir)
}
$arguments += @('--aed-version', $AedVersion)
if ($Activate) {
    $arguments += '--activate'
}

& $Python @arguments
if ($LASTEXITCODE -ne 0) {
    throw "model installation failed with exit code $LASTEXITCODE"
}
