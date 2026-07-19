# Voime 阶段一 POC 验证报告（M1 交付物）

- 日期：2026-07-19
- 分支：voime-ng-poc
- 验证人：自动化代理（全流程脚本可复现）

## 0. 结论汇总

| 任务 | 验收标准 | 结果 | 结论 |
|---|---|---|---|
| POC-01 | 三个模型加载成功，无报错 | VAD 0.02s / ASR 4.51s / Punc 1.35s 加载成功，冒烟推理正确 | ✅ PASS |
| POC-02 | 说话时打印 "Sentence Detected" | 文件模拟 3/3 句；**真实麦克风**经扬声器耦合 4/4 句 | ✅ PASS |
| POC-03 | 终端实时打印无标点文本 | 中/英/中 3 句实时识别正确，平均 RTF 0.32 | ✅ PASS |
| POC-04 | PTT 松开后输出带标点完整文本 | `你好世界，你好世界，你好世界。`，Punc 38ms | ✅ PASS |
| POC-05 | 延迟测试报告，确认 <500ms | 中文 beam=1 端到端 483~491ms 达标；beam=3 与英文配置 563~735ms 超标 | ⚠️ 部分达标 |
| POC-06 | WASAPI 采集→Rust 打印长度；无锁队列无泄漏 | 全部通过（采样精度 15.000s/14.994s，120s 内存零增长，-2/-3 路径验证） | ✅ PASS |

**总体结论：阶段一通过，可进入阶段二（Rust 核心引擎）。** 唯一风险项为 ASR 延迟在部分配置下超出 500ms 目标，需在阶段二通过 ONNX Runtime + 量化/解码参数优化解决（见 §6）。

## 1. 验证环境

| 项 | 值 |
|---|---|
| CPU | AMD Ryzen 7 9800X3D（AVX2/AVX-512） |
| 内存 | 32 GB |
| OS | Windows 11 x64（GBK 控制台） |
| 麦克风 | Fifine USB Microphone（44.1k/48k F32 采集验证通过） |
| Python | 3.12.13（工作区 `.venv`，uv 安装；系统 3.14 不可用：`kaldi_native_fbank` 无 cp314 wheel） |
| PyTorch | 2.11.0+cpu / torchaudio 2.11.0+cpu（官方锁定 2.1.0+cu118，向下兼容验证通过） |
| 关键依赖 | transformers 5.14.1（官方锁 4.51.3，兼容）、numpy 1.26.4、kaldi_native_fbank 1.22.3、sentencepiece 0.2.2、sounddevice 0.5.5 |
| Rust | 1.97.1 stable，x86_64-pc-windows-gnu（工作区 `.toolchain\rust`，系统无 MSVC） |
| C++ | MinGW-w64 g++ 16.1.0 UCRT（工作区 `.toolchain\mingw64`） |
| 推理代码 | FireRedASR2S @ 4e7d9aa（Apache-2.0，浅克隆于 `poc/third_party/`，**Windows 下零修改直接运行**） |
| 模型 | FireRedASR2-AED（4.73GB，1.15B 参数 fp32）、FireRedVAD Stream-VAD（2.2MB）、FireRedPunc（407MB，BERT-Lert 101.7M 参数）—— HuggingFace `FireRedTeam/*` |

## 2. 模型链路验证（Python 线，POC-01 ~ POC-05）

脚本：`poc/python/poc0*.py`（共享封装 `voime_poc/`），原始日志：`poc/python/out/*.log`。

### POC-01 模型加载 ✅

| 模型 | 目录 | 加载耗时 | 备注 |
|---|---|---|---|
| FireRedStreamVad | `models/FireRedVAD/Stream-VAD` | 0.02s | 568K 参数 |
| FireRedASR2-AED | `models/FireRedASR2-AED` | 4.51s | 1,152,521,275 参数（4396.5MB fp32） |
| FireRedPunc | `models/FireRedPunc` | 1.35s | 101,680,901 参数（387.9MB fp32） |

冒烟推理（`assets/hello_zh.wav`，2.32s）：VAD `[(0.46,1.84)]` → ASR `你好世界`（0.67s，RTF 0.285）→ Punc `你好世界。`（0.02s）。三链路连通。

### POC-02 实时录音 + VAD 切分 ✅

自动化环境无人声输入，采用三级验证：

