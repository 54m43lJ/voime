param(
    [switch]$Firered,
    [switch]$Qwen3,
    [ValidateSet('cuda', 'directml')]
    [string]$FireredRuntime = 'cuda',
    [ValidateSet('vulkan', 'cuda')]
    [string]$QwenRuntime = 'vulkan',
    [ValidateSet('cpu', 'firered', 'qwen3')]
    [string]$Backend = '',
    [switch]$NoModels,
    [string]$ModelDir = (Join-Path $env:LOCALAPPDATA 'voime\models'),
    [string]$SourceDir = '',
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\Voime'),
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$BinDir = $Prefix
$InstallFirered = $true
$InstallQwen = $true
if ($Firered -or $Qwen3) {
    $InstallFirered = [bool]$Firered
    $InstallQwen = [bool]$Qwen3
}
if (-not $PSBoundParameters.ContainsKey('FireredRuntime')) {
    $video = Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty Name
    if ($video -match 'NVIDIA') { $FireredRuntime = 'cuda' } else { $FireredRuntime = 'directml' }
}
if (-not [string]::IsNullOrEmpty($Backend) -and ($Firered -or $Qwen3)) {
    if ($Backend -eq 'firered' -and -not $InstallFirered) {
        throw '--backend firered requires installing the FireRed backend'
    }
    if ($Backend -eq 'qwen3' -and -not $InstallQwen) {
        throw '--backend qwen3 requires installing the Qwen backend'
    }
}

if ($Uninstall) {
    $RuntimeDir = Join-Path $Prefix 'lib\voime'
    if ($InstallFirered) {
        Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $BinDir 'voime-gpu-firered.exe')
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $RuntimeDir 'firered')
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $ModelDir 'fireredasr2-aed')
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $ModelDir 'fireredpunc')
    }
    if ($InstallQwen) {
        Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $BinDir 'voime-gpu-qwen3.exe')
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $RuntimeDir 'qwen3')
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $ModelDir 'qwen3-asr-1.7b')
    }
    Write-Host 'Selected GPU backends removed; final.backend is unchanged.'
    exit 0
}

if (-not (Test-Path (Join-Path $BinDir 'voimed.exe')) -and
    -not (Test-Path (Join-Path $env:LOCALAPPDATA 'Programs\Voime\voimed.exe'))) {
    throw 'core install not found; install core before the GPU module'
}

if ($InstallFirered) {
    $prebuilt = if ([string]::IsNullOrEmpty($SourceDir)) { '' } else { Join-Path $SourceDir 'bin\voime-gpu-firered.exe' }
    if (-not [string]::IsNullOrEmpty($prebuilt) -and (Test-Path $prebuilt)) {
        Write-Host 'Installing precompiled FireRed worker...'
        Copy-Item -Force $prebuilt $BinDir
    } else {
        Write-Host "Building FireRed worker ($FireredRuntime)..."
        Push-Location (Join-Path $Root 'crates\voime-gpu-firered')
        try {
            cargo build --release --features $FireredRuntime
            if ($LASTEXITCODE -ne 0) { throw 'FireRed worker build failed' }
        } finally { Pop-Location }
        Copy-Item -Force (Join-Path $Root 'crates\voime-gpu-firered\target\release\voime-gpu-firered.exe') $BinDir
    }
}

if ($InstallQwen) {
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
}

if (-not $NoModels) {
    if ([string]::IsNullOrEmpty($SourceDir)) {
        $SourceDir = Join-Path $Root 'models'
    }
    $models = @()
    if ($InstallFirered) { $models += @('fireredasr2-aed', 'fireredpunc') }
    if ($InstallQwen) { $models += 'qwen3-asr-1.7b' }
    foreach ($model in $models) {
        $source = Join-Path $SourceDir $model
        if (-not (Test-Path (Join-Path $source 'manifest.json'))) {
            throw "missing model manifest: $source\manifest.json (use -NoModels to skip)"
        }
        $target = Join-Path $ModelDir $model
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $target
        New-Item -ItemType Directory -Force -Path $ModelDir | Out-Null
        Copy-Item -Recurse -Force $source $target
    }
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
