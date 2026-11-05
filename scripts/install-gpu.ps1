param(
    [switch]$Qwen3,
    [ValidateSet('vulkan', 'cuda')]
    [string]$QwenRuntime = 'vulkan',
    [ValidateSet('cpu', 'qwen3')]
    [string]$Backend = '',
    [switch]$NoModels,
    [string]$ModelDir = (Join-Path $env:LOCALAPPDATA 'voime\models'),
    [string]$SourceDir = '',
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\Voime'),
    [switch]$Yes,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$BinDir = $Prefix

function Find-Python {
    $python = Get-Command python3 -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $python) {
        $python = Get-Command python -ErrorAction SilentlyContinue | Select-Object -First 1
    }
    if (-not $python) {
        throw 'python3 is required to install GPU models; install Python or pass -NoModels'
    }
    return $python.Source
}

if ($Uninstall) {
    $RuntimeDir = Join-Path $Prefix 'lib\voime'
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $BinDir 'voime-gpu-qwen3.exe')
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $RuntimeDir 'qwen3')
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $ModelDir 'qwen3-asr-1.7b')
    Write-Host 'GPU backend removed; final.backend is unchanged.'
    exit 0
}

if (-not (Test-Path (Join-Path $BinDir 'voimed.exe')) -and
    -not (Test-Path (Join-Path $env:LOCALAPPDATA 'Programs\Voime\voimed.exe'))) {
    throw 'core install not found; install core before the GPU module'
}

$installModels = -not $NoModels
$python = $null
if ($installModels) { $python = Find-Python }

$prebuilt = if ([string]::IsNullOrEmpty($SourceDir)) { '' } else { Join-Path $SourceDir 'bin\voime-gpu-qwen3.exe' }
if (-not [string]::IsNullOrEmpty($prebuilt) -and (Test-Path $prebuilt)) {
    Write-Host 'Installing precompiled Qwen worker...'
    Copy-Item -Force $prebuilt $BinDir
} else {
    Write-Host "Building Qwen worker ($QwenRuntime)..."
    Push-Location (Join-Path $Root 'crates\voime-gpu-qwen3')
    try {
        cargo build --release --features $QwenRuntime
        if ($LASTEXITCODE -ne 0) { throw 'Qwen worker build failed' }
    } finally { Pop-Location }
    Copy-Item -Force (Join-Path $Root 'crates\voime-gpu-qwen3\target\release\voime-gpu-qwen3.exe') $BinDir
}

if ($installModels) {
    $modelArgs = @(
        (Join-Path $Root 'scripts\install-gpu-models.py'),
        '--model-root', $ModelDir,
        '--qwen3'
    )
    if (-not [string]::IsNullOrEmpty($SourceDir)) {
        $resolvedSourceDir = (Resolve-Path -LiteralPath $SourceDir).Path
        $modelArgs += @('--source-dir', $resolvedSourceDir, '--no-download')
    }
    if ($Yes) { $modelArgs += '--yes' }
    & $python @modelArgs
    if ($LASTEXITCODE -ne 0) { throw 'GPU model installation failed' }
}

function Set-FinalBackend {
    param([string]$Config, [string]$Backend)
    $lines = @()
    if (Test-Path $Config) { $lines = Get-Content $Config }
    $out = @()
    $inFinal = $false
    $written = $false
    $hasFinal = $false
    foreach ($line in $lines) {
        if ($line -match '^\s*\[final\]\s*$') {
            if ($inFinal -and -not $written) {
                $out += "backend = `"$Backend`""
                $written = $true
            }
            $inFinal = $true
            $hasFinal = $true
            $out += $line
            continue
        }
        if ($line -match '^\s*\[') {
            if ($inFinal -and -not $written) {
                $out += "backend = `"$Backend`""
                $written = $true
            }
            $inFinal = $false
            $out += $line
            continue
        }
        if ($inFinal -and $line -match '^\s*backend\s*=') {
            if (-not $written) {
                $out += "backend = `"$Backend`""
                $written = $true
            }
            continue
        }
        $out += $line
    }
    if ($inFinal -and -not $written) {
        $out += "backend = `"$Backend`""
        $written = $true
    }
    if (-not $hasFinal) {
        $out += '[final]'
        $out += "backend = `"$Backend`""
    }
    $temporary = "$Config.tmp"
    Set-Content -Path $temporary -Value $out -Encoding UTF8
    Move-Item -Force $temporary $Config
}

if (-not [string]::IsNullOrEmpty($Backend)) {
    $configDir = Join-Path $env:APPDATA 'voime'
    New-Item -ItemType Directory -Force -Path $configDir | Out-Null
    $config = Join-Path $configDir 'config.toml'
    Set-FinalBackend -Config $config -Backend $Backend
    Write-Host "final.backend = $Backend (restart voimed to apply)"
}

Write-Host "GPU module installed. Run: $(Join-Path $BinDir 'voime.exe') doctor"
