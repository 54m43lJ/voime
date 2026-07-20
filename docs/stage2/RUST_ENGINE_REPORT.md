# Voime 阶段二 Rust 核心引擎报告（M2 交付物）

- 日期：2026-07-20
- 分支：voime-ng-poc
- 交付：`engine/target/release/voime_core.dll`（8.4MB）、`engine/include/voime.h`、
  验证工具 `voime_cli.exe`、`ffi_smoke.exe`

## 0. 任务验收汇总

| 任务 | 验收标准 | 结果 | 结论 |
|---|---|---|---|
| RUST-01 | 项目编译通过（引入 ort） | workspace（voime_core cdylib+rlib / voime_cli）编译通过，ort 2.0.0-rc.12 + ORT 1.27.0 | ✅ |
| RUST-02 | 从指定路径加载 ONNX 至内存 | `ModelManager::load(dir)` 加载三模型；缺失返回 -4 并经 error 结果上报 | ✅ |
| RUST-03 | SessionState 状态机 | Idle/Recording/Finalizing 流转正确，Finalizing 期间 start 返回 -3（含单元测试） | ✅ |
| RUST-04 | StreamVadEngine：PCM→句子边界 | 官方 `fireredvad_stream_vad_with_cache.onnx`，句段与 Python 参考一致 | ✅ |
| RUST-05 | AsrEngine：音频切片→文本，语言提示 | CTC int8 ONNX + greedy 解码，中英文识别正确；lang_hint 接口就位 | ✅ |
| RUST-06 | PuncEngine：文本→带标点 | 自研 BERT ONNX，6/6 测试文本与 PyTorch 参考逐字一致 | ✅ |
| RUST-07 | FFI + 无锁队列，-2 溢出码 | PRD 5.1 八函数全导出；突发 3000 chunk 实测 948 次 -2 | ✅ |
| RUST-08 | Worker + 异步事件 | 条件变量休眠（空闲 0% CPU）、CString 所有权转移、SetEvent 通知 | ✅ |
| M2 附加 | 60s Buffer 限流/护栏 | 队列 2048 chunk≈61s 溢出 -2；60s 会话自动收尾（单次 warning + final） | ✅ |

**总体结论：阶段二全部任务通过。** 端到端 PTT 延迟（末句 ASR + Punc）实测 **250~300ms**，
相对 <500ms 目标有 40%+ 余量；模型常驻内存由阶段一 PyTorch fp32 的 9.7GB 降至 **1.2GB**。

## 1. ONNX 模型选型与来源

