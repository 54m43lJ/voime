[CmdletBinding()]
param(
    [switch]$Gpu,
    [string]$ModelDir = (Join-Path $env:LOCALAPPDATA 'voime\models'),
    [string]$CacheDir = (Join-Path $env:LOCALAPPDATA 'voime\cache'),
    [string]$SourceDir = '',
    [string]$PuncSourceDir = '',
    [ValidateSet('v1', 'v2', 'auto')]
    [string]$AedVersion = 'v1',
    [string]$Python = ''
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
    '--model-root', $ModelDir,
    '--cache-dir', $CacheDir,
    '--aed-version', $AedVersion
)
if ($Gpu) {
    $arguments += '--gpu'
}
if ($SourceDir) {
    $arguments += @('--source-dir', $SourceDir)
}
if ($PuncSourceDir) {
    $arguments += @('--punc-source-dir', $PuncSourceDir)
}

& $Python @arguments
if ($LASTEXITCODE -ne 0) {
    throw "model installation failed with exit code $LASTEXITCODE"
}
