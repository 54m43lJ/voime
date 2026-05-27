# Voime — 轻量语音输入法

按下快捷键，说话，文字直接出现在光标位置。支持 AI 润色修正错字和标点。

## 架构

```
┌───────────────────────────────────────────────────────┐
│  Windows 应用 (Notepad / Word / VS Code / 任意)        │
│  ┌─────────────────────────────────────────────────┐ │
│  │  voime-ime.dll  (TSF 输入法)                     │ │
│  │  WASAPI 麦克风 → WebSocket → 接收文本 → 提交到光标 │ │
│  └──────────────────┬──────────────────────────────┘ │
└─────────────────────┼────────────────────────────────┘
                      │ ws://127.0.0.1:9876/asr
┌─────────────────────┼────────────────────────────────┐
│  voime-server (Python, 跨平台 Linux/macOS/Windows)    │
│  ┌──────────────────▼──────────────────────────────┐ │
│  │  ASR 引擎 (三选一)                                │ │
│  │  - faster-whisper  (CPU / NVIDIA CUDA)          │ │
│  │  - whisper.cpp     (CPU / NVIDIA CUDA / AMD Vulkan)│ │
│  │  - OpenAI API      (云端)                        │ │
│  └──────────────────┬──────────────────────────────┘ │
│  ┌──────────────────▼──────────────────────────────┐ │
│  │  LLM 润色 (可选, OpenAI-compatible API)          │ │
│  │  - Ollama / vLLM / OpenAI / 任意兼容服务          │ │
│  └─────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────┘
```

## 快速开始

### 环境要求

| 组件   | 要求                               |
|--------|------------------------------------|
| 后端   | Python 3.10+, pip                  |
| 前端   | Windows 10/11, MSVC 2022, CMake 3.20+ |
| ASR    | 见下方三种后端选一种                |
| LLM    | (可选) Ollama 或其他 OpenAI 兼容 API |

---

## 1. 后端部署

```bash
git clone <repo-url> voime
cd voime/server
pip install -r requirements.txt
```

### 配置

```bash
cp config.yaml.example config.yaml
# 编辑 config.yaml 选择 ASR 后端和模型
```

### 1a. ASR 后端：faster-whisper（CPU / NVIDIA GPU）

默认后端，无需额外安装。支持 CPU 和 NVIDIA CUDA。

```yaml
asr:
  backend: "faster_whisper"
  model: "base"            # tiny / base / small / medium / large-v3
  language: "zh"
  device: "cpu"            # cpu | cuda
  compute_type: "int8"     # int8 | float16 | float32
```

首次启动会自动下载模型到 `~/.cache/huggingface/`。

Intel GPU 用户：设 `device: "cpu"` 配合 `compute_type: "int8"`，faster-whisper 的 CTranslate2 后端会使用 oneDNN 加速。

### 1b. ASR 后端：whisper.cpp（AMD GPU / Vulkan）

适用于 **AMD 显卡**或其他支持 Vulkan 的设备。需要手动编译 whisper.cpp。

#### 编译 whisper.cpp

```bash
# 安装 Vulkan SDK
# Ubuntu/Debian:
sudo apt install vulkan-sdk libvulkan-dev

# Arch:
sudo pacman -S vulkan-devel

# Windows: 从 https://vulkan.lunarg.com/ 下载安装

# 克隆并编译 whisper.cpp
git clone https://github.com/ggerganov/whisper.cpp
cd whisper.cpp
cmake -B build -DWHISPER_VULKAN=1
cmake --build build -j$(nproc)
```

如果是 NVIDIA 卡，将 `-DWHISPER_VULKAN=1` 换成 `-DWHISPER_CUDA=1`。
如果是纯 CPU，不加任何 GPU 参数即可。

#### 下载 GGML 模型

```bash
cd whisper.cpp
# 下载 base 模型（~142MB）
bash models/download-ggml-model.sh base
# 其他可选: tiny / small / medium / large-v3
```

#### 配置 Voime

```yaml
asr:
  backend: "whisper_cpp"
  model: "base"
  language: "zh"
  whisper_cpp:
    binary: "/path/to/whisper.cpp/build/bin/whisper-cli"
    model_path: "/path/to/whisper.cpp/models/ggml-base.bin"
    threads: 4
```

如果你的 whisper.cpp 目录在 voime/server 的兄弟目录，binary 和 model_path 可以留空，程序会自动探测。

### 1c. ASR 后端：OpenAI Whisper API（云端）

无需本地 GPU，需要 OpenAI API Key。

```yaml
asr:
  backend: "faster_whisper"       # 暂时用这个占位，openai_realtime 后端待实现
  openai:
    api_key: "${OPENAI_API_KEY}"
```

> **注意**: OpenAI Realtime API 后端尚未实现，使用 `faster_whisper` 后端即可满足大部分需求。如需云端方案可以通过设置 `WHISPER_CPP_BINARY` 指向自建的 whisper-asr-webservice 实例。

---

## 2. LLM 润色（可选）

开启后，ASR 输出会经由 LLM 修正错别字、听错词和标点符号。

```yaml
llm:
  enabled: true
  base_url: "http://127.0.0.1:11434/v1"   # Ollama
  model: "qwen3:4b"
  system_prompt: "你是一个专业的文本校对助手..."
  user_prompt_template: "请修正以下语音输入文本：\n{text}"
```

支持任何 OpenAI-compatible API：Ollama、vLLM、OpenAI、Groq、DeepSeek 等。

安装 Ollama 并拉取模型：

```bash
# 安装 Ollama: https://ollama.com
ollama pull qwen3:4b
```

---

## 3. 启动后端

