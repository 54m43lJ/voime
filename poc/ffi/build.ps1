# POC-06 构建脚本：Rust cdylib + C++ 宿主
# 用法: powershell -ExecutionPolicy Bypass -File poc\ffi\build.ps1
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$env:RUSTUP_HOME = Join-Path $Root '.toolchain\rust\rustup'
$env:CARGO_HOME = Join-Path $Root '.toolchain\rust\cargo'
$CargoBin = Join-Path $env:CARGO_HOME 'bin'
$Gxx = Join-Path $Root '.toolchain\mingw64\bin\g++.exe'
$OutDir = Join-Path $PSScriptRoot 'out'
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

Write-Host '[build] cargo build --release ...'
Push-Location (Join-Path $PSScriptRoot 'rust')
& (Join-Path $CargoBin 'cargo.exe') build --release
if ($LASTEXITCODE -ne 0) { Pop-Location; throw 'cargo build failed' }
Pop-Location

$dll = Join-Path $PSScriptRoot 'rust\target\release\voime_poc_core.dll'
Copy-Item $dll $OutDir -Force

Write-Host '[build] g++ poc_host.cpp ...'
& $Gxx -std=c++17 -O2 -Wall -static -static-libgcc -static-libstdc++ `
    (Join-Path $PSScriptRoot 'cpp\poc_host.cpp') `
    -o (Join-Path $OutDir 'poc_host.exe') `
    (Join-Path $OutDir 'voime_poc_core.dll') -lole32 -lwinmm -luuid -lwinpthread
if ($LASTEXITCODE -ne 0) { throw 'g++ build failed' }

Write-Host "[build] OK -> $OutDir\poc_host.exe, voime_poc_core.dll"
