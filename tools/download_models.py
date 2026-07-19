"""Download FireRed model weights into <repo>/models/.

Repos (HuggingFace):
  - FireRedTeam/FireRedASR2-AED  (~4.73 GB)
  - FireRedTeam/FireRedVAD       (~7 MB, VAD + Stream-VAD + AED)
  - FireRedTeam/FireRedPunc      (~0.82 GB, skip tensorflow .h5)

Set HF_ENDPOINT=https://hf-mirror.com to use the mirror.
"""
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODELS = ROOT / "models"

REPOS = [
    ("FireRedTeam/FireRedASR2-AED", "FireRedASR2-AED", None),
    ("FireRedTeam/FireRedVAD", "FireRedVAD", None),
    ("FireRedTeam/FireRedPunc", "FireRedPunc", ["*.h5"]),
]


def main() -> int:
    from huggingface_hub import snapshot_download

    MODELS.mkdir(exist_ok=True)
    for repo_id, local_name, ignore in REPOS:
        dest = MODELS / local_name
        print(f"[download] {repo_id} -> {dest}", flush=True)
        path = snapshot_download(
            repo_id,
            local_dir=str(dest),
            ignore_patterns=ignore,
            max_workers=4,
        )
        print(f"[download] done: {path}", flush=True)
    print("[download] ALL DONE", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
