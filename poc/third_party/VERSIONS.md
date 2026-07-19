# 第三方依赖版本锁定

| 来源 | 版本 | 用途 |
|---|---|---|
| github.com/FireRedTeam/FireRedASR2S | 4e7d9aaf4482a47cec1724807026b9b151926eb5 (main, 浅克隆) | VAD/ASR/Punc 官方推理代码 |
| huggingface.co/FireRedTeam/FireRedASR2-AED | 2304afed56eacfee6256dee5937ed22ffa0b64ec | ASR 模型权重 |
| huggingface.co/FireRedTeam/FireRedVAD | 7990aaccc6b7aec1e527743bd30201f2c4a03b8c | VAD/Stream-VAD/AED 权重 |
| huggingface.co/FireRedTeam/FireRedPunc | e448fd967f44182a1c323cc30f5d89f2400c28da | Punc 权重 + chinese-lert-base |

（以上目录均被 .gitignore 排除，按 `tools/download_models.py` 与 README 步骤拉取。）
