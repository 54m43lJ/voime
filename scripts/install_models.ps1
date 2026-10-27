$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
$Cache = if ($env:XDG_CACHE_HOME) {
    Join-Path $env:XDG_CACHE_HOME "voime"
} elseif ($env:LOCALAPPDATA) {
    Join-Path $env:LOCALAPPDATA "voime\cache"
} else {
    Join-Path $HOME ".cache\voime"
}
$Venv = Join-Path $Cache "venv"
$Requirements = Join-Path $Root "tools\requirements-export.txt"

$Python = $env:VOIME_PYTHON
$PythonArgs = @()
if (-not $Python) {
    $Command = Get-Command python -ErrorAction SilentlyContinue
    if ($Command) {
        $Python = $Command.Source
    } else {
        $Command = Get-Command py -ErrorAction SilentlyContinue
        if (-not $Command) {
            throw "install_models: 找不到 Python 3.10+；请设置 VOIME_PYTHON"
        }
        $Python = $Command.Source
        $PythonArgs = @("-3")
    }
}

& $Python @PythonArgs -c "import sys; raise SystemExit(0 if sys.version_info >= (3,10) else 1)"
if ($LASTEXITCODE -ne 0) {
    throw "install_models: 需要 Python 3.10+"
}

New-Item -ItemType Directory -Force -Path $Cache | Out-Null
$VenvPython = Join-Path $Venv "Scripts\python.exe"
if (-not (Test-Path $VenvPython)) {
    Write-Host "install_models: 创建虚拟环境 $Venv"
    & $Python @PythonArgs -m venv $Venv
    if ($LASTEXITCODE -ne 0) { throw "创建虚拟环境失败" }
}

$Hash = (Get-FileHash -Algorithm SHA256 $Requirements).Hash
$Marker = Join-Path $Venv ".voime-requirements"
$Installed = if (Test-Path $Marker) { (Get-Content $Marker -Raw).Trim() } else { "" }
if ($Installed -ne $Hash) {
    Write-Host "install_models: 安装导出依赖"
    & $VenvPython -m pip install --upgrade pip
    if ($LASTEXITCODE -ne 0) { throw "pip 升级失败" }
    & $VenvPython -m pip install -r $Requirements
    if ($LASTEXITCODE -ne 0) { throw "依赖安装失败" }
    Set-Content -Path $Marker -Value $Hash
}

& $VenvPython (Join-Path $Root "tools\install_models.py") @args
exit $LASTEXITCODE
