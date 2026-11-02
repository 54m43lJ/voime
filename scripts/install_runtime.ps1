[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('windows-directml', 'windows-cuda', 'linux-migraphx', 'linux-cuda')]
    [string]$Runtime,
    [string]$Archive = '',
    [string]$Url = ''
)

$ErrorActionPreference = 'Stop'
if (-not $Archive -and -not $Url) {
    throw 'install_runtime.ps1: pass -Archive or -Url; GPU runtimes are explicit opt-in'
}
$base = if ($env:VOIME_RUNTIME_DIR) {
    $env:VOIME_RUNTIME_DIR
} else {
    Join-Path $env:LOCALAPPDATA 'voime\runtime'
}
$target = Join-Path $base $Runtime
New-Item -ItemType Directory -Force -Path $target | Out-Null

$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("voime-runtime-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $temporary | Out-Null
try {
    if ($Archive) {
        $package = $Archive
    } else {
        $package = Join-Path $temporary 'runtime.zip'
        Write-Host "install_runtime: downloading $Url"
        Invoke-WebRequest -Uri $Url -OutFile $package
    }
    Expand-Archive -LiteralPath $package -DestinationPath $temporary -Force
    $source = Get-ChildItem -LiteralPath $temporary -Directory |
        Where-Object { $_.Name -eq $Runtime -or $_.Name -eq 'runtime' } |
        Select-Object -First 1
    if (-not $source) { $source = Get-Item -LiteralPath $temporary }
    Copy-Item -Path (Join-Path $source.FullName '*') -Destination $target -Recurse -Force
    Write-Host "install_runtime: installed $Runtime -> $target"
    Write-Host "install_runtime: set VOIME_RUNTIME_DIR=$base before starting voimed"
}
finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
