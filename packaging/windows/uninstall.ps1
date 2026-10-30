[CmdletBinding()]
param(
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\Voime'),
    [switch]$RemoveModels,
    [switch]$RemoveSettings
)

$ErrorActionPreference = 'Stop'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

Get-Process -Name voime-tray,voimed,voime -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Remove-ItemProperty -Path $runKey -Name 'VoimeTray' -ErrorAction SilentlyContinue

if (Test-Path -LiteralPath $Prefix) {
    Remove-Item -LiteralPath $Prefix -Recurse -Force
    Write-Host "uninstall: removed $Prefix"
}

if ($RemoveSettings) {
    $config = Join-Path $env:APPDATA 'voime'
    if (Test-Path -LiteralPath $config) {
        Remove-Item -LiteralPath $config -Recurse -Force
        Write-Host "uninstall: removed $config"
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
