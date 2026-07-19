"""Check PyPI wheel availability for Python 3.14 win_amd64."""
import json
import urllib.request

PKGS = [
    "torch", "torchaudio", "sounddevice", "soundfile", "sentencepiece",
    "transformers", "numpy", "kaldiio", "kaldi_native_fbank", "cn2an",
    "librosa", "peft", "textgrid", "huggingface_hub",
]

for pkg in PKGS:
    try:
        with urllib.request.urlopen(
            f"https://pypi.org/pypi/{pkg}/json", timeout=20
        ) as r:
            d = json.load(r)
        v = d["info"]["version"]
        files = d["releases"].get(v, [])
        names = [f["filename"] for f in files]
        cp314 = [n for n in names if "cp314" in n and "win_amd64" in n]
        abi3 = [n for n in names if "abi3" in n and "win_amd64" in n]
        pure = [n for n in names if "py3-none-any" in n or "py2.py3-none-any" in n]
        if cp314:
            status = f"OK cp314: {cp314[0]}"
        elif abi3:
            status = f"OK abi3: {abi3[0]}"
        elif pure:
            status = "OK pure-python"
        else:
            win = [n for n in names if "win_amd64" in n]
            status = f"MISSING cp314 (latest win: {win[-1] if win else 'none'})"
        print(f"{pkg:22s} {v:12s} {status}")
    except Exception as e:  # noqa: BLE001
        print(f"{pkg:22s} ERROR {e}")
