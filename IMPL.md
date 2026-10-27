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

## 11. 建议后续补入正式文档的内容

以下内容适合在后续维护中继续补入 PROVIDER / CORE（部分字段/结构本次已补，但仍缺少完整说明）：

1. manifest 的 `fbank`、`max_segment_ms`、`punc.max_tokens` 字段语义与默认值；
2. 实际 VAD/ASR/Punc ONNX 张量名、cache 形状、CMVN 融合方式；
3. external data 与主文件 SHA-256 的边界；
4. ASR encoder 不暴露 `input_lengths`、decoder 无 KV cache 的实现约束；
5. Punc 的 CLS 对齐、`vocab.txt`、labels id 映射、英文修正；
6. VAD 后处理/分段阈值与官方 Python 后处理的差异；
7. 安装脚本的下载来源、HF_TOKEN 交互、缓存、磁盘占用、原子安装；
8. Pipeline 的音频 Start/Finish 边界消费、同步临时识别、事件顺序；
9. `SegmentDropped` 对临时行的处理；
10. `Tui::draw` 的可变 App 与滚动状态更新方式。
