# IMPL — 开发中的实现补充与文档差异

本文记录开发过程中遇到的、README / USER / CORE / PROVIDER / BUILD 没有覆盖的细节，以及文档与最终实现不一致、需要做出工程决策的地方。

正式契约仍以项目文档为准；本文只记录“文档之外”的事实，避免把实现细节重新写回用户文档。

## 1. 文档问题与处理结果

| # | 文档位置 | 问题 | 实现与处理 |
|--:|----------|------|------------|
| 1 | PROVIDER §3.1 | 写的是“音频错误通过独立通道交给工作线程”，但 CORE §4.8 的 `capture::start` 只有音频数据通道 | 错误回调向同一有界通道写入零长度错误标记；工作线程维护连续错误计数。PROVIDER 已更新说明 |
| 2 | PROVIDER §4.2 | 声称 fbank 参数、帧移、分段上限由 manifest 声明，但示例没有这些字段 | 新增可选字段 `fbank { num_bins, frame_ms, shift_ms }`、`max_segment_ms`、`punc.max_tokens`；旧包缺省时使用 80 / 25 / 10、10000、400 |
| 3 | PROVIDER §4.2 | 模型包目录没有 `cmvn.ark`，但 FireRedVAD / FireRedASR2-AED 实际需要 CMVN | 导出时把 CMVN 的 `(x - mean) * istd` 融合进 VAD ONNX；ASR encoder 包装器内部应用 CMVN。模型包运行期不需要 `cmvn.ark` |
| 4 | PROVIDER §4.2 | 官方 FireRedPunc `out_dict` 的 id 0 是 `<space>`，不是空字符串 | 导出脚本把 id 0 映射为 `""`；最终文本从 ASR 原文重建，英文词间空格不依赖 Punc 的 space 标签 |
| 5 | PROVIDER §4.2 | Punc 包只声明 `vocab.txt`，官方实际还有 `tokenizer.json` | 运行时从 `vocab.txt` 构建 BERT WordPiece；实测该 vocab 与官方 `chinese-lert-base` 的 vocab 完全一致 |
| 6 | PROVIDER §4.2 | 没有描述 Punc 的 `[CLS]` 对齐、`token_type_ids`、输出长度 | 导出包装器内部加 `[CLS]` 并从输出中去掉；运行时输入 `input_ids` / `attention_mask` / `token_type_ids`，输出长度与输入 token 数一致 |
| 7 | PROVIDER §4.2 | 没有说明 ASR 特殊 token 的来源 | 运行期从 `tokens.txt` 查找 `<sos>` / `<eos>` / `<pad>`；官方词表实际 id：`<blank>`=0、`<unk>`=1、`<pad>`=2、`<sos>`=3、`<eos>`=4 |
| 8 | PROVIDER §4.2 | 没有覆盖 ONNX >2 GiB 的 external data | 安装脚本对量化后模型优先内联；无法内联时保留分片，manifest 只校验主文件 SHA-256。默认 int8 包已实测为内联 |
| 9 | CORE §4.5 | `SegmentDropped` 只清除 `recording` / `finalizing`，会让临时行残留成假结果 | 同时清空 `partial`；CORE 已更新 |
| 10 | CORE §4.1 / §4.5 / §4.18 / §5.6 | `run` 在 `main.rs`、`app::run` 按值接收 Pipeline、`Tui::draw` 接收 `&App` | 实际为 `src/lib.rs::run`、`app::run(mode, &mut Pipeline, ..)`、`Tui::draw(&mut App)`，以便主线程关闭 Pipeline 并更新滚动状态；CORE 已更新 |
| 11 | CORE / PROVIDER | `kaldi-fbank` 实际 crate 名为 `kaldi-native-fbank` | 文档与依赖统一 |
| 12 | CORE SBOM | Python 依赖写成“固定版本”，实际使用版本范围以兼容 Python 3.10+ / 新版本 | 文档改为“版本范围” |
| 13 | CORE §4.6 | `MAX_SEGMENT_MS` 称由常量和 VAD 模型包决定，但 manifest 没有字段 | 增加 `max_segment_ms`，默认 10000，启动时校验不超过 60000 |
| 14 | USER §2 | 分段上限“由 VAD 模型包声明”，但原 manifest 缺少对应字段 | 同上；PROVIDER 示例已补 |
| 15 | BUILD | 原文档没有安装脚本的 HuggingFace 来源、缓存、token、磁盘占用说明 | BUILD 已补；安装脚本新增 `HF_TOKEN` 交互提示 |
| 16 | README Credits | 原表把 `kaldi-native-fbank` 写成 `kaldi-fbank`，且未列 `tokenizers` | 已修正并补列 |

## 2. 模型包与导出

### 2.1 新增的 manifest 字段

```json
{
  "schema": 1,
  "sample_rate": 16000,
  "fbank": { "num_bins": 80, "frame_ms": 25, "shift_ms": 10 },
  "max_segment_ms": 10000,
  "vad": { "...": "..." },
  "asr": { "...": "..." },
  "punc": { "...": "...", "max_tokens": 400 }
}
```

- 三个字段都是可选的；旧 manifest 不写时分别使用 80/25/10、10000、400。
- `fbank` 同时用于 VAD 和 ASR，避免两处各自硬编码。
- `max_segment_ms` 是运行期分段上限，不是 ASR 的 60 s 硬上限；启动时仍校验不超过 60 s。
- `punc.max_tokens` 是 FireRedPunc 单次推理上限；超长文本由 Punc 内部切块。

### 2.2 实际 ONNX 张量契约

导出脚本生成的实际契约（与文档示例中的“通用占位名”不同）：

