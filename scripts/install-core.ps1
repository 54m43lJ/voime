param(
    [switch]$NoModels,
    [string]$ModelDir = (Join-Path $env:LOCALAPPDATA 'voime\models'),
    [string]$SourceDir = '',
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\Voime'),
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$BinDir = $Prefix
$RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$TrayWasRunning = [bool](Get-Process -Name 'voime-tray' -ErrorAction SilentlyContinue)

function Stop-Core {
    if (Get-Process -Name 'voime-tray' -ErrorAction SilentlyContinue) {
        if (-not ('VoimeInstallerNative' -as [type])) {
            Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class VoimeInstallerNative {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindowW(string className, string windowName);
    [DllImport("user32.dll")]
    public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
}
"@
        }
        $hwnd = [VoimeInstallerNative]::FindWindowW('VoimeTrayWindow', $null)
        if ($hwnd -ne [IntPtr]::Zero) {
            [void][VoimeInstallerNative]::PostMessageW($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
            $deadline = (Get-Date).AddSeconds(3)
            while ((Get-Date) -lt $deadline -and
                   (Get-Process -Name 'voime-tray' -ErrorAction SilentlyContinue)) {
                Start-Sleep -Milliseconds 100
            }
        }
    }
    Get-Process -Name 'voime-tray', 'voimed' -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 300
}

function Remove-Core {
    Stop-Core
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $BinDir 'voimed.exe')
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $BinDir 'voime.exe')
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $BinDir 'voime-tray.exe')
    Remove-ItemProperty -Path $RunKey -Name 'Voime' -ErrorAction SilentlyContinue
    Write-Host "Voime core removed (models left in $ModelDir)."
}

if ($Uninstall) {
    Remove-Core
    exit 0
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo is required to build Voime'
}

Write-Host 'Building voimed, voime and voime-tray...'
Push-Location $Root
try {
    cargo build --release -p voimed -p voime
    if ($LASTEXITCODE -ne 0) { throw 'core build failed' }
} finally { Pop-Location }
Push-Location (Join-Path $Root 'frontends\windows')
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'Windows frontend build failed' }
} finally { Pop-Location }

New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
Stop-Core
Copy-Item -Force (Join-Path $Root 'target\release\voimed.exe') $BinDir
Copy-Item -Force (Join-Path $Root 'target\release\voime.exe') $BinDir
Copy-Item -Force (Join-Path $Root 'frontends\windows\target\release\voime-tray.exe') $BinDir

if (-not $NoModels) {
    if ([string]::IsNullOrEmpty($SourceDir)) {
        $python = Get-Command python3 -ErrorAction SilentlyContinue
        if (-not $python) { $python = Get-Command python -ErrorAction SilentlyContinue }
        if (-not $python) {
            throw 'python3 is required to download core models; pass -SourceDir or -NoModels'
        }
        & $python.Source (Join-Path $Root 'scripts\install-core-models.py') --model-root $ModelDir
        if ($LASTEXITCODE -ne 0) { throw 'core model installation failed' }
    } else {
        $python = Get-Command python3 -ErrorAction SilentlyContinue
        if (-not $python) { $python = Get-Command python -ErrorAction SilentlyContinue }
        if (-not $python) {
            throw 'python3 is required to validate core models; pass -NoModels to skip'
        }
        & $python.Source (Join-Path $Root 'scripts\install-core-models.py') `
            --model-root $ModelDir --source-dir $SourceDir --no-download
        if ($LASTEXITCODE -ne 0) { throw 'core model installation failed' }
    }
    Write-Host "Core models installed under $ModelDir"
}

$trayPath = Join-Path $BinDir 'voime-tray.exe'
$desiredRunValue = '"' + $trayPath + '"'
$currentRunValue = (Get-ItemProperty -Path $RunKey -Name 'Voime' -ErrorAction SilentlyContinue).Voime
New-Item -Path $RunKey -Force | Out-Null
if ($currentRunValue -ne $desiredRunValue) {
    Set-ItemProperty -Path $RunKey -Name 'Voime' -Value $desiredRunValue
}
if ($TrayWasRunning) {
    Write-Host 'voime-tray was stopped for the upgrade; start it again when ready.'
}
Write-Host "Voime core installed. Run: $(Join-Path $BinDir 'voime.exe') doctor"
