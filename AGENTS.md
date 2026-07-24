# AGENTS.md — Voime 项目代理须知

## 项目目标
Windows 离线语音输入法：C++ 前端 + Rust 核心引擎（FFI/C ABI），模型链路
FireRedStreamVad → FireRedASR2-AED → FireRedPunc。目标延迟 <500ms，动态内存 <200MB（不含模型）。

## 当前阶段
- 阶段一（POC 原型验证）：已完成，报告 `docs/poc/POC_REPORT.md`。
- 阶段二（Rust 核心引擎）：已完成，交付 `engine/`（voime_core.dll + include/voime.h），
  报告 `docs/stage2/RUST_ENGINE_REPORT.md`。
- 阶段三（C++ 前端 CPP-01~08）：已完成，交付 `frontend/`（voime_input.exe + 自测版），
  报告 `docs/stage3/CPP_FRONTEND_REPORT.md`。
- 下一步：阶段四（集成测试与产品化 QA-01~06）。

## 阶段三关键约定（frontend/）
- 构建：`powershell -ExecutionPolicy Bypass -File frontend\build.ps1`（MinGW g++，
  需 `-municode -D_WIN32_WINNT=0x0A00`），双产物：GUI 版 + `voime_input_console.exe`（自测版）。
- 自测：`voime_input_console.exe --selftest [--wav <16k s16 mono wav>]`（15 项验收）、
  `--repair`（依赖修复专项，真实下载 ~38MB）。
- FFmpeg 6.0 在 `.toolchain/ffmpeg/ffmpeg-6.0-full_build-shared/`（gyan.dev GitHub 镜像
  GyanD/codexffmpeg）；运行时仅需 swresample-4.dll + avutil-58.dll，随 exe 同目录分发。
- swresample 与 voime_core.dll 均为运行时延迟加载（LoadLibrary），缺依赖时可弹修复 UI。
- 配置/日志：`%APPDATA%\VoimeInput\config.toml` / `voime.log`（首启自动写默认，PRD 5.2）。
- 运行时 DLL 布局：exe + voime_core.dll + onnxruntime*.dll + swresample/avutil（不含模型）。

## 阶段二关键约定（engine/）
- ONNX 模型：`models/FireRedVAD-ONNX/`（官方）、`FireRedASR2-AED-CTC-ONNX/`（CTC int8）、
  `FireRedPunc-ONNX/`（自研导出，`tools/export_punc_onnx.py` 可复现）。
- ONNX Runtime 1.27.0 在 `.toolchain/onnxruntime/`；`ort` crate 用 `load-dynamic`，
  经 `VOIME_ORT_DLL` 或同目录 onnxruntime.dll 定位。
- 构建 Rust 必须先把工作区 MinGW 加入 PATH：`$env:PATH = "$PWD\.toolchain\mingw64\bin;$env:PATH"`
  （rust-mingw 自含 dlltool 缺 as.exe，会导致 windows-sys 构建失败）。
- 验证 CLI：`engine/target/release/voime_cli.exe {vad|asr|punc|e2e|bench} <wav>`；
  FFI 验证：`tools/test_ffi_e2e.py`（ctypes）。

## 环境约定（重要）
- **一切工具链安装在仓库内**：`.toolchain/`（uv、Python 3.12、rustup/cargo、mingw、ffmpeg、onnxruntime）、`.venv/`。
  禁止向系统目录（C:\Program Files、%LOCALAPPDATA% 等）安装软件。
- Python 用 **3.12**（不是系统 3.14：`kaldi_native_fbank` 无 cp314 wheel）。统一经 `.venv\Scripts\python` 调用。
- Rust 用 `x86_64-pc-windows-gnu` target（系统无 MSVC）；C++ 用 `.toolchain/mingw` 的 g++。
  环境变量：`RUSTUP_HOME=.toolchain\rust\rustup`、`CARGO_HOME=.toolchain\rust\cargo`。
- 模型权重在 `models/`（gitignore）：`FireRedASR2-AED/`、`FireRedVAD/`、`FireRedPunc/`。
- 第三方推理代码：`poc/third_party/FireRedASR2S`（浅克隆，gitignore），通过 `PYTHONPATH` 引入。
- 官方代码仅测过 Linux；Windows 兼容性问题就地修补并记录到 POC 报告。

## 关键路径
- POC 脚本：`poc/python/poc0*.py`，共享封装在 `poc/python/voime_poc/`。
- POC 运行产物：`poc/python/out/`（gitignore）。
- 前端源码：`frontend/src/`，构建产物 `frontend/build/`（gitignore）。
- 报告：`docs/poc/POC_REPORT.md`、`docs/stage2/RUST_ENGINE_REPORT.md`、
  `docs/stage3/CPP_FRONTEND_REPORT.md`。

## 提交规范
- 中文提交信息，格式 `<范围>: <动作><描述>`，如 `poc: 新增 POC-01 模型加载脚本`。
- 不提交模型、工具链、虚拟环境、构建产物（见 .gitignore）。