```text
VAD
  inputs:
    feat       float32 [1, T, 80]
    caches_in  float32 [8, 1, 128, 19]
  outputs:
    probs      float32 [1, T, 1]
    caches_out float32 [8, 1, 128, ...]

ASR encoder
  inputs:
    features   float32 [1, T, 80]
  outputs:
    encoder_out float32 [1, T', 1280]

ASR decoder
  inputs:
    tokens      int64   [1, U]
    encoder_out float32 [1, T', 1280]
  outputs:
    logits      float32 [1, U, 8667]

Punc
  inputs:
    input_ids      int64 [1, T]
    attention_mask int64 [1, T]
    token_type_ids int64 [1, T]
  outputs:
    logits         float32 [1, T, 5]
```

- VAD 的 `caches_out` 最后一维在 ONNX 元数据里可能是动态的；manifest 只记录名字，运行期状态尺寸由输入 `caches_in` 决定。
- ASR 词表 8667 来自官方 `dict.txt`；Punc 类别 5 来自官方 `out_dict`。
- `verify_models.py` 目前对 VAD cache 形状 `[8,1,128,19]`、上述张量名做了硬编码校验；上游模型结构变化时需要同步修改。
- `SessionFactory::build` 只校验 manifest 声明的张量是否存在、类型/秩/静态维度是否一致；动态维度 `-1` 视为通配。

### 2.3 CMVN 与 fbank 标度

- 内部音频是归一化 `f32 [-1, 1]`。
- Kaldi fbank 训练输入是 int16 量级，因此 `Fbank` 在送特征提取器前乘以 `32768.0`。不乘会导致 fbank 能量整体偏小，VAD / ASR 质量明显下降。
- VAD / ASR 的 CMVN 都在导出时融合：
  - VAD：在官方 ONNX 图前插入 `Sub(mean)` / `Mul(istd)` 节点。
  - ASR encoder：PyTorch 包装器内使用注册 buffer 做 `(features - mean) * istd`。
- 运行期模型包不包含 `cmvn.ark`。

### 2.4 external data 与量化

- `torch.onnx.export` 在 FP32 大模型上会生成大量 external data 分片（命名不一定是 `.data`）。
- 默认安装流程：
  1. 导出 FP32；
  2. ONNX Runtime 动态 int8 量化；
  3. 尝试把量化模型内联成单个 `.onnx`；
  4. 删除已不再被最终模型引用的旧分片；
  5. 校验、写 manifest、原子安装。
- 默认 int8 真实模型包实测约 1.3 GB，ASR encoder/decoder、Punc、VAD 均为单文件内联。
- `--no-quantize` 下可能保留 FP32 external data；此时必须把分片和主 `.onnx` 一起复制。manifest 的主文件 SHA-256 不覆盖分片，这是当前格式的限制。

### 2.5 模型路径

- manifest 中的模型路径先按“相对路径、不允许 `..` / 绝对路径”校验，再在 `Config::resolve` 中 rebase 到 `--model-dir`。
- 因此 `ModelManifest` 在 pipeline 中使用的是绝对路径，模型校验仍使用相对路径。

## 3. 音频与特征

### 3.1 cpal 采集

- 只使用 `default_input_device` 或按 `DeviceTrait::name()` 精确匹配的指定设备。
- 只读取设备的 `default_input_config()`；运行期不枚举其他配置、不热切换设备。
- 只接受 `I16`、`U16`、`F32`。其他格式启动失败。
- `I16`：`sample as f32 / 32768.0`；`U16`：`(sample - 32768) / 32768.0`；`F32` 原样。
- 回调中只做格式转换与 `try_send`；通道容量 2048，按 10–20 ms 回调块约可覆盖 20–40 s 积压，避免同步 ASR 期间丢音。
- 运行期流错误使用零长度 `AudioChunk` 作为标记写入同一有界通道；工作线程连续收到 3 次后以 `Fatal` 退出。错误详情本身被丢弃。
- 因为错误标记与音频共用有界通道，通道满时标记可能被丢掉，连续错误计数存在低估可能。
- 退出时 `pause()` 后丢弃流。

### 3.2 重采样

- `StreamResampler` 目标固定 16 kHz、单声道；多声道先求平均。
- 使用 `rubato::FastFixedIn`，输入块长 1024，多项式 Septic。
- 输入采样率等于 16 kHz 时直接旁路，不初始化重采样器。
- 分段结束时不 flush 重采样器，最多可能丢掉不足 1024 个输入帧；44.1/48 kHz 下约 21–23 ms。
- 设备采样率/声道数在启动时固定；运行期变化视为致命错误。

### 3.3 fbank

- ASR 使用 `Fbank`（`kaldi_native_fbank::online::OnlineFeature`），每个语音段新建，结束后丢弃。
- VAD 使用 `StreamingFbank`：只保留当前 25 ms 窗口和滚动余量，每次推进 10 ms；连续监听不会累积整段特征。
- VAD 推理按 100 ms 批处理（`VAD_BATCH_MS`）。逐 10 ms 调用 ONNX 时单次约 2.3 ms，批处理后每 10 ms 音频约 0.28 ms。
- 两者都使用 80 bins、25 ms 窗、10 ms 帧移、`snip_edges=true`、`dither=0`、无 energy、log fbank、power 频谱。
- 参数从 manifest 的 `fbank` 读取；默认值只是兼容旧包的 fallback。
- VAD 的 10 ms 音频单元与 fbank 输出帧存在约 25 ms 的初始延迟；VAD 推理按 100 ms 批处理后，事件最多再晚约 100 ms 到达分段器。pipeline 用 `frame_audio` 队列把“第 i 帧”与“第 i 个 10 ms 音频块”配对，段边界重置时保留当前批次剩余音频。

## 4. VAD 与分段

### 4.1 VAD 后处理

文档只描述了 `VadConfig` 字段，没有给出具体数值，也没有覆盖官方 FireRedVAD 的完整后处理。当前实现：

