[CmdletBinding()]
param(
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\Voime'),
    [switch]$RemoveModels,
    [switch]$RemoveSettings,
    [switch]$RemoveRuntime
)

$ErrorActionPreference = 'Stop'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

function Stop-VoimeProcesses {
    foreach ($name in @('voime-tray', 'voimed', 'voime', 'voime-gpu')) {
        Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object {
            Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
        }
    }
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Process -Name voime-tray,voimed,voime,voime-gpu -ErrorAction SilentlyContinue) -and ((Get-Date) -lt $deadline)) {
        Start-Sleep -Milliseconds 100
    }
}

function Remove-WithRetry {
    param([string]$Path)
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        try {
            Remove-Item -LiteralPath $Path -Recurse -Force
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
Remove-ItemProperty -Path $runKey -Name 'VoimeTray' -ErrorAction SilentlyContinue

if (Test-Path -LiteralPath $Prefix) {
    Remove-WithRetry -Path $Prefix
    Write-Host "uninstall: removed $Prefix"
}

if ($RemoveSettings) {
    $config = Join-Path $env:APPDATA 'voime'
    if (Test-Path -LiteralPath $config) {
        Remove-Item -LiteralPath $config -Recurse -Force
        Write-Host "uninstall: removed $config"
    }
}

if ($RemoveRuntime) {
    $runtime = Join-Path $env:LOCALAPPDATA 'voime\runtime'
    if (Test-Path -LiteralPath $runtime) {
        Remove-Item -LiteralPath $runtime -Recurse -Force
        Write-Host "uninstall: removed $runtime"
    }
}

if ($RemoveModels) {
    $data = Join-Path $env:LOCALAPPDATA 'voime'
    if (Test-Path -LiteralPath $data) {
        Remove-Item -LiteralPath $data -Recurse -Force
        Write-Host "uninstall: removed $data"
    }
}

Write-Host 'uninstall: done'
