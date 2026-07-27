# 阶段五打包：TSF 架构绿色版 Zip（含运行时依赖与注册脚本，不含模型）
# 用法: powershell -ExecutionPolicy Bypass -File frontend\package.ps1
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Version = '1.1.0-tsf'
$BuildDir = Join-Path $PSScriptRoot 'build'
$DistDir = Join-Path $PSScriptRoot 'dist'
$PkgName = "voime_input-$Version-win64"
$StageDir = Join-Path $DistDir $PkgName

# 确保最新构建
& (Join-Path $PSScriptRoot 'build.ps1')

Remove-Item -Recurse -Force $StageDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

# 核心程序 + TSF 文本服务 + 运行时依赖（FFmpeg DLL 随包分发；不含模型，首启向导下载）
$Files = @(
    'voime_input.exe',          # 标准版（TSF/IPC 架构；无 SendInput）
    'voime_tsf.dll',            # TSF 文本服务（注册后语言栏可用）
    'voime_input_notsf.exe',    # NOTSF 调试版（热键 + SendInput，无 TSF 依赖）
    'voime_input_console.exe',  # 控制台自测版
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
Copy-Item (Join-Path $PSScriptRoot 'tsf\register_tsf.ps1') $StageDir
Copy-Item (Join-Path $PSScriptRoot 'tsf\unregister_tsf.ps1') $StageDir

# 使用说明
$Readme = @"
Voime 语音输入法 $Version (TSF 架构, Windows 10 1903+ x64)
=============================================================

快速开始
--------
1. 解压到任意目录（建议 %LOCALAPPDATA%\VoimeInput）。
2. 首次运行 voime_input.exe，模型下载向导自动弹出（约 1.2GB，
   下载到 %APPDATA%\VoimeInput\models，之后完全离线）。
3. 【注册输入法】右键以 PowerShell 运行 register_tsf.ps1
   （会弹 UAC 提权确认；语言栏出现 "Voime 语音输入"）。
4. 在语言栏切换到 "Voime 语音输入"，在任意应用（含管理员权限
   的 CMD/任务管理器）按住热键说话，松开即上屏。
5. 系统托盘图标右键 -> 退出。

架构说明（PRD 2.2 控制反转）
----------------------------
- voime_input.exe  = 被动计算服务（IPC 服务端 + 音频 + 悬浮窗）。
- voime_tsf.dll    = 标准 Windows 文本服务：拦截 PTT 热键 -> 指令给
                     Agent，收到识别文本后经 TSF 在光标处注入。
  因 TSF 在目标进程内工作，可注入管理员权限窗口（SendInput 做不到）。
- voime_input_notsf.exe = 调试回退版：全局热键 + 模拟键盘注入，
  无需注册，但不能输入提权窗口。

配置
----
%APPDATA%\VoimeInput\config.toml（hotkey、ffmpeg 版本、下载地址等）。
PTT 热键建议避开被其他软件全局占用的组合（TSF 模式下被
RegisterHotKey 占用的组合键到不了输入法；日志可见）。

卸载
----
1. 运行 unregister_tsf.ps1（提权）。
2. 删除安装目录与 %APPDATA%\VoimeInput。

已知限制
--------
- 需 CPU 支持 AVX2，内存 >= 4GB。
- 识别模型需联网下载一次，之后完全离线可用。
- 仅 x64 应用（暂不支持 32 位进程注入）。
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