- 语音阈值：0.5；
- 最短语音：80 ms；
- 最短静音（continuous）：200 ms；
- PTT 静音自动结束：1200 ms；
- 最大段长：manifest 的 `max_segment_ms`，默认 10000 ms。

没有实现官方 Python 后处理中的 5 帧概率平滑、`pad_start_frame` 等细节；这些由 Segmenter 的最短语音/静音计数近似替代。若后续要更贴近官方行为，需要补充平滑窗口和预卷参数。

### 4.2 Segmenter 行为

- 未激活时只累积连续语音帧；达到 `min_speech_ms` 后才产生 `SpeechStart`，并把之前的 pending 音频作为段首。
- 激活后所有帧都进入段；连续静音达到阈值或段长达到上限时产生 `SpeechEnd`，并带 `SegmentEndReason::{Silence, MaxDuration}`。
- PTT 在 `MaxDuration` 结束一段后仍保持 `collecting`，继续把余下按住期间的语音分成下一段；只有静音结束或松键才停止。
- 段尾静音不会裁剪，会原样送入 ASR。
- `force_end` 只返回已经激活的段；不足 `min_speech_ms` 的 pending 语音被丢弃，不产生结果行。
- `start_ms` / `end_ms` 以 10 ms 帧计算，`start_ms` 是首帧起点的近似值。
- 段结束后 VAD 与 Segmenter 状态清零；continuous 的下一段重新开始，PTT 仅在静音结束或松键后回到等待。

### 4.3 错误处理

- VAD 推理失败：丢弃当前段，continuous 继续监听；PTT 回到等待按键。
- VAD 失败不会触发进程退出。
- 音频流连续失败 3 次才触发 `PipelineEvent::Fatal`，由 App 恢复终端并以 1 退出。

## 5. ASR

### 5.1 Encoder

- 导出包装器只暴露 `features` 输入，不暴露 `input_lengths`。
- 包装器内部：
  - 应用 CMVN；
  - 按官方 Conformer 前端补 3 帧；
  - 用全 1 mask（没有 padding），因此不需要长度输入。
- 输出 `encoder_out [1, T', 1280]`，`T'` 随输入动态变化。
- 运行期对超过 60 s 的段直接报错；pipeline 使用 manifest 的 `max_segment_ms` 限制段长，默认 10 s。

### 5.2 Decoder 与 beam search

- 导出的 decoder 没有 KV cache；beam search 每一步都用完整 token 前缀重新跑 decoder。
- 默认 `beam_size=3`、`max_decode_len=256`；临时识别固定使用 `beam_size=1`，最终识别仍用 3。
- 每步只取词表 top-k，用 `select_nth_unstable_by` 代替对 8667 个词全排序。
- 目标 mask 在导出时使用 float 0/1 的因果 mask（ONNX 对 uint8 位运算支持不好）；交叉注意力不传 mask（无 padding）。
- 没有实现官方的 `softmax_smoothing`、`length_penalty`、`eos_penalty`。
- 初始 token 使用 `<sos>`；`<eos>` 结束；特殊 id 从 `tokens.txt` 查找，找不到时回退 1/2/0。
- 输出取最后一个位置的 logits，做 log-softmax 后按累计分数保留 beam。
- 重新跑完整前缀的代价是 O(U²) 级别；短句很快，长句/低配 CPU 上需要关注延迟。

### 5.3 文本解码

- `tokens.txt` 约定一行一个 token，行号即 id；官方 `dict.txt` 的 `token id` 格式在导出时转换。
- 解码时过滤 `<...>` / `[...]` 特殊 token，处理 SentencePiece `▁` 空格、`<space>` 和 `<0xAB>` byte fallback。
- ASR 解码本身不做大小写转换；最终英文大小写与中文/英文标点由 Punc 后的英文修正步骤处理。

## 6. FireRedPunc

### 6.1 分词与模型

- 运行时从 `punc/vocab.txt` 构建 WordPiece，并挂 `BertNormalizer` + `BertPreTokenizer`，与官方 `tokenizer.json` 的分词行为一致。
- 导出包装器内部补 `[CLS]`，输出时去掉 CLS，所以运行期 logits 与输入 token 一一对齐。
- 输入 `attention_mask` 全 1（文本不 padding）；`token_type_ids` 全 0。
- 输出类别数为 5：id 0 空、id 1 `，`、id 2 `。`、id 3 `？`、id 4 `！`。

### 6.2 标点插入与英文修正

- 按 tokenizer 的 **byte offsets** 在原始文本对应位置插入标点；不是按 token 字符串重新拼接。
- 如果模型输出长度是输入 token 数 + 1，会尝试跳过首行（CLS）；当前导出器已经去掉 CLS。
- 超长文本按字符上限切块，切块后仍超 token 上限时递归二分；不静默丢弃尾部。
- 标点后增加了一个官方 `RuleBaedTxtFix` 的近似实现：
  - 全文转小写；
  - 英文词相邻的中文标点转换为 ASCII 标点；
  - `, . ? !` 后接英文字母时补空格；
  - 首个字符为英文时大写；
  - 独立的 `i` / `i'm` 等转大写。
- 该修正会改变英文大小写，属于官方 Punc 的文本规整行为，不是新增的润色功能。

## 7. Pipeline 与 App

### 7.1 事件与顺序

- `PipelineEvent` 增加了 `Fatal(String)`，用于音频流持续失败；App 收到后恢复终端并返回错误。
- continuous 的段结束后，`finalize` 在同一个 `handle_audio` 内立即执行（队列容量仍为 2，但通常不会积压），因此事件顺序是：
  - `Recording(false)` → `Final` → 下一段的 `Recording(true)`。
  这样 App 不会把上一段的 Final 误当成新段的状态。
- PTT 收到 `Recording(false)` 后置 `finalizing=true`；continuous 只清 `recording`。

### 7.2 音频边界处理

