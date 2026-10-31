# POC：模型包级切换

本分支 `poc/model-switch` 按 `PROPOSAL-sensevoice.html` 实现了包级切换的第一版：

- 同一个 `voimed` 二进制支持 FireRedASR2-CTC 与 SenseVoice。
- 切换只改 `models/active` 指针并重建 pipeline，不替换二进制、不重启 daemon。
- IPC 协议提升到 2；新增 `ListModels`、`SetModel`、`RestartBackend`。
- Windows 设置窗口与 Fcitx 配置页可选择已安装模型包；录音中拒绝重启。

## 模型包布局

```text
~/.local/share/voime/models/        # Windows: %LOCALAPPDATA%\voime\models
├── active                          # 单行包目录名；legacy 表示旧布局
├── firered-ctc/
│   ├── manifest.json
│   ├── vad/stream-vad.onnx
│   ├── punc/...
│   └── asr/model.int8.onnx + tokens.txt
├── sense-voice-2024-itn/           # use_itn=1 + 内置标点
│   ├── manifest.json
│   ├── vad/stream-vad.onnx
│   └── asr/model.int8.onnx + tokens.txt
└── sense-voice-2024-punc/          # use_itn=0 + FireRedPunc
    ├── manifest.json
    ├── vad/stream-vad.onnx
    ├── punc/...
    └── asr/model.int8.onnx + tokens.txt
```

解析顺序：`--model-dir` 显式覆盖 → `active` → 旧布局（模型根目录直接含
`manifest.json`）。旧安装无需迁移。

## 安装

```bash
# FireRed（沿用原有导出流程）
scripts/install_models.sh --asr fire-red-ctc --model-dir ~/.local/share/voime/models/firered-ctc

# SenseVoice ITN：无需已有 FireRed 包，会自动下载 FireRed VAD
scripts/install_models.sh --asr sense-voice --variant itn --activate

# SenseVoice + FireRedPunc：需要一个带 punc/ 的已有包
scripts/install_models.sh --asr sense-voice --variant punc \
  --source-dir ~/.local/share/voime/models/firered-ctc --activate
```

Windows 对应参数为 `-Asr sense-voice -Variant itn|punc -Activate`。

## 使用

```bash
voime ctl models                 # 列出已安装包、active 与 running
voime ctl use sense-voice-2024-itn
voime ctl restart-backend        # 空闲时重建 pipeline
voime doctor                     # 显示 active、kind、标点策略与 SHA 前缀
```

daemon 未运行时 `use` 只写 `active`；下次启动生效。`--model-dir` 固定模型时
拒绝 `SetModel`，只允许重启当前包。

Windows：设置窗口“模型”下拉 + “重启后端”。Fcitx：配置页“Model”下拉，
保存 / Apply 触发 `SetModel` + `RestartBackend`。录音 / 识别中重启返回
`Busy`，旧 pipeline 不会被静默丢弃。

## SenseVoice 实现

- 80 bin fbank、hamming 窗、25/10 ms、输入乘 32768。
- LFR 7/6：与 sherpa-onnx 1.12.27 一致，只保留完整窗口；这样可复现提案中的
  样例输出（`gold` / `开放`）。短于 7 帧的音频返回空结果。
- 560 维 CMVN：`(x + neg_mean) * inv_stddev`。
- 四输入：`x`、`x_length`、`language`、`text_norm`；从 ONNX metadata 读取
  LFR、CMVN、语言 / ITN id 与 `vocab_size` 并交叉校验。
- CTC greedy 后跳过前 4 个 token（语言 / 情感 / 事件 / 文本规整）。
- 标点策略由 `asr.options.punc` 决定：`builtin` 直接用模型输出；`external`
  交给 FireRedPunc。

## 测试

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# 可选真实模型测试
VOIME_SENSEVOICE_MODEL_DIR=<包目录> \
VOIME_SENSEVOICE_AUDIO_DIR=<wav 目录> \
cargo test --test asr_sensevoice -- --nocapture
```

本机已用 2024-07-17 ITN 包验证 5 条公开样例，输出与 sherpa-onnx 1.12.27
一致（英文、中文、日文、粤语、韩文）。

## 暂未实现

- FireRed AED 后端：清单类型已保留，`AsrBackend` 加载时会返回“尚未启用”，
  不影响 CTC / SenseVoice 包切换与回滚。
- 运行时热切换：按提案要求不做；切换一律通过 `active` + 重启后端。
- 产品文档（README / USER / CORE / PROVIDER / BUILD）尚未全部同步，正式合并
  前应按提案 §11 落到各所有者章节。