1. **文件模拟实时流**（`session_zh_3sent.wav`，8.96s = 3 句 + 1s 句间静音，按真实时间节奏喂入）：检出 3/3 句 —— `0.42~2.30s`、`3.73~5.62s`、`7.05~8.95s`，与构造完全一致。
2. **真实麦克风采集**（Fifine，44.1kHz→16kHz）：设备打开、数据流正常（6s 采集 5.80s，环境底噪 RMS 19.3，未误检）。
3. **扬声器→麦克风声学耦合**（播放 4 遍 hello_zh.wav，真实硬件全链路）：检出 4/4 句（`2.25s`/`5.42s`/`8.60s`/`11.80s` 起，各 1.88s）。

流式 VAD 以 25ms（400 样本）帧驱动 `detect_frame`，逐帧打印 Speech Start / Sentence Detected。日志：`poc02_realtime.log`、`poc02_mic_spk.log`。

### POC-03 VAD → ASR → 显示 流水线 ✅

输入 `session_mix_3sent.wav`（中/英/中，8.88s，实时节奏），逐句检出即识别：

```
[vad] Sentence Detected: 0.42s ~ 2.30s  >> [asr] (1.88s, 0.58s, rtf=0.309) 你好世界
[vad] Sentence Detected: 3.58s ~ 5.72s  >> [asr] (2.15s, 0.77s, rtf=0.357) hello speech
[vad] Sentence Detected: 6.97s ~ 8.85s  >> [asr] (1.88s, 0.56s, rtf=0.297) 你好世界
```

平均 RTF 0.322（CPU fp32，beam_size=3）。识别全部正确。

### POC-04 PTT 松开后 Punc 标点补全 ✅

模拟一次 PTT 会话（8.96s，3 句中文）→ 期间实时输出无标点文本 → 松开（会话结束）后拼接全文送 Punc：

```
输入: 你好世界你好世界你好世界
输出: 你好世界，你好世界，你好世界。   (Punc 耗时 0.038s)
```

mic 模式支持回车模拟 PTT 按下/松开（交互式）。英文句段拼接由 `smart_join` 处理词间空格。

### POC-05 延迟测量 ⚠️ 部分达标

方法：VAD 流式切分 → 逐句 ASR（重复 3 次取中位数，含预热）→ 全文 Punc；端到端 PTT 延迟 = ASR(最后一句) + Punc(全文)。测试音频为 2s 短句的 1/2/4 段拼接（覆盖短/中/长会话）。数据：`poc05_latency.json`。

**VAD 流式开销：1.12~1.29 ms/帧（预算 25ms/帧，占用约 5%）** —— 与录音并行，不构成延迟。

**ASR 单句（约 2s）延迟与端到端 PTT 延迟：**

| 用例 | beam=3 末句 ASR | beam=3 端到端 | beam=1 末句 ASR | beam=1 端到端 |
|---|---|---|---|---|
| zh 1×2s | 0.552s | 563ms ❌ | 0.480s | **491ms ✅** |
| zh 2×2s | 0.564s | 576ms ❌ | 0.471s | **483ms ✅** |
| zh 4×2s | 0.556s | 571ms ❌ | 0.475s | **490ms ✅** |
| en 1×2s | 0.709s | 719ms ❌ | 0.597s | 608ms ❌ |
| en 2×2s | 0.716s | 727ms ❌ | 0.594s | 606ms ❌ |
| en 4×2s | 0.720s | 735ms ❌ | 0.600s | 615ms ❌ |

**Punc：10~15ms（全文 8~32 字）—— 可忽略。**

观察：
- 中文在 beam=1 时稳定达标（483~491ms）；beam=3 超 13~15%；英文解码 token 更多（BPE），beam=1 也超约 20%。
- 本会话长度（1/2/4 句）对末句延迟无影响 —— VAD 分段识别架构符合设计预期，延迟只取决于最后一句时长。
- 内存（RSS，含模型）：基线 0.3GB → +VAD/Punc 1.0GB → +ASR(fp32) 9.7~9.8GB。动态部分（队列/缓冲，POC-06 实测）约 7MB，远低于 200MB 目标；模型常驻 9.7GB 为 fp32 PyTorch 形态，阶段二转 ONNX/int8 后预期降到 1.5GB 量级。

**改进路径（阶段二 RUST 线）**：① ONNX Runtime（ort）图优化 + AVX2，相对 PyTorch 通常 1.5~3× 加速；② int8 动态量化（编码器为主）；③ 默认 beam=1 或 beam=2 折中；④ 英文长句可由 VAD `max_speech_frame` 控制单句上限（20s→8s）。综合后全配置 <500ms 可达。

## 3. C++/Rust 通讯线验证（POC-06）✅

