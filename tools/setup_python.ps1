# 安装工作区本地 Python 3.12（通过 uv）并创建 .venv
# 用法: powershell -ExecutionPolicy Bypass -File tools\setup_python.ps1
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Toolchain = Join-Path $Root '.toolchain'
$UvDir = Join-Path $Toolchain 'uv'
$UvExe = Join-Path $UvDir 'uv.exe'
$PyDir = Join-Path $Toolchain 'python'
$DlDir = Join-Path $Toolchain 'downloads'

New-Item -ItemType Directory -Force -Path $UvDir, $PyDir, $DlDir | Out-Null

if (-not (Test-Path $UvExe)) {
    $zip = Join-Path $DlDir 'uv.zip'
    Write-Host '[setup] downloading uv ...'
    Invoke-WebRequest -UseBasicParsing `
        -Uri 'https://github.com/astral-sh/uv/releases/latest/download/uv-x86_64-pc-windows-msvc.zip' `
        -OutFile $zip
    Expand-Archive -Force -Path $zip -DestinationPath $UvDir
    Remove-Item $zip -ErrorAction SilentlyContinue
}
Write-Host "[setup] uv: $(& $UvExe --version)"

$env:UV_PYTHON_INSTALL_DIR = $PyDir
& $UvExe python install 3.12
Write-Host '[setup] creating .venv ...'
& $UvExe venv --python 3.12 (Join-Path $Root '.venv')
Write-Host '[setup] done. python:'
& (Join-Path $Root '.venv\Scripts\python.exe') --version
