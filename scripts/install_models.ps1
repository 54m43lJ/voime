[CmdletBinding()]
param(
    [string]$ModelDir = (Join-Path $env:LOCALAPPDATA 'voime\models'),
    [string]$ModelRoot = (Join-Path $env:LOCALAPPDATA 'voime\models'),
    [ValidateSet('fire-red-ctc', 'sense-voice')]
    [string]$Asr = 'fire-red-ctc',
    [ValidateSet('itn', 'punc')]
    [string]$Variant = 'itn',
    [string]$CacheDir = (Join-Path $env:LOCALAPPDATA 'voime\cache'),
    [string]$Python = '',
    [switch]$Activate,
    [switch]$NoQuantize,
    [switch]$KeepStage
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Requirements = Join-Path $Root 'tools\requirements-export.txt'
$Venv = Join-Path $CacheDir 'venv'
$VenvPython = Join-Path $Venv 'Scripts\python.exe'

if (-not $ModelDir) { $ModelDir = Join-Path $env:LOCALAPPDATA 'voime\models' }
if (-not $CacheDir) { $CacheDir = Join-Path $env:LOCALAPPDATA 'voime\cache' }

$PythonPrefix = @()
if (-not $Python) {
    $command = Get-Command python -ErrorAction SilentlyContinue
    if ($command) {
        $Python = $command.Source
    }
    else {
        $command = Get-Command py -ErrorAction SilentlyContinue
        if (-not $command) {
            throw 'Python 3.10+ was not found; install Python or pass -Python <path>'
        }
        $Python = $command.Source
        $PythonPrefix = @('-3')
    }
}

function Invoke-Python {
    param([string[]]$Arguments)
    & $Python @PythonPrefix @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "python failed with exit code $LASTEXITCODE"
    }
}

Invoke-Python @('-c', 'import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else 1)')

if ($Asr -eq 'sense-voice') {
    $senseArgs = @(
        (Join-Path $Root 'tools\install_models.py'),
        '--asr', 'sense-voice',
        '--variant', $Variant,
        '--model-root', $ModelRoot,
        '--cache-dir', $CacheDir
    )
    if ($PSBoundParameters.ContainsKey('ModelDir')) {
        $senseArgs += @('--model-dir', $ModelDir)
    }
    if ($Activate) { $senseArgs += '--activate' }
    & $Python @PythonPrefix @senseArgs
    exit $LASTEXITCODE
}

New-Item -ItemType Directory -Force -Path $CacheDir | Out-Null

if (-not (Test-Path -LiteralPath $VenvPython)) {
    Write-Host "install_models: creating virtual environment $Venv"
    Invoke-Python @('-m', 'venv', $Venv)
}

$requirementHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Requirements).Hash
$marker = Join-Path $Venv '.voime-requirements'
$currentHash = ''
if (Test-Path -LiteralPath $marker) {
    $currentHash = (Get-Content -LiteralPath $marker -Raw).Trim()
}
if ($currentHash -ne $requirementHash) {
    Write-Host 'install_models: installing export dependencies'
    & $VenvPython -m pip install --upgrade pip
    if ($LASTEXITCODE -ne 0) { throw 'pip upgrade failed' }
    & $VenvPython -m pip install -r $Requirements
    if ($LASTEXITCODE -ne 0) { throw 'pip install failed' }
    Set-Content -LiteralPath $marker -Value $requirementHash -Encoding ASCII
}

$arguments = @(
    (Join-Path $Root 'tools\install_models.py'),
    '--asr', 'fire-red-ctc',
    '--model-root', $ModelRoot,
    '--model-dir', $ModelDir,
    '--cache-dir', $CacheDir
)
if ($Activate) { $arguments += '--activate' }
if ($NoQuantize) { $arguments += '--no-quantize' }
if ($KeepStage) { $arguments += '--keep-stage' }

& $VenvPython @arguments
if ($LASTEXITCODE -ne 0) {
    throw "model installation failed with exit code $LASTEXITCODE"
}