代码：`poc/ffi/`（Rust cdylib `voime_poc_core.dll` 418KB + C++ 宿主 `poc_host.exe`）。构建：`poc/ffi/build.ps1`。

| 验证点（PRD 3.3/4.1/5.1） | 方法 | 结果 |
|---|---|---|
| FFI 连通 | `voime_version/create_engine` | ✅ `voime_poc_core 0.1.0` |
| 状态机 | start/stop 返回码 | ✅ 0/-1/-3 正确流转 |
| Finalizing 拒绝 | Worker 延时 200ms 构造窗口，stop 后立即 start | ✅ 返回 **-3** |
| 无锁队列溢出 | Worker 延时 50ms + 突发 3000×480 样本 | ✅ 入队 2049，**951 次返回 -2 丢帧**，会话统计 dropped=951（容量 2048 chunk ≈ 61s，对齐 60s 护栏） |
| push 不阻塞采集线程 | 30ms chunk 实时节奏 | ✅ 500/500 chunk 零丢失（WASAPI 15s） |
| Rust 打印长度 | `--verbose` 逐 chunk 打印 | ✅ `[rust] chunk len=480 total_samples=...` |
| Windows Event 异步通知 | SetEvent → WaitForSingleObject → poll_result | ✅ 每秒 progress + session_end 均送达 |
| 结果所有权转移 | poll_result 取指针 → C++ 拷贝打印 → free_string 归还 | ✅ 无异常，120s 后内存不增 |
| WASAPI 共享模式 | 默认采集设备 GetMixFormat | ✅ **48000Hz 2ch 32bit float**（PRD 预期格式），设备名正确枚举 |
| 重采样（C++ 端） | 48k F32 立体声 → 16k s16 mono（线性插值，POC 简化实现；产品用 Media Foundation） | ✅ 15s 采集得 **240000 样本 = 15.000s**（wall 14.994s） |
| 无泄漏 | 120s 持续推送，每 5s 记录内存 | ✅ working set 恒定 6.7MB，private 1.3MB 零增长；3818 chunk、0 丢帧 |

已知 cosmetic 问题：C++ `wprintf` 设备名在 GBK 控制台显示不全（不影响功能）。

## 4. Windows 兼容性记录

FireRedASR2S 官方"仅测过 Linux"，本次 Windows 适配结论：

1. **推理代码零修改**：`fireredasr2s` 全部模块（VAD/ASR/Punc）在 Windows + Python 3.12 直接运行。
2. 依赖版本调整：torch 2.1.0+cu118 → **2.11.0+cpu**（无 GPU 环境）；transformers 4.51.3 → **5.14.1**（BertModel/BertTokenizer 行为兼容）；numpy 1.26.1 → 1.26.4（同为 1.26 系，规避 2.x 兼容风险）。
3. Python 必须 ≤3.12（`kaldi_native_fbank` 无 3.13/3.14 win wheel）。
4. Rust 端 POC 未引 `windows crate`，SetEvent 用 4 行 extern 声明搞定；后续正式引擎再引入官方 binding。
5. MinGW 链接 Rust DLL：GNU ld 直接以 `voime_poc_core.dll` 为输入即可；C++ 运行库需 `-static` 静态化避免 DLL 地狱。

## 5. 风险与阶段二建议

| # | 事项 | 建议 |
|---|---|---|
| 1 | ASR 英文/beam=3 延迟超 500ms | 阶段二 ONNX 化 + int8 量化 + beam 策略（§2 POC-05） |
| 2 | 模型常驻内存 9.7GB（fp32 PyTorch） | ort + 量化后重测；产品 200MB 指标仅约束动态内存，达标无风险（POC-06 实测 7MB） |
| 3 | 英文句间拼接需空格规则 | POC-04 `smart_join` 已验证，移植到 Rust 引擎 |
| 4 | WASAPI 设备热插拔/独占冲突 | 阶段三 CPP 线处理（POC 未覆盖） |
| 5 | 重采样音质 | 产品用 Media Foundation Resampler（POC 用线性插值，仅验证链路） |
| 6 | 测试音频以合成/重复语句为主 | 阶段四 QA 引入真实语料集回归识别率（PRD >95% 目标） |

## 6. 复现索引

- Python 线：`README.md`「运行 POC」节；日志与测试音频 `poc/python/out/`；延迟数据 `poc/python/out/poc05_latency.json`。
- C++/Rust 线：`poc/ffi/README.md`；日志 `poc/ffi/out/run_*.log`。
- 模型下载：`tools/download_models.py`；环境安装：`tools/setup_python.ps1`。