```bash
cd voime/server
python -m voime_server.main

# 输出:
# Starting Voime server on 127.0.0.1:9876
# ASR backend: whisper_cpp (model: base)
# LLM polishing disabled
```

测试服务是否正常：

```bash
# 健康检查
curl http://127.0.0.1:9876/health

# 用麦克风测试
python tests/simulate_client.py

# 用 WAV 文件测试
python tests/simulate_client.py --file sample.wav
```

---

## 4. 前端部署（Windows TSF 输入法）

### 编译

```cmd
cd voime\frontend
cmake -B build
cmake --build build --config Release
```

编译产物：`build\Release\voime-ime.dll`

### 注册输入法

以**管理员**身份运行：

```cmd
regsvr32 build\Release\voime-ime.dll
```

### 启用输入法

1. 打开 **设置 → 时间和语言 → 语言和区域**
2. 在中文(简体)下点击 **添加键盘**
3. 选择 **Voime — 语音输入法**

### 使用

1. 确保后端服务已启动（`python -m voime_server.main`）
2. 打开任意文本编辑器（记事本、Word、VS Code）
3. 切换到 Voime 输入法（`Win+Space` 或语言栏）
4. 按 **F2** 开始录音，说话，再按 **F2** 结束
5. 识别的文字出现在光标位置

### 卸载

```cmd
regsvr32 /u build\Release\voime-ime.dll
```

---

## 5. 配置参考

```yaml
server:
  host: "127.0.0.1"     # 监听地址
  port: 9876             # 监听端口

asr:
  backend: "whisper_cpp" # faster_whisper | whisper_cpp | openai_realtime
  model: "base"          # tiny | base | small | medium | large-v3
  language: "zh"         # zh | en | auto
  device: "cpu"
  compute_type: "int8"

  vad:
    threshold: 0.5             # 语音概率阈值 (0-1)
    min_speech_duration: 0.3   # 最短语音段 (秒)
    silence_duration: 1.0      # 触发结束的静音时长 (秒)

  streaming:
    chunk_interval: 0.5        # 增量转录间隔 (秒)
    stable_count: 2            # N 次连续一致才标记为 stable

  whisper_cpp:
    binary: ""                 # whisper-cli 路径，空则自动探测
    model_path: ""             # ggml model 路径，空则自动探测
    threads: 4

  openai:
    api_key: "${OPENAI_API_KEY}"
    base_url: "https://api.openai.com/v1"

llm:
  enabled: false
  base_url: "http://127.0.0.1:11434/v1"
  api_key: "ollama"
  model: "qwen3:4b"
  timeout: 15
  system_prompt: "你是一个专业的文本校对助手..."
  user_prompt_template: "请修正以下语音输入文本：\n{text}"
```

---

## 6. GPU 支持矩阵

| 显卡       | 推荐后端       | 配置                                          |
|------------|---------------|-----------------------------------------------|
| NVIDIA     | faster-whisper | `device: cuda`                                |
| AMD        | whisper.cpp   | 编译 `-DWHISPER_VULKAN=1`，`device: vulkan`    |
| Intel Arc  | whisper.cpp   | 编译 `-DWHISPER_VULKAN=1`，`device: vulkan`    |
| Apple M 系列 | faster-whisper | `device: cpu` (CoreML 自动加速)               |
| 无 GPU     | faster-whisper | `device: cpu`, `compute_type: int8`           |

---

## 7. 故障排查

**faster-whisper 模型下载慢**
```bash
# 手动下载模型到缓存目录
export HF_ENDPOINT=https://hf-mirror.com
python -m voime_server.main
```

**whisper.cpp 找不到**
```bash
# 显式指定路径
whisper_cpp:
  binary: "/home/user/whisper.cpp/build/bin/whisper-cli"
  model_path: "/home/user/whisper.cpp/models/ggml-base.bin"
```

**Vulkan 初始化失败**
```bash
# 检查 Vulkan 驱动
vulkaninfo | grep "deviceName"
# 如无输出，安装 vulkan-sdk
```

**regsvr32 注册失败**
- 确认以管理员身份运行
- 确认 `voime-ime.dll` 路径不含中文或空格
- 检查 VS 运行库是否安装（VC++ Redistributable）

**IME 激活后不工作**
- 确认后端服务正在运行：`curl http://127.0.0.1:9876/health`
- 查看 DebugView 或 Event Viewer 中的日志

## 目录结构

```
voime/
├── README.md
├── server/                          # 后端 (Python, 跨平台)
│   ├── requirements.txt
│   ├── config.yaml.example
│   ├── tests/simulate_client.py
│   └── voime_server/
│       ├── main.py                  # 入口
│       ├── config.py                # 配置管理
│       ├── ws_handler.py            # WebSocket 服务
│       ├── asr/
│       │   ├── engine.py            # ASR 基类
│       │   ├── faster_whisper.py    # faster-whisper 后端
│       │   ├── whisper_cpp.py       # whisper.cpp 后端 (AMD Vulkan)
│       │   └── vad.py               # 语音活动检测
│       └── llm/
│           ├── engine.py            # LLM 基类
│           └── openai_compat.py     # OpenAI-compatible 后端
│
└── frontend/                        # 前端 (C++, Windows TSF IME)
    ├── CMakeLists.txt
    └── src/
        ├── main.cpp                 # DLL 入口
        ├── ime_service.h/.cpp       # TSF 输入法核心
        ├── audio_capture.h/.cpp     # WASAPI 音频采集
        ├── ws_client.h/.cpp         # WinHTTP WebSocket
        ├── registry.h/.cpp          # COM/TSF 注册
        └── globals.h                # GUID 定义
```
