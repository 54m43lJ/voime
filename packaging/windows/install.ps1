[CmdletBinding()]
param(
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\Voime'),
    [string]$ModelDir = (Join-Path $env:LOCALAPPDATA 'voime\models'),
    [string]$CacheDir = (Join-Path $env:LOCALAPPDATA 'voime\cache'),
    [switch]$NoBuild,
    [switch]$NoStartup,
    [switch]$NoModels
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

function Assert-Success {
    param([string]$Step)
    if ($LASTEXITCODE -ne 0) {
        throw "$Step failed with exit code $LASTEXITCODE"
    }
}

function Stop-VoimeProcesses {
    foreach ($name in @('voime-tray', 'voimed', 'voime')) {
        Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object {
            Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
        }
    }
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Process -Name voime-tray,voimed,voime -ErrorAction SilentlyContinue) -and ((Get-Date) -lt $deadline)) {
        Start-Sleep -Milliseconds 100
    }
}

function Copy-InstalledFile {
    param([string]$Source, [string]$Destination)
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        try {
            Copy-Item -LiteralPath $Source -Destination $Destination -Force
            return
        }
        catch [System.IO.IOException] {
            if ($attempt -eq 19) {
                throw
            }
            Start-Sleep -Milliseconds 250
        }
    }
}

Stop-VoimeProcesses

if (-not $NoBuild) {
    Push-Location $Root
    try {
        Write-Host 'install: building voimed and voime'
        & cargo build --release -p voimed -p voime
        Assert-Success 'cargo build -p voimed -p voime'
        Write-Host 'install: building voime-tray'
        & cargo build --release --manifest-path (Join-Path $Root 'frontends\windows\Cargo.toml')
        Assert-Success 'cargo build voime-tray'
    }
    finally {
        Pop-Location
    }
}

$sources = @(
    @{ Path = (Join-Path $Root 'target\release\voimed.exe'); Name = 'voimed.exe' },
    @{ Path = (Join-Path $Root 'target\release\voime.exe'); Name = 'voime.exe' },
    @{ Path = (Join-Path $Root 'frontends\windows\target\release\voime-tray.exe'); Name = 'voime-tray.exe' }
)
foreach ($source in $sources) {
    if (-not (Test-Path -LiteralPath $source.Path)) {
        throw "missing $($source.Path); build without -NoBuild first"
    }
}

Stop-VoimeProcesses
New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
foreach ($source in $sources) {
    Copy-InstalledFile -Source $source.Path -Destination (Join-Path $Prefix $source.Name)
    Write-Host "install: installed $($source.Name)"
}

$runtime = Get-ChildItem -LiteralPath (Join-Path $Root 'target\release') -Filter onnxruntime.dll -File -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
if ($runtime) {
    Copy-InstalledFile -Source $runtime.FullName -Destination (Join-Path $Prefix 'onnxruntime.dll')
    Write-Host 'install: installed onnxruntime.dll'
}
else {
    Write-Host 'install: onnxruntime is statically linked; no DLL to copy'
}

$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if ($NoStartup) {
    Remove-ItemProperty -Path $runKey -Name 'VoimeTray' -ErrorAction SilentlyContinue
    Write-Host 'install: skipped login startup'
}
else {
    New-Item -Path $runKey -Force | Out-Null
    $tray = Join-Path $Prefix 'voime-tray.exe'
    Set-ItemProperty -Path $runKey -Name 'VoimeTray' -Value ('"' + $tray + '"')
    Write-Host 'install: enabled login startup'
}

if ($NoModels) {
    Write-Host 'install: skipped model installation'
}
else {
    & (Join-Path $Root 'scripts\install_models.ps1') -ModelDir $ModelDir -CacheDir $CacheDir
    Assert-Success 'install_models.ps1'
}

Write-Host ''
Write-Host "installed to $Prefix"
Write-Host "models:  $ModelDir"
Write-Host "config:  $(Join-Path $env:APPDATA 'voime\config.json')"
Write-Host ''
Write-Host 'next:'
Write-Host "  & '$(Join-Path $Prefix 'voime-tray.exe')'"
Write-Host "  & '$(Join-Path $Prefix 'voime.exe')' ctl status"
