# Voime — Windows 离线语音输入法

基于本地离线推理的 Windows 语音输入工具：轻量、快速、离线、精准。

- **架构**：C++ 前端（Win32/WASAPI/Direct2D） + Rust 核心引擎（ONNX Runtime），C ABI FFI 通信。
- **模型**：FireRedASR2-AED（识别）、FireRedVAD Stream-VAD（流式端点检测）、FireRedPunc（标点补全）。
- **产品设计文档**：见 `docs/`（PRD V1.2 定稿）。

## 仓库结构

```
voime/
├── docs/            # 设计文档与验证报告（docs/poc/ 阶段一，docs/stage2/ 阶段二）
├── engine/          # 阶段二：Rust 核心引擎（voime_core cdylib + voime_cli + include/voime.h）
├── poc/             # 阶段一：POC 原型验证
│   ├── python/      #   Python 逻辑线（POC-01 ~ POC-05）
│   ├── ffi/         #   C++/Rust 通讯线（POC-06）
│   └── third_party/ #   第三方源码（FireRedASR2S，gitignore）
├── models/          # 模型权重（gitignore，按需下载）
├── tools/           # 辅助脚本
└── .toolchain/      # 工作区本地工具链（uv/python/rust/mingw，gitignore）
```

## 当前状态

- **阶段一（POC 原型验证）已完成（2026-07-19）**：POC-01~04、06 通过，POC-05 部分达标。
  报告：`docs/poc/POC_REPORT.md`。
- **阶段二（Rust 核心引擎）已完成（2026-07-20）**：RUST-01~08 全部通过。
  交付 `voime_core.dll` + `engine/include/voime.h`；端到端 PTT 延迟 250~300ms（目标 <500ms），
  模型内存 1.2GB（较阶段一 9.7GB 降 8×）。报告：`docs/stage2/RUST_ENGINE_REPORT.md`。

### 引擎构建（阶段二）

```powershell
$env:PATH = "$PWD\.toolchain\mingw64\bin;$env:PATH"   # dlltool/as 依赖
cargo build --workspace --release                     # engine/ 目录
# 产物: engine/target/release/voime_core.dll, voime_cli.exe
$env:VOIME_MODEL_DIR = "$PWD\models"
.\engine\target\release\voime_cli.exe e2e poc\python\out\session_zh_3sent.wav
```

### 复现环境搭建

```powershell
# 1. 安装工作区本地 Python 3.12（uv）
.\tools\setup_python.ps1          # 下载 uv、安装 Python、创建 .venv

# 2. 安装 Python 依赖
.\.venv\Scripts\pip install -r poc\python\requirements.txt

# 3. 下载模型（约 5.1GB，含 ASR/VAD/Punc 三个模型）
.\.venv\Scripts\python tools\download_models.py

# 4. 拉取 FireRedASR2S 推理代码
git clone --depth 1 https://github.com/FireRedTeam/FireRedASR2S.git poc\third_party\FireRedASR2S
```

### 运行 POC

```powershell
.\.venv\Scripts\python poc\python\poc01_load_models.py      # POC-01 模型加载
.\.venv\Scripts\python poc\python\poc02_vad_realtime.py     # POC-02 实时 VAD
.\.venv\Scripts\python poc\python\poc03_pipeline.py         # POC-03 VAD->ASR
.\.venv\Scripts\python poc\python\poc04_punc.py             # POC-04 标点补全
.\.venv\Scripts\python poc\python\poc05_latency.py          # POC-05 延迟测量
# POC-06 见 poc/ffi/README.md
```