- `StartSegment` 命令执行前会丢弃音频通道里已排队的块，避免按下 Space 之前的音频进入新段。代价是如果工作线程正好积压，会丢掉按下前尚未处理的音频；这些音频本来也不属于新段。
- `FinishSegment` 命令执行前会先消费音频通道中已排队的块，保证按键前采集的音频尽量进入最终段；排空期间临时识别被抑制。
- 重采样器/分段器的 pending 样本在 Start 时清理；段结束时重置 VAD 与 Segmenter 并清空帧配对队列，不强制 flush 重采样器。

### 7.3 临时识别

- 临时识别是同步的：`emit_partial` 在 pipeline 工作线程内执行，不会与最终识别并行。
- 每次只识别当前段最后 `PARTIAL_MAX_MS`（6 s）；超长段会截断，事件文本以 `…` 开头。这样单次临时识别成本有上限，不会随段长线性增长。
- 下一次临时识别同时满足两个条件：至少新增 `PARTIAL_MIN_NEW_AUDIO_MS`（800 ms）音频，且距上次开始至少 `PARTIAL_INTERVAL_MS`（800 ms）；实际间隔还会按上次耗时动态放大（至少 2 倍耗时）。因此慢推理结束后不会在下一帧立刻再次触发。
- 临时识别用 greedy（`beam_size=1`），最终识别用 beam 3。
- `FinishSegment` 排空音频通道时会抑制临时识别，避免把松键时的积压音频逐帧变成临时推理。
- 最终识别的优先级通过每次音频帧后立即 `drain_queue` 体现；正在执行的临时识别无法被中途取消。
- 最终识别仍是同步的：10 s 段在测试机上约 2.5–3 s，期间音频进入 2048 块缓冲；连续模式在段边界会有可感知的延迟，但不再丢音。

### 7.4 App / UI

- `Tui::draw` 改为接收 `&mut App`，因为需要把实际滚动偏移和终端高度写回 `App`。
- `App` 额外保存 `fatal`、`viewport_height`；`SegmentDropped` 清空 partial，避免留下没有最终结果的假行。
- Final 事件：追加结果行、清空 partial、清除 recording/finalizing、整体写剪贴板；写失败忽略。
- 滚动状态：`follow=true` 时每次绘制把 offset 拉到最底；PageUp/Home 停止跟随；只有 End 恢复跟随；PageDown 不恢复。
- 终端恢复：正常返回、错误返回、panic hook、`Drop` 四条路径都会尝试恢复 raw mode / 备用屏幕 / 光标 / 鼠标上报。

## 8. 安装脚本

### 8.1 下载来源

| 内容 | 来源 |
|------|------|
| FireRedASR2-AED 权重 | HuggingFace `FireRedTeam/FireRedASR2-AED` |
| FireRedPunc 权重 | HuggingFace `FireRedTeam/FireRedPunc` |
| FireRedVAD ONNX / CMVN | GitHub `FireRedTeam/FireRedVAD` raw 文件 |
| FireRedASR2S 导出代码 | GitHub `main` 分支 tarball |

- `--source-dir` 可提供本地 ASR/Punc 权重目录，跳过 HuggingFace。
- `--code-dir` 可提供本地 FireRedASR2S 源码。
- VAD 本地文件不存在时仍会从 GitHub 下载。

### 8.2 HF_TOKEN 交互

- 需要 HuggingFace 下载且本地缓存不完整时，检查 `HF_TOKEN` / `HUGGING_FACE_HUB_TOKEN`。
- 未检测到时：
  - 交互终端：警告并提示输入，输入不回显；直接回车走未认证下载；
  - 非交互环境：警告后直接未认证下载，不阻塞。
- 输入的 token 只写入当前进程环境变量，不写配置文件。
- 缓存标记为 `<cache>/weights/<repo>/.voime-complete`；两个 HF 权重都完整时不再提示。
- `scripts/install_models.sh` / `.ps1` 继承 stdin，因此提示可以直接工作。

### 8.3 安装与原子替换

- 在目标目录同盘的 `target.parent` 下创建临时 stage，保证 rename 同文件系统。
- 安装前若目标存在，先改名为 `.backup-<pid>`，再 rename stage 到目标；失败时恢复备份。
- 成功后删除备份；`--keep-stage` 可在失败时保留 stage。
- 导出、量化、内联、清理、校验、写 manifest 都在 stage 内完成，最后一步才替换正式目录。

### 8.4 Python 依赖

- `requirements-export.txt` 用版本范围而不是精确 pin，以兼容 Python 3.10+ 和当前 PyTorch/ONNX Runtime 版本。
- torch/torchaudio 使用 CPU 索引。
- 安装脚本只依赖 Python 做模型转换，Rust 二进制运行时不需要 Python。

## 9. 验证与已知限制

### 9.1 已验证

- `cargo fmt --all --check`
- `cargo clippy --all-targets`
- `cargo test`：26 个单元测试 + 2 个 clipboard 测试 + 3 个 segmenter 测试
- `cargo build --release`，产物约 31 MB
- 真实官方权重安装：下载、导出、int8 量化、内联、校验、原子安装
- 真实模型包约 1.3 GB，路径为测试机上的 `/mnt/DATA/Work_linux/voime-dev-models`
- 真实音频：
  - 中文 `hello_zh.wav` → `你好世界。`
  - 英文 `hello_en.wav` → `Hello speech.`
- PTT、continuous、PTT 静音自动结束、TUI 红点/临时结果、真实麦克风采集、剪贴板初始化均已运行验证

### 9.2 限制

