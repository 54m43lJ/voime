# 阶段五构建脚本（MinGW g++，与 engine/ 同工具链）
# 产物（frontend/build/）：
#   voime_input.exe         GUI 标准模式（TSF/IPC 架构；不含热键/SendInput/自测代码）
#   voime_input_notsf.exe   GUI NOTSF 调试模式（本地热键 + SendInput，无 IPC）
#   voime_input_console.exe 控制台 NOTSF 模式（--selftest/--repair/--soak）
#   voime_tsf.dll           TSF 文本服务（frontend/tsf/）
#   tsf_harness.exe         qa06 用 TSF 宿主测试程序（frontend/qa/）
# 用法: powershell -ExecutionPolicy Bypass -File frontend\build.ps1
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Gxx = Join-Path $Root '.toolchain\mingw64\bin\g++.exe'
$FF = Join-Path $Root '.toolchain\ffmpeg\ffmpeg-6.0-full_build-shared'
$OutDir = Join-Path $PSScriptRoot 'build'
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# Agent 公共源码（ipc_server.cpp 在 NOTSF 下为空编译单元）
$SrcCommon = @(
    'main.cpp', 'app.cpp', 'engine.cpp', 'config.cpp', 'deps.cpp', 'http.cpp',
    'resampler.cpp', 'audio_capture.cpp', 'overlay.cpp',
    'models.cpp', 'wizard.cpp', 'wav_source.cpp', 'ipc_server.cpp'
) | ForEach-Object { Join-Path $PSScriptRoot "src\$_" }
# NOTSF 专属源码（全局热键 + SendInput 注入 + 控制台自测）
$SrcNotsf = @('hotkey.cpp', 'injector.cpp', 'selftest.cpp') |
            ForEach-Object { Join-Path $PSScriptRoot "src\$_" }

$Flags = @(
    '-std=c++17', '-O2', '-Wall', '-DUNICODE', '-D_UNICODE',
    '-D_WIN32_WINNT=0x0A00', '-DWINVER=0x0A00',
    '-I', (Join-Path $Root 'engine\include'),
    '-I', (Join-Path $FF 'include')
)
$Libs = @(
    '-lole32', '-luuid', '-lwinmm', '-lavrt', '-ld2d1', '-ldwrite', '-lwinhttp', '-lshell32',
    '-lgdi32', '-lcomdlg32', '-lcomctl32', '-lbcrypt', '-ladvapi32',
    '-static', '-static-libgcc', '-static-libstdc++', '-lwinpthread'
)

Write-Host '[build] g++ -> voime_input.exe (GUI, 标准 TSF/IPC 模式) ...'
& $Gxx @Flags @SrcCommon @Libs -mwindows -municode -o (Join-Path $OutDir 'voime_input.exe')
if ($LASTEXITCODE -ne 0) { throw 'g++ build failed (std gui)' }

Write-Host '[build] g++ -> voime_input_console.exe (console/selftest, NOTSF) ...'
& $Gxx @Flags ($SrcCommon + $SrcNotsf) @Libs -mconsole -municode -DVOIME_NOTSF -DVOIME_CONSOLE_MAIN -o (Join-Path $OutDir 'voime_input_console.exe')
if ($LASTEXITCODE -ne 0) { throw 'g++ build failed (console)' }

Write-Host '[build] g++ -> voime_input_notsf.exe (GUI 调试版, NOTSF) ...'
& $Gxx @Flags ($SrcCommon + $SrcNotsf) @Libs -mwindows -municode -DVOIME_NOTSF -o (Join-Path $OutDir 'voime_input_notsf.exe')
if ($LASTEXITCODE -ne 0) { throw 'g++ build failed (notsf gui)' }

Write-Host '[build] g++ -> voime_tsf.dll (TSF 文本服务) ...'
$TsfSrc = Get-ChildItem (Join-Path $PSScriptRoot 'tsf\src\*.cpp') | ForEach-Object { $_.FullName }
$TsfFlags = @(
    '-std=c++17', '-O2', '-Wall', '-DUNICODE', '-D_UNICODE',
    '-D_WIN32_WINNT=0x0A00', '-DWINVER=0x0A00',
    '-I', (Join-Path $PSScriptRoot 'src')
)
$TsfLibs = @(
    '-lole32', '-luuid', '-ladvapi32', '-lbcrypt',
    '-static', '-static-libgcc', '-static-libstdc++', '-lwinpthread'
)
& $Gxx @TsfFlags @TsfSrc (Join-Path $PSScriptRoot 'tsf\tsf.def') @TsfLibs -shared -o (Join-Path $OutDir 'voime_tsf.dll')
if ($LASTEXITCODE -ne 0) { throw 'g++ build failed (tsf dll)' }

Write-Host '[build] g++ -> tsf_harness.exe (qa06 TSF 宿主) ...'
& $Gxx @TsfFlags (Join-Path $PSScriptRoot 'qa\tsf_harness.cpp') @TsfLibs -mwindows -municode -o (Join-Path $OutDir 'tsf_harness.exe')
if ($LASTEXITCODE -ne 0) { throw 'g++ build failed (harness)' }

# 拷贝运行时依赖（FFmpeg DLL 随包分发，PRD 2.2 极简安装策略；不含模型）
$Dlls = @(
    (Join-Path $Root 'engine\target\release\voime_core.dll'),
    (Join-Path $Root '.toolchain\onnxruntime\lib\onnxruntime.dll'),
    (Join-Path $Root '.toolchain\onnxruntime\lib\onnxruntime_providers_shared.dll'),
    (Join-Path $FF 'bin\swresample-4.dll'),
    (Join-Path $FF 'bin\avutil-58.dll')
)
foreach ($d in $Dlls) { Copy-Item $d $OutDir -Force }

Write-Host "[build] OK -> $OutDir"
Get-ChildItem $OutDir | Select-Object Name, Length
