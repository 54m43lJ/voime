"""Check wheels for cp310/cp311/cp312 win_amd64."""
import json
import urllib.request

for pkg in ["kaldi_native_fbank", "torch", "torchaudio", "sentencepiece", "textgrid", "sounddevice"]:
    with urllib.request.urlopen(f"https://pypi.org/pypi/{pkg}/json", timeout=20) as r:
        d = json.load(r)
    v = d["info"]["version"]
    names = [f["filename"] for f in d["releases"].get(v, [])]
    row = [f"{pkg}=={v}"]
    for tag in ("cp310", "cp311", "cp312"):
        hit = [n for n in names if tag in n and "win_amd64" in n]
        row.append(f"{tag}: {'Y' if hit else '-'}")
    pure = [n for n in names if "none-any" in n]
    if pure:
        row.append("pure: Y")
    print(" | ".join(row))