- Windows 构建和运行未在本机验证。
- 单可执行 + 静态 ONNX Runtime 已在 Linux 验证；Windows 若 `ort` 产出动态库，发布包需要同时携带该库。
- 未实现真实 X11/Wayland 剪贴板内容回读；环境无剪贴板管理器时，进程退出后内容可能丢失，符合 PROVIDER §4.4 的说明。
- VAD 没有官方 5 帧平滑和 pad_start；分段阈值是工程默认值，不是模型包声明值。
- ASR decoder 没有 KV cache，beam search 会重跑完整前缀；低配 CPU 的延迟仍需按实际硬件评估。测试机（4 intra-op 线程）10 s 段 encoder≈1.5 s、decoder≈0.8 s，整段≈2.5–3 s；20 s 段约 5–6 s（仅作放宽上限时的参考）；8 线程没有明显收益，属于当前 int8 CPU 模型的技术限制。
- 临时识别同步执行且只覆盖段尾 6 s；最终识别同步执行，段边界仍会阻塞音频处理，依赖 2048 块音频缓冲兜底。彻底消除边界延迟需要把 ASR 移到独立工作线程。长语音卡顿的修复记录见 §10。
- 默认 int8 量化只做了 ONNX Runtime 可加载性/形状校验，没有在固定验证集上评估 WER/F1；正式发布前应按 PROVIDER 的要求补充质量验证。
- 安装脚本使用 GitHub `main` 分支源码和官方权重 URL，上游变化可能影响可复现性；需要固定时可加 commit / revision 参数。
- manifest 只对主 ONNX 文件计算 SHA-256；external data 分片不在哈希范围内。
- 当前仓库没有提交模型权重，也没有提交许可证文件；权重许可随模型包分发。

## 10. 已修复：长语音识别卡顿

### 10.1 现象

长语音（约 10 s 以上）时 CPU 持续占用，但结果迟迟不出现，最终文本明显缺句。用 20 ms 音频块模拟采集时，30 s PTT 语音触发 67 次临时识别，丢弃 25.16 s 音频，松键后 32.9 s 才结束，最终只剩约 2 句；全程 CPU 约 255%。

### 10.2 根因

1. 临时识别被逐帧重复触发。旧 `emit_partial()` 在推理开始前更新 `last_partial`；长段的单次推理本身已超过 `PARTIAL_INTERVAL_MS`（800 ms），推理结束后下一帧音频立即再次满足 `now - last_partial >= 800 ms`，实际变成每 10 ms 音频触发一次临时识别。
2. `FinishSegment` 排空音频通道时仍走 `handle_audio()`，没有关闭临时识别。松键时通道中的积压音频会逐帧触发临时识别，所以松键后还会继续卡。
3. 临时识别和最终识别都在音频工作线程内同步执行。推理期间 VAD、分段和重采样全部停摆；旧音频通道只有 16 块（约 0.3 s），`try_send` 满后静默丢块。长段一旦丢音，分段器迟迟到不了段尾，最终结果必然缺内容。
4. PTT 达到 `max_segment_ms` 后直接 `collecting = false`，用户继续按住 Space 说出的后续语音不会进入识别。
5. 附带开销：VAD 每 10 ms 跑一次 ONNX；临时识别每次重算整段 fbank 和整段 ASR；beam search 每步对 8667 个词全排序；临时识别也使用 beam 3。

### 10.3 处理

- 临时识别只取当前段末尾 `PARTIAL_MAX_MS`（6 s），并使用 greedy（`PARTIAL_BEAM_SIZE=1`），单次成本不再随段长增长。
- 下一次临时识别同时要求新增音频量至少 `max(PARTIAL_MIN_NEW_AUDIO_MS, 2 × 上次耗时)`，且距上次开始至少 `max(PARTIAL_INTERVAL_MS, 上次耗时)`；慢推理后不会在下一帧立刻重触发。
- `FinishSegment` 排空音频通道期间设置 `suppress_partials`，积压音频只做 VAD/分段，不再触发临时识别。
- VAD 改为 `VAD_BATCH_MS=100` 批处理；实测每 10 ms 音频的 VAD 开销从约 2.27 ms 降到约 0.28 ms。
- 音频通道容量 16 → 2048，覆盖同步最终识别的积压；`run` 使用 `select_biased!` 让 `StartSegment` / `FinishSegment` / `Shutdown` 优先于音频。
- `SegmenterEvent::SpeechEnd` 增加 `SegmentEndReason`；PTT 在 `MaxDuration` 后保持 `collecting`，把余下语音分成下一段，只有静音结束或松键才停止。
- beam search 每步只取词表 top-k，用 `select_nth_unstable_by` 代替全排序。
- 分段上限默认从 20 s 调整为 10 s，manifest、代码默认值、`verify_models.py` 与文档同步。

### 10.4 验证

| 场景 | 修复前 | 修复后 |
|------|--------|--------|
| 30 s PTT | 67 次临时识别，丢 25.16 s，松键后 32.9 s，最终约 2 句 | — |
| 25 s PTT（10 s 分段） | — | 10 s + 10 s + 5 s，3 个 Final，丢 0 s，10 s 段最终识别约 2.4 s，总耗时约 25.95 s |
| 12 s PTT | 21 次临时识别，丢 7.7 s，松键后 7.5 s | 7 次临时识别，丢 0 s，松键后约 3.1 s |

测试使用 20 ms 音频块模拟 cpal 回调，测试音频为重复的 `hello_zh.wav`；修复前使用原始文件，修复后使用裁掉长静音的副本，避免 PTT 1.2 s 自动结束干扰长段测试。

### 10.5 仍存在的限制

- 最终识别仍在音频线程同步执行：10 s 段约 2.5–3 s，20 s 段约 5–6 s；段边界会有可感知延迟，但 2048 块缓冲下不再丢音。
- 10 s 段实测 encoder≈1.5 s、decoder（beam 3）≈0.8 s，decoder 没有 KV cache；8 线程相对 4 线程没有明显收益，这是当前 int8 CPU 模型的技术限制。
- 临时结果只覆盖段尾 6 s，长段会以 `…` 开头；最终结果仍覆盖完整段。
- 如果硬件明显慢于测试机，或设备回调块非常小，2048 块缓冲仍可能溢出；彻底消除段边界阻塞需要把 ASR 移到独立工作线程。

