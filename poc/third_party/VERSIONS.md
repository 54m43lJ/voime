# 第三方依赖版本锁定

| 来源 | 版本 | 用途 |
|---|---|---|
| github.com/FireRedTeam/FireRedASR2S | 4e7d9aaf4482a47cec1724807026b9b151926eb5 (main, 浅克隆) | VAD/ASR/Punc 官方推理代码 |
| huggingface.co/FireRedTeam/FireRedASR2-AED | 2304afed56eacfee6256dee5937ed22ffa0b64ec | ASR 模型权重 |
| huggingface.co/FireRedTeam/FireRedVAD | 7990aaccc6b7aec1e527743bd30201f2c4a03b8c | VAD/Stream-VAD/AED 权重 |
| huggingface.co/FireRedTeam/FireRedPunc | e448fd967f44182a1c323cc30f5d89f2400c28da | Punc 权重 + chinese-lert-base |

（以上目录均被 .gitignore 排除，按 `tools/download_models.py` 与 README 步骤拉取。）

## 阶段二 ONNX 资产

| 来源 | 版本/路径 | 用途 |
|---|---|---|
| github.com/FireRedTeam/FireRedVAD `pretrained_models/onnx_models/` | main（2306042B with_cache.onnx） | 官方流式 VAD ONNX |
| huggingface.co/VidraAI/FireRedASR2-onnx | 776861440B model.int8.onnx | ASR encoder+CTC int8 |
| 自研导出 `tools/export_punc_onnx.py` | opset 17, torch 2.11 | Punc BERT ONNX（388MB） |
| microsoft/onnxruntime v1.27.0 | onnxruntime-win-x64-1.27.0.zip | ORT 运行时（load-dynamic） |
