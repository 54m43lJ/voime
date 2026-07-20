"""下载阶段二所需 ONNX 资产：
1. VidraAI/FireRedASR2-onnx（CTC int8 单图，~776MB）-> models/FireRedASR2-AED-CTC-ONNX/
2. FireRedVAD 官方 onnx_models（GitHub 仓库内小文件）-> models/FireRedVAD-ONNX/
"""
import shutil
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODELS = ROOT / "models"

VAD_FILES = [
    "cmvn.ark",
    "fireredvad_aed.onnx",
    "fireredvad_stream_vad.onnx",
    "fireredvad_stream_vad_with_cache.onnx",
    "fireredvad_vad.onnx",
]
VAD_BASE = "https://raw.githubusercontent.com/FireRedTeam/FireRedVAD/main/pretrained_models/onnx_models"


def main() -> int:
    from huggingface_hub import snapshot_download

    dest = MODELS / "FireRedASR2-AED-CTC-ONNX"
    print(f"[download] VidraAI/FireRedASR2-onnx -> {dest}", flush=True)
    snapshot_download("VidraAI/FireRedASR2-onnx", local_dir=str(dest), max_workers=4)
    print("[download] CTC model done", flush=True)

    vad_dir = MODELS / "FireRedVAD-ONNX"
    vad_dir.mkdir(parents=True, exist_ok=True)
    for name in VAD_FILES:
        target = vad_dir / name
        if target.exists() and target.stat().st_size > 0:
            print(f"[skip] {name}", flush=True)
            continue
        url = f"{VAD_BASE}/{name}"
        print(f"[download] {url}", flush=True)
        with urllib.request.urlopen(url, timeout=120) as r, open(target, "wb") as f:
            shutil.copyfileobj(r, f)
        print(f"[download] {name} -> {target.stat().st_size} bytes", flush=True)

    print("[download] ALL DONE", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