## 11. 正式文档修订清单

以下清单把 §1 记录的文档差异与 §10 的长语音修复整理成可执行的正式文档修订。重复项已合并；“不动”表示当前文档已经一致，或该章节由其它条目统一修改，避免同一段被多次覆盖。

### 11.1 音频错误通道与错误计数

**问题描述**

`PROVIDER §3.1` 曾写“音频错误通过独立通道交给工作线程”，但 `CORE §4.8` 的采集接口只有音频数据通道。实际实现是把零长度 `AudioChunk` 作为错误标记写入同一有界通道，工作线程连续收到 3 次后以 `Fatal` 退出。

**解决方法**

- 错误回调写入零长度 `AudioChunk` 哨兵。
- 工作线程维护 `audio_error_streak`，连续 3 次触发 `PipelineEvent::Fatal`。
- 回调仍只用 `try_send`，不阻塞音频线程；通道满时错误标记可能被丢弃，连续错误计数存在低估可能。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| PROVIDER | §3.1 注 | 不动 | 已写明错误以零长度标记写入同一有界通道；不要改回“独立错误通道”。 |
| CORE | §4.8 注 | 增 | 增加：错误回调写零长度 `AudioChunk`；工作线程连续 3 次触发 `Fatal`；通道满时标记可能丢失，计数可能低估。 |
| PROVIDER | §4.6 `StreamLost` 行 | 不动 | 已有“连续失败 3 次后退出”，与实现一致。 |

### 11.2 manifest 可选字段与分段上限

**问题描述**

对应 §1 #2、#13、#14。`PROVIDER §4.2` 曾声称 fbank 参数、帧移和分段上限由 manifest 声明，但示例没有这些字段；`CORE §4.6` 又把分段上限说成常量；`USER §2` 则写成“由 VAD 模型包声明”。实际实现使用 manifest 的 `fbank`、`max_segment_ms`、`punc.max_tokens`，旧包缺省值分别为 80/25/10、10000、400。

**解决方法**

- manifest 新增可选字段：`fbank { num_bins, frame_ms, shift_ms }`、`max_segment_ms`、`punc.max_tokens`。
- 旧 manifest 不写时使用默认值 80/25/10、10000、400。
- `max_segment_ms` 校验范围 1..=60000；运行期以 manifest 为准，`MAX_SEGMENT_MS` 只是推荐默认值。
- 当前默认分段上限已从 20 s 调整为 10 s；`verify_models.py` 写 manifest 时也使用 10000。
- PTT 在达到上限后继续收集的行为由条目 11.11 统一修改，本条不重复改事件结构。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| PROVIDER | §4.2 JSON 与注 | 改 | 保留现有字段；在注中补“三个字段可选，缺省 80/25/10、10000、400；`max_segment_ms` 范围 1..=60000；运行期以 manifest 为准”。 |
| CORE | §4.4 注 | 增 | 补 `FbankManifest` 缺省 80/25/10、`ModelManifest.max_segment_ms` 缺省 10_000、`PuncArtifact.max_tokens` 缺省 400。 |
| CORE | §4.6 常量 | 不动 | `MAX_SEGMENT_MS` 已标推荐值 10_000；注明运行时取 manifest，不再作为唯一来源。 |
| USER | §2 分段上限 | 不动 | 已改为“由模型包清单声明”，并说明 PTT/continuous 余下语音另起一段。 |
| CORE | §4.11 | 不动 | 事件结构由条目 11.11 统一修改，避免同一代码块被覆盖。 |

### 11.3 CMVN 融合

**问题描述**

对应 §1 #3。模型包目录没有 `cmvn.ark`，但 FireRedVAD / FireRedASR2-AED 实际需要 CMVN，文档没有说明统计量在哪里应用。

**解决方法**

- VAD ONNX 导出时在模型图前融合 `Sub(mean)` / `Mul(istd)`。
- ASR encoder 的导出包装器内部应用 `(features - mean) * istd`。
- 运行期模型包不包含 `cmvn.ark`，只输入 fbank。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| PROVIDER | §4.2 注 | 不动 | 已有“CMVN 在导出时融合、运行期不读取 `cmvn.ark`”。 |
| CORE | §4.10 注 | 增 | 补：VAD 的 CMVN 已融合进 ONNX 图，`FireRedVad` 运行期只处理 fbank。 |
| CORE | §4.13 注 | 增 | 补：ASR encoder 包装器内部应用 CMVN；与条目 11.11 的 `transcribe_with_beam` 注并列追加。 |
| BUILD | §3 | 不动 | 安装脚本负责导出与融合，用户不需要额外处理 `cmvn.ark`。 |

### 11.4 FireRedPunc 导出契约

**问题描述**

对应 §1 #4、#5、#6。官方 FireRedPunc 的 `out_dict` 中 id 0 是 `<space>` 而不是空字符串；模型包只声明 `vocab.txt`，但官方还带 `tokenizer.json`；文档也没有说明 `[CLS]` 对齐、`token_type_ids` 和输出长度。

**解决方法**

