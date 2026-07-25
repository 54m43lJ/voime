# QA-06 打包：极简绿色版 Zip（含运行时依赖，不含模型，PRD 2.2）
# 用法: powershell -ExecutionPolicy Bypass -File frontend\package.ps1
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Version = '1.0.0-mvp'
$BuildDir = Join-Path $PSScriptRoot 'build'
$DistDir = Join-Path $PSScriptRoot 'dist'
$PkgName = "voime_input-$Version-win64"
$StageDir = Join-Path $DistDir $PkgName

# 确保最新构建
& (Join-Path $PSScriptRoot 'build.ps1')

Remove-Item -Recurse -Force $StageDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

# 核心程序 + 运行时依赖（FFmpeg DLL 随包分发；不含模型，首启向导下载）
$Files = @(
    'voime_input.exe',
    'voime_core.dll',
    'onnxruntime.dll',
    'onnxruntime_providers_shared.dll',
    'swresample-4.dll',
    'avutil-58.dll'
)
foreach ($f in $Files) {
    $src = Join-Path $BuildDir $f
    if (!(Test-Path $src)) { throw "缺构建产物: $src" }
    Copy-Item $src $StageDir
}

# 使用说明
$Readme = @"
Voime 语音输入法 $Version (MVP, Windows 10 1903+ x64)
=====================================================

快速开始
--------
1. 解压到任意目录，运行 voime_input.exe（绿色版，无需安装）。
2. 首次启动自动弹出模型下载向导（约 1.2GB，下载到 %APPDATA%\VoimeInput\models）。
3. 按住 Ctrl+Space 说话，松开即把识别文本输入到当前窗口。
4. 系统托盘图标右键 -> 退出。

配置
----
%APPDATA%\VoimeInput\config.toml（hotkey、ffmpeg 版本、下载地址等）。

已知限制（MVP）
--------------
- 无法向管理员权限运行的窗口输入文字（Windows UIPI 限制，后续 TSF 版本解决）。
- 需 CPU 支持 AVX2，内存 >= 4GB。
- 识别模型需联网下载一次，之后完全离线可用。
"@
Set-Content -Path (Join-Path $StageDir 'README.txt') -Value $Readme -Encoding UTF8

# 打 Zip
New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
$ZipPath = Join-Path $DistDir "$PkgName.zip"
Remove-Item $ZipPath -ErrorAction SilentlyContinue
Compress-Archive -Path $StageDir -DestinationPath $ZipPath -CompressionLevel Optimal

$zip_mb = [math]::Round((Get-Item $ZipPath).Length / 1MB, 1)
Write-Host "[package] OK -> $ZipPath (${zip_mb}MB)"
Get-ChildItem $StageDir | Select-Object Name, Length
