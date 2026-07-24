# 阶段三 C++ 前端构建脚本（MinGW g++，与 engine/ 同工具链）
# 用法: powershell -ExecutionPolicy Bypass -File frontend\build.ps1
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Gxx = Join-Path $Root '.toolchain\mingw64\bin\g++.exe'
$FF = Join-Path $Root '.toolchain\ffmpeg\ffmpeg-6.0-full_build-shared'
$OutDir = Join-Path $PSScriptRoot 'build'
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$Sources = @(
    'main.cpp', 'app.cpp', 'engine.cpp', 'config.cpp', 'deps.cpp',
    'resampler.cpp', 'audio_capture.cpp', 'hotkey.cpp', 'overlay.cpp',
    'injector.cpp', 'selftest.cpp'
) | ForEach-Object { Join-Path $PSScriptRoot "src\$_" }

$CommonArgs = @(
    '-std=c++17', '-O2', '-Wall', '-DUNICODE', '-D_UNICODE',
    '-D_WIN32_WINNT=0x0A00', '-DWINVER=0x0A00',
    '-I', (Join-Path $Root 'engine\include'),
    '-I', (Join-Path $FF 'include')
) + $Sources + @(
    '-lole32', '-luuid', '-lwinmm', '-lavrt', '-ld2d1', '-ldwrite', '-lwinhttp', '-lshell32',
    '-lgdi32', '-lcomdlg32',
    '-static', '-static-libgcc', '-static-libstdc++', '-lwinpthread'
)

Write-Host '[build] g++ -> voime_input.exe (GUI) ...'
& $Gxx @CommonArgs -mwindows -municode -o (Join-Path $OutDir 'voime_input.exe')
if ($LASTEXITCODE -ne 0) { throw 'g++ build failed (gui)' }

Write-Host '[build] g++ -> voime_input_console.exe (console/selftest) ...'
& $Gxx @CommonArgs -mconsole -municode -DVOIME_CONSOLE_MAIN -o (Join-Path $OutDir 'voime_input_console.exe')
if ($LASTEXITCODE -ne 0) { throw 'g++ build failed (console)' }

# 拷贝运行时依赖（FFmpeg DLL 随包分发，PRD 2.2 极简安装策略；不含模型）
$Dlls = @(
    (Join-Path $Root 'engine\target\release\voime_core.dll'),
    (Join-Path $Root '.toolchain\onnxruntime\lib\onnxruntime.dll'),
    (Join-Path $Root '.toolchain\onnxruntime\lib\onnxruntime_providers_shared.dll'),
    (Join-Path $FF 'bin\swresample-4.dll'),
    (Join-Path $FF 'bin\avutil-58.dll')
)
foreach ($d in $Dlls) { Copy-Item $d $OutDir -Force }

Write-Host "[build] OK -> $OutDir\voime_input.exe"
Get-ChildItem $OutDir | Select-Object Name, Length