- 导出脚本把官方 id 0 映射为 `""`；最终文本从 ASR 原文重建，英文词间空格不依赖 Punc 的 space 标签。
- 运行期从 `vocab.txt` 构建 BERT WordPiece，并挂 `BertNormalizer` + `BertPreTokenizer`；实测与官方 `chinese-lert-base` 词表一致，不需要 `tokenizer.json`。
- 导出包装器内部加 `[CLS]` 并从输出中去掉；运行期输入 `input_ids` / `attention_mask` / `token_type_ids`，输出长度与输入 token 数一致。
- 标点按 tokenizer 的 byte offsets 插入原文本；超长文本按字符切块，仍超限时递归二分。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| PROVIDER | §4.2 注 | 改 | 在现有注中补：官方 id 0 `<space>` 导出映射为 `""`；运行期只需 `vocab.txt` 构建 WordPiece；`[CLS]` 由导出包装器加/去，输出长度等于输入 token 数。与条目 11.5 的 `tokens.txt` 注并列追加。 |
| CORE | §4.16 注 | 增 | 补：WordPiece + `BertNormalizer` / `BertPreTokenizer`；`attention_mask` 全 1、`token_type_ids` 全 0；按 byte offsets 插入标点；超长文本切块后递归二分，不丢尾部。 |
| CORE | §4.15 | 不动 | ASR tokenizer 与 Punc WordPiece 是两个对象；不要混改。 |

### 11.5 ASR 特殊 token 的来源

**问题描述**

对应 §1 #7。文档没有说明 `<sos>` / `<eos>` / `<pad>` 的来源；实际运行时从 `tokens.txt` 查找，官方词表 id 为 `<blank>`=0、`<unk>`=1、`<pad>`=2、`<sos>`=3、`<eos>`=4，找不到时回退 1/2/0。

**解决方法**

- `Tokenizer::load` 按 token 字符串查找特殊 id，不硬编码官方 id。
- `Tokenizer` 保存 `sos_id` / `eos_id` / `pad_id`，并提供 getter 与 `vocab_size`。
- 解码时过滤 `<...>` / `[...]`，处理 SentencePiece `▁`、`<space>` 和 `<0xAB>` byte fallback。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| PROVIDER | §4.2 注 | 增 | 追加：`tokens.txt` 含 `<sos>` / `<eos>` / `<pad>`；运行期按 token 查找，官方 id 为 3/4/2；不要写死。与条目 11.4 的注并列。 |
| CORE | §4.15 | 改 | 结构体补 `sos_id` / `eos_id` / `pad_id`；补 `sos_id()` / `eos_id()` / `pad_id()` / `vocab_size()`；注中写特殊 token 查找与 byte fallback。 |
| CORE | §4.13 | 不动 | `AedConfig` 已包含 `sos_id` / `eos_id` / `pad_id`。 |

### 11.6 external data 与量化模型

**问题描述**

对应 §1 #8。文档没有覆盖 ONNX >2 GiB 的 external data：安装脚本对量化模型优先内联；无法内联时保留分片；manifest 只校验主文件 SHA-256，分片不在哈希范围内。

**解决方法**

- 默认安装流程：导出 FP32 → int8 动态量化 → 尝试内联 → 清理未引用分片 → 校验、写 manifest、原子安装。
- 无法内联时保留分片，分片必须与主 `.onnx` 一起复制。
- `manifest` 只对主文件计算 SHA-256；分片不在校验范围，这是当前格式限制。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| PROVIDER | §4.2 注 | 不动 | 已有 external data 保留说明与“manifest 只声明主文件 SHA-256”。 |
| BUILD | §3 安装方式/量化 | 改 | 补：量化后优先内联成单个 `.onnx`；无法内联时保留分片并随主文件复制，清理不再被引用的旧分片。 |
| CORE | §4.4 注 | 增 | 补：`verify` 只校验主文件 SHA-256；external data 分片不在哈希范围，必须与主文件一起保留。 |

### 11.7 SegmentDropped 与临时行残留

**问题描述**

对应 §1 #9。`CORE §4.5` 曾只说明 `SegmentDropped` 清除 `recording` / `finalizing`，会让临时行文本残留成假结果。

**解决方法**

- `SegmentDropped` 同时清空 `partial`。
- 不新增结果行，不修改剪贴板。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE | §4.5 注 | 不动 | 已写明清除 `recording` / `finalizing` 与临时行文本。 |
| CORE | §5.6 | 不动 | 剪贴板只由 `Final` 写入；`SegmentDropped` 不触发写入，现有描述正确。 |
| USER | §2 空语音段 | 不动 | 与“不产生结果行、剪贴板不变”一致。 |

### 11.8 Pipeline / Tui 的签名所有权

**问题描述**

对应 §1 #10。`CORE` 曾把 `run` 写在 `main.rs`、`app::run` 写成按值接收 Pipeline、`Tui::draw` 写成接收 `&App`；实际入口在 `src/lib.rs::run`，`app::run` 接收 `&mut Pipeline`，`Tui::draw` 接收 `&mut App`。

**解决方法**

- `run` 放在库根，供二进制入口与集成测试共用。
- `app::run(mode, &mut Pipeline, clipboard)`。
- `Tui::draw(&mut App)`，以便把实际滚动偏移和终端高度写回 `App`。
- 主线程负责关闭 Pipeline 再停止采集。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE | §4.1、§4.5、§4.18、§5.6 | 不动 | 四处的签名与退出顺序已按上述实现更新；不要改回按值接收或 `&App`。 |

### 11.9 依赖名称与 Python 版本范围

**问题描述**

对应 §1 #11、#12。README Credits 曾把 `kaldi-native-fbank` 写成 `kaldi-fbank`，且漏列 `tokenizers`；CORE SBOM 曾把 Python 依赖写成“固定版本”，实际使用版本范围以兼容 Python 3.10+。

**解决方法**

- README Credits 与 CORE SBOM 统一使用 `kaldi-native-fbank`，补列 `tokenizers`。
- 安装期 Python 依赖在 `tools/requirements-export.txt` 使用版本范围，不进入运行时 SBOM。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| README | §3 Credits | 不动 | 已修正为 `kaldi-native-fbank` 并补 `tokenizers`。 |
| CORE | §3 SBOM | 不动 | 已使用 `kaldi-native-fbank`、`tokenizers`；注明版本与许可证以 Cargo 清单为准。 |
| CORE | §1 文件树、§3 注 | 不动 | 已写 `requirements-export.txt` 为安装期依赖，使用版本范围。 |

