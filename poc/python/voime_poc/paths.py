"""路径常量与 sys.path 初始化。"""
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
THIRD_PARTY = REPO_ROOT / "poc" / "third_party"
FIREREDASR2S_ROOT = THIRD_PARTY / "FireRedASR2S"

MODELS_DIR = REPO_ROOT / "models"
ASR_MODEL_DIR = MODELS_DIR / "FireRedASR2-AED"
VAD_MODEL_DIR = MODELS_DIR / "FireRedVAD" / "VAD"
VAD_STREAM_MODEL_DIR = MODELS_DIR / "FireRedVAD" / "Stream-VAD"
PUNC_MODEL_DIR = MODELS_DIR / "FireRedPunc"

ASSETS_DIR = FIREREDASR2S_ROOT / "assets"
OUT_DIR = REPO_ROOT / "poc" / "python" / "out"


def setup_sys_path() -> None:
    """把 FireRedASR2S 仓库根加入 sys.path（其包名为 fireredasr2s）。"""
    p = str(FIREREDASR2S_ROOT)
    if p not in sys.path:
        sys.path.insert(0, p)


def ensure_out_dir() -> Path:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    return OUT_DIR