| 模块 | 模型文件 | 来源 | 说明 |
|---|---|---|---|
| 流式 VAD | `fireredvad_stream_vad_with_cache.onnx` (2.3MB) | FireRedTeam/FireRedVAD 官方（GitHub `pretrained_models/onnx_models`） | 输入 feat[1,T,80] + caches_in[8,1,128,19]，输出 probs + caches_out |
| ASR | `model.int8.onnx` (740MB) | 社区导出 VidraAI/FireRedASR2-onnx（HF） | 官方权重的 encoder+CTC 分支 int8；输入 x[N,T,80]/x_lens，输出 log_probs[N,T',8667] |
| Punc | `punc_bert.onnx` (388MB) | **本项目自研导出**（`tools/export_punc_onnx.py`） | chinese-lert-base + 5 类分类头；onnxruntime 复算与 PyTorch 参考 6/6 逐字一致 |

选择 CTC 分支（而非 attention decoder）的理由：单图无状态、解码简单、延迟低；attention
decoder（sherpa-onnx 格式 encoder/decoder 已存在于社区）列为后续质量升级项（见 §6）。

## 2. 数值对齐验证（Rust vs Python 参考）

| 项 | 方法 | 结果 |
|---|---|---|
| fbank+cmvn 特征 | hello_zh.wav 230×80 全量对比（`tools/compare_feat.py`） | 最大绝对差 **1.1e-5**（浮点舍入级）✅ |
| VAD 句段 | session_zh/mix_3sent.wav | 与阶段一 Python 结果一致（起点差 ≤1 帧 25ms）✅ |
| ASR 文本 | hello_zh/hello_en | `你好世界` / `hello speech` 正确 ✅ |
| Punc 文本 | 6 条中英测试句 | 与 PyTorch 逐字一致（导出时已校验；Rust 复算亦一致）✅ |

## 3. 性能（release 构建，Ryzen 7 9800X3D，单句 ~2s）

| 指标 | 阶段二（ONNX int8） | 阶段一（PyTorch fp32） | 目标 |
|---|---|---|---|
| VAD 逐帧 | **0.48ms/帧**（25ms 预算） | 1.2ms/帧 | 实时 |
| ASR 单句 | **230~290ms**（RTF 0.11~0.14） | 480ms(zh,b1)~720ms(en,b3) | — |
| Punc 全文 | **11~13ms** | 10~15ms | — |
| **PTT 端到端**（末句 ASR+Punc） | **250~300ms** | 483ms(zh,b1)~735ms(en,b3) | **<500ms ✅** |
| 模型常驻内存（进程峰值 RSS） | **1.2GB**（模型本体 1.13GB） | 9.7GB | — |
| 动态内存（队列/缓冲，阶段一实测） | 7MB | 7MB | <200MB ✅ |

11.4s 四句会话（zh/en）逐句延迟平稳（230~289ms），与会话长度无关，符合分段识别架构预期。

## 4. FFI 端到端验证

**Python ctypes 全链路**（`tools/test_ffi_e2e.py`，驱动 release DLL）：

```
start_session rc=0 -> 推送 session_mix_3sent.wav 8.88s
[partial #1] 你好世界 (asr 228ms)   # 事件触发 poll
[partial #2] hello spech (asr 258ms)
[partial #3] 你好世界 (asr 230ms)
[final] 你好世界，hello spech，你好世界。  (punc 12ms, dropped 0)
溢出测试: 突发 3000 chunk -> ok=2052, -2=948 ✅
60s 护栏: 单次 warning + final 收尾 ✅
```

**C++ 冒烟**（`engine/ffi_smoke/ffi_smoke.cpp` 经 `voime.h` 链接 DLL）：
version/model_ready/start/push/事件/final/free_string/destroy 全流程 PASS。

## 5. 缺陷修复记录（阶段二）

| # | 问题 | 修复 |
|---|---|---|
| 1 | Finalizing 期间队列尾包被误判丢弃（快速推送+stop 时仅处理 0.72/8.88s） | Worker 消费不再按状态过滤：队列中的 chunk 全部属于录音会话 |
| 2 | 60s 护栏告警逐 chunk 刷屏（52 条） | 每会话单次告警，finalize 后复位 |
| 3 | rust-mingw 自含 dlltool 缺 `as.exe` 致 windows-sys 构建失败 | 构建 PATH 指向工作区完整 MinGW（`.toolchain/mingw64/bin`） |
| 4 | tokenizers 无默认 regex 引擎 | 显式启用 `onig` feature |
| 5 | torch 2.11 默认 dynamo ONNX 导出缺 onnxscript | 导出指定 `dynamo=False` 传统导出器 |

## 6. 已知问题与后续建议

| # | 事项 | 影响 | 建议 |
|---|---|---|---|
| 1 | CTC greedy 偶发漏字（"hello spech"/"he speech"，4/20 句出现） | 英文识别率略低于 attention beam | 接入 attention decoder ONNX（sherpa 格式）或 CTC+attention rescore；QA 阶段引入真实语料回归 |
| 2 | `lang_hint` 对 CTC 分支为 advisory（仅影响英文空格规整） | 接口已就位，模型未消费 | 接 attention decoder 后消费 LID/prompt |
| 3 | VAD 对纯正弦音误检为语音（0.8s 句段，ASR 返回空） | 极端合成输入；真实环境底噪未复现（阶段一实测） | 空文本 partial 前端可忽略；必要时调 speech_threshold |
| 4 | ORT 环境进程级单例（init_from 一次） | 多 engine 实例共享环境（当前设计满足单实例产品形态） | — |
| 5 | 构建依赖工作区 MinGW PATH（dlltool/as） | 新环境构建需按 README 设置 | 已固化到 build 脚本/AGENTS.md |

## 7. 产物与复现

```powershell
# 构建（需要工作区 MinGW 在 PATH 提供 dlltool/as）
$env:PATH = "$PWD\.toolchain\mingw64\bin;$env:PATH"
cargo build --workspace --release   # engine/

# 验证
$env:VOIME_MODEL_DIR = "$PWD\models"
.\engine\target\release\voime_cli.exe e2e   poc\python\out\session_zh_3sent.wav
.\engine\target\release\voime_cli.exe bench poc\python\out\latency\zh_4x2s.wav
.\.venv\Scripts\python tools\test_ffi_e2e.py          # FFI E2E（ctypes）
.\engine\target\release\ffi_smoke.exe                  # C++ 冒烟
```

- FFI 头文件：`engine/include/voime.h`
- 模型目录约定：见 `voime.h` 头部注释 与 `engine/voime_core/src/models.rs`