### 11.10 BUILD 安装说明与 README Credits

**问题描述**

对应 §1 #15、#16。BUILD 曾缺少 HuggingFace 来源、缓存、token、磁盘占用和原子替换说明；README Credits 曾写错 crate 名称。

**解决方法**

- BUILD 补默认安装目录、虚拟环境、网络、磁盘、量化、原子替换。
- 补 `HF_TOKEN` 交互：缺失时提示，非交互环境继续未认证下载；token 不落盘。
- README Credits 修正 crate 名称并补齐依赖。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| BUILD | §3 安装模型 | 不动 | 已含默认目录、venv、网络、磁盘、原子替换、量化与 `HF_TOKEN` 注。 |
| BUILD | §1 环境 | 不动 | 已写 Python 3.10+、网络与磁盘要求。 |
| USER | §2 模型安装 | 不动 | 已说明由安装脚本完成下载/转换/校验，TUI 运行期不下载。 |
| README | §3 Credits | 不动 | 与条目 11.9 重叠，保持现有修正即可。 |

### 11.11 长语音识别卡顿与 10 s 分段

**问题描述**

对应 §10。长语音时临时识别被逐帧重复触发，`FinishSegment` 排空音频时也继续触发；同步 ASR 阻塞采集线程，旧 16 块音频通道静默丢块；PTT 达到 20 s 上限后停止收集；VAD 每 10 ms 跑一次 ONNX，临时识别重算整段，beam search 全词表排序。最终表现为 CPU 持续占用、结果迟迟不出、文本缺句。

**解决方法**

- 临时识别只取段尾 6 s，使用 greedy（beam=1）；按“新增音频量 + 上次耗时”动态调度；`FinishSegment` 排空期间抑制临时识别。
- VAD 按 100 ms 批处理；`frame_audio` 保留当前批次剩余音频，段边界重置时不错位。
- 音频通道 16 → 2048；命令优先于音频（`select_biased!`）。
- `SegmenterEvent::SpeechEnd` 增加 `SegmentEndReason::{Silence, MaxDuration}`；PTT 在 `MaxDuration` 后继续收集，只有静音结束或松键才停止。
- beam search 每步只取 top-k，使用 `select_nth_unstable_by`。
- 分段上限默认 20 s → 10 s，manifest、代码默认值、`verify_models.py` 与文档同步。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE | §2 单段串行 | 不动 | 已写明临时/最终识别串行、不重叠、不可取消。 |
| CORE | §4.6 Worker 与常量 | 不动 | 已包含 `next_partial_at` / `next_partial_samples` / `suppress_partials`、批处理与临时识别常量。 |
| CORE | §4.8 注 | 不动 | 容量已写 2048；错误标记细节由条目 11.1 补，不要重复改容量。 |
| CORE | §4.11 | 改 | 把 `SpeechEnd(Segment)` 替换为 `SpeechEnd { segment, reason: SegmentEndReason }`；增加 `SegmentEndReason` 枚举；注中说明 `Silence` / `MaxDuration` 与 PTT 继续收集。 |
| CORE | §4.13 | 增 | 增加 `pub fn transcribe_with_beam(&mut self, features: &[Vec<f32>], beam_size: usize) -> anyhow::Result<AsrOutput>;`；注中说明临时识别 beam=1、最终识别 beam=3。CMVN 注由条目 11.3 负责。 |
| CORE | §4.14 | 增 | 注中补：每步用 top-k 选择，不对 8667 词全排序；decoder 无 KV cache，重跑完整前缀。 |
| CORE | §5.2 | 增 | 在 PTT 注中补：达到 `max_segment_ms` 时保持 `collecting`，余下语音另起一段；只有静音自动结束或松键才回到等待。 |
| CORE | §5.3、§5.4 | 不动 | 已更新命令优先、100 ms VAD 批处理与动态临时识别调度。 |
| USER | §2 临时结果节奏、分段上限 | 不动 | 已写动态间隔、段尾 6 s、`…` 截断与 PTT/continuous 继续分段。 |
| BUILD | §5 测试 | 改 | 把 `tests/segmenter.rs` 的覆盖说明改为“端点、静音自动结束、最大段长（`MaxDuration`）”。 |
| BUILD | §6 发布检查 | 增 | 运行验收补一条：长语音（>10 s）应正常分成多段，PTT 按住期间不丢后续语音。 |
| PROVIDER | §4.2 | 不动 | `max_segment_ms` 已为 10000；字段说明由条目 11.2 统一补，避免重复。 |

### 11.12 落笔顺序

真正需要修改的章节集中在：

- `CORE.html`：§4.4、§4.8、§4.10、§4.11、§4.13、§4.14、§4.15、§4.16
- `PROVIDER.html`：§4.2 注
- `BUILD.html`：§3、§5、§6

其余条目标“不动”，保持与实现一致。没有需要整段删除的内容；旧签名与旧描述通过“改”替换即可。

## 12. 其他建议补入正式文档的内容

以下内容未全部纳入 §11 的具体修订表；与 §11 重叠的条目以 §11 为准，其余仍建议后续补入 PROVIDER / CORE：

1. 实际 VAD / ASR / Punc ONNX 张量名与 VAD cache 形状；
2. ASR encoder 不暴露 `input_lengths`、decoder 无 KV cache 的实现约束；
3. Punc 的英文文本修正规则（大小写、ASCII 标点、独立 `i`）；
4. VAD 后处理与官方 Python 后处理的差异（5 帧平滑、`pad_start_frame`）；
5. Pipeline 的音频 Start/Finish 边界消费与事件顺序；
6. 安装脚本的下载来源、缓存目录与 `HF_TOKEN` 交互细节。
