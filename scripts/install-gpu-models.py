#!/usr/bin/env python3
"""Install GPU models as schema 5, one-model-per-directory layouts.

By default the models are downloaded from fixed upstream revisions, preferring
a matching ModelScope mirror when one exists and falling back to Hugging Face
otherwise. Files are cached under the platform cache directory and staged before
replacing the target directory. ``--source-dir`` switches to an offline source
and never touches the network. Downloads are resumable and every file is checked
against its pinned SHA-256 before it is installed.
"""

from __future__ import annotations

import argparse
import getpass
import hashlib
import json
import os
import shutil
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path

SCHEMA = 5
MAX_SEGMENT_MS = 30_000
CHUNK_BYTES = 1024 * 1024
PROGRESS_BYTES = 512 * 1024 * 1024

FIRERED_ASR_REVISION = "294ac0154da9c445741810327f1ae6d33a09ecfd"
QWEN_REVISION = "36a678687ba7d07a74ca70ccb0e36902e005fb80"
QWEN_MODELSCOPE_REVISION = "fd6f16ded582f5b0789ef4aea7401707ce49746b"
PUNC_REVISION_MODEL = "db9a00bfcb6a42744052561a776aea73d802efdc"
PUNC_REVISION_OUT_DICT = "a9d6cc5fb506bb3fa9a5939e546b1b2ea7b921b7"
PUNC_REVISION_PUNC_JSON = "7d932cd0cd662f6bba5b47f9b9c4cee080523811"

PROVIDER_INFO = {
    "modelscope": (
        "ModelScope",
        (
            "MODELSCOPE_API_TOKEN",
            "MODELSCOPE_SDK_TOKEN",
            "MODELSCOPE_API_KEY",
            "MODELSCOPE_TOKEN",
            "MODELSCOPE_ACCESS_TOKEN",
        ),
    ),
    "huggingface": (
        "Hugging Face",
        (
            "HF_TOKEN",
            "HUGGING_FACE_HUB_TOKEN",
            "HUGGINGFACEHUB_API_TOKEN",
            "HUGGINGFACE_TOKEN",
        ),
    ),
}
TOKENS: dict[str, str] = {}
TOKEN_DECLINED: set[str] = set()
ALLOW_TOKEN_PROMPT = True

QWEN_LICENSE_NOTICE = """Qwen3-ASR-1.7B
Licensed under the Apache License, Version 2.0.
https://www.apache.org/licenses/LICENSE-2.0
Source: https://huggingface.co/Qwen/Qwen3-ASR-1.7B
"""


@dataclass(frozen=True)
class FileSpec:
    name: str
    size: int
    sha256: str
    urls: tuple[str, ...]


@dataclass(frozen=True)
class ModelSpec:
    id: str
    kind: str
    version: str
    license: str
    backend: str
    format: str
    precision: str
    files: tuple[FileSpec, ...]
    license_notice: str | None = None


def hf_url(repo: str, revision: str, name: str) -> str:
    return f"https://huggingface.co/{repo}/resolve/{revision}/{name}?download=true"


def modelscope_urls(repo: str, revision: str, name: str) -> tuple[str, str]:
    path = f"{repo}/resolve/{revision}/{name}"
    return (
        f"https://modelscope.cn/models/{path}",
        f"https://www.modelscope.cn/models/{path}",
    )


# FireRedASR2-AED uses the pinned Ciwiruk ONNX export. ModelScope has other
# FireRedASR2 ONNX exports, but not this exact file set, so keep the HF source.
FIRERED_MODELS = (
    ModelSpec(
        id="fireredasr2-aed",
        kind="asr",
        version=FIRERED_ASR_REVISION,
        license="Apache-2.0",
        backend="firered",
        format="onnx",
        precision="fp32",
        files=(
            FileSpec(
                "encoder.onnx",
                564_553,
                "4275797da7515ab7d112b684cc1a1589ccf4efdca5c8b97165e3f7b195e46e6e",
                (hf_url("Ciwiruk/FireRedASR2-AED-onnx", FIRERED_ASR_REVISION, "encoder.onnx"),),
            ),
            FileSpec(
                "encoder.data",
                2_893_370_496,
                "d9658f82e025b4fd44b9ad94ef10406a5919bdd006d0b7102e5193ce37ad8e6c",
                (hf_url("Ciwiruk/FireRedASR2-AED-onnx", FIRERED_ASR_REVISION, "encoder.data"),),
            ),
            FileSpec(
                "ctc.onnx",
                44_410_182,
                "ec3f28bd4376cfbded7319e34203b6e9f503a6c12731a15a8f485038fbfb4a75",
                (hf_url("Ciwiruk/FireRedASR2-AED-onnx", FIRERED_ASR_REVISION, "ctc.onnx"),),
            ),
            FileSpec(
                "decoder.onnx",
                392_235,
                "3e426f8fa9e60242d2fa570896eae1539e8859a82d544c50fc5f3b186323bfa0",
                (hf_url("Ciwiruk/FireRedASR2-AED-onnx", FIRERED_ASR_REVISION, "decoder.onnx"),),
            ),
            FileSpec(
                "decoder.data",
                1_793_474_560,
                "c9a46fba63fd90dacf226169a6573b2df34c910178aff4f2a8a4d62d5fc5e6ff",
                (hf_url("Ciwiruk/FireRedASR2-AED-onnx", FIRERED_ASR_REVISION, "decoder.data"),),
            ),
            FileSpec(
                "dict.txt",
                79_172,
                "1bc613de2112d257e61a349c3e72d1b1a9cf19c33d3ca954197ad2171e5ea07b",
                (hf_url("Ciwiruk/FireRedASR2-AED-onnx", FIRERED_ASR_REVISION, "dict.txt"),),
            ),
            FileSpec(
                "train_bpe1000.model",
                251_707,
                "473bbc157cb4eade2059b30a3c877a1c29bd50cadbfbed869ae36eeade7fee07",
                (
                    hf_url(
                        "Ciwiruk/FireRedASR2-AED-onnx",
                        FIRERED_ASR_REVISION,
                        "train_bpe1000.model",
                    ),
                ),
            ),
            FileSpec(
                "cmvn.ark",
                1_311,
                "6efba6105429d1630c05d818d956bfe4edfad37a04b3b27bb5a029b9adb37945",
                (hf_url("Ciwiruk/FireRedASR2-AED-onnx", FIRERED_ASR_REVISION, "cmvn.ark"),),
            ),
            FileSpec(
                "config.json",
                1_203,
                "2a38d390e6bde9807466c765d012daca4d5d66d2019890dffafec95f62310e4f",
                (hf_url("Ciwiruk/FireRedASR2-AED-onnx", FIRERED_ASR_REVISION, "config.json"),),
            ),
        ),
    ),
    ModelSpec(
        id="fireredpunc",
        kind="punc",
        version="2026-03-10",
        license="Apache-2.0",
        backend="firered",
        format="onnx",
        precision="fp32",
        files=(
            FileSpec(
                "model.onnx",
                407_027_716,
                "fbcb4dd1e54f07bf770672a9691fb54af41774a130e878e25d105f2f5ba31fde",
                modelscope_urls("manyeyes/FireRedPunc-zh-en-onnx", PUNC_REVISION_MODEL, "model.onnx"),
            ),
            FileSpec(
                "tokens.txt",
                109_540,
                "45bbac6b341c319adc98a532532882e91a9cefc0329aa57bac9ae761c27b291c",
                modelscope_urls("manyeyes/FireRedPunc-zh-en-onnx", PUNC_REVISION_MODEL, "tokens.txt"),
            ),
            FileSpec(
                "out_dict",
                33,
                "6f0f7e0004881d617bc6e1d7b5b39972da80dcb49576bca489b1603ee55e20bb",
                modelscope_urls("manyeyes/FireRedPunc-zh-en-onnx", PUNC_REVISION_OUT_DICT, "out_dict"),
            ),
            FileSpec(
                "punc.json",
                380,
                "e3cae8dd92ea6cfc52b713238977412e4b7a47aa9b9ae4eaa94156e0984fcb21",
                modelscope_urls("manyeyes/FireRedPunc-zh-en-onnx", PUNC_REVISION_PUNC_JSON, "punc.json"),
            ),
        ),
    ),
)

QWEN_MODELS = (
    ModelSpec(
        id="qwen3-asr-1.7b",
        kind="asr",
        version=QWEN_REVISION,
        license="Apache-2.0",
        backend="qwen3",
        format="gguf",
        precision="bf16",
        files=(
            FileSpec(
                "Qwen3-ASR-1.7B-bf16.gguf",
                4_069_674_944,
                "1af18763dfafde2bbf071ef8a0952f7bee66f140c3565e5ccc5afb07dc1f9227",
                modelscope_urls(
                    "ggml-org/Qwen3-ASR-1.7B-GGUF",
                    QWEN_MODELSCOPE_REVISION,
                    "Qwen3-ASR-1.7B-bf16.gguf",
                )
                + (
                    hf_url(
                        "ggml-org/Qwen3-ASR-1.7B-GGUF",
                        QWEN_REVISION,
                        "Qwen3-ASR-1.7B-bf16.gguf",
                    ),
                ),
            ),
            FileSpec(
                "mmproj-Qwen3-ASR-1.7B-bf16.gguf",
                641_773_984,
                "8882e9ddab3186f9aa71b1417c847177913e1466655ac944cf86e9b846735d62",
                modelscope_urls(
                    "ggml-org/Qwen3-ASR-1.7B-GGUF",
                    QWEN_MODELSCOPE_REVISION,
                    "mmproj-Qwen3-ASR-1.7B-bf16.gguf",
                )
                + (
                    hf_url(
                        "ggml-org/Qwen3-ASR-1.7B-GGUF",
                        QWEN_REVISION,
                        "mmproj-Qwen3-ASR-1.7B-bf16.gguf",
                    ),
                ),
            ),
        ),
        license_notice=QWEN_LICENSE_NOTICE,
    ),
)


def log(message: str) -> None:
    print(f"[gpu-models] {message}", flush=True)


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"[gpu-models] {message}")


def default_model_root() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    else:
        base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(base) / "voime" / "models"


def default_cache_dir() -> Path:
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    else:
        base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "voime"


def human_size(size: int) -> str:
    if size >= 1024**3:
        return f"{size / 1024**3:.2f} GiB"
    if size >= 1024**2:
        return f"{size / 1024**2:.1f} MiB"
    return f"{size} bytes"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(CHUNK_BYTES), b""):
            digest.update(block)
    return digest.hexdigest()


def file_ok(path: Path, spec: FileSpec) -> bool:
    return (
        path.is_file()
        and not path.is_symlink()
        and path.stat().st_size == spec.size
        and sha256_file(path).lower() == spec.sha256.lower()
    )


def manifest_matches(root: Path, spec: ModelSpec, *, check_hash: bool) -> bool:
    if root.is_symlink() or not root.is_dir():
        return False
    manifest_path = root / "manifest.json"
    if manifest_path.is_symlink() or not manifest_path.is_file():
        return False
    try:
        value = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    if not isinstance(value, dict):
        return False
    if (
        value.get("schema") != SCHEMA
        or value.get("id") != spec.id
        or value.get("kind") != spec.kind
    ):
        return False
    runtime = value.get("runtime")
    if not isinstance(runtime, dict):
        return False
    if (
        runtime.get("backend") != spec.backend
        or runtime.get("format") != spec.format
        or runtime.get("precision") != spec.precision
        or runtime.get("max_segment_ms") != MAX_SEGMENT_MS
    ):
        return False
    entries = value.get("files")
    if not isinstance(entries, list):
        return False
    by_path = {}
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("path"), str):
            return False
        by_path[entry["path"]] = entry
    for file_spec in spec.files:
        entry = by_path.get(file_spec.name)
        if entry is None:
            return False
        if entry.get("size") != file_spec.size:
            return False
        if str(entry.get("sha256", "")).lower() != file_spec.sha256.lower():
            return False
        path = root / file_spec.name
        if path.is_symlink() or not path.is_file():
            return False
        if path.stat().st_size != file_spec.size:
            return False
        if check_hash and sha256_file(path).lower() != file_spec.sha256.lower():
            return False
    expected = {file_spec.name for file_spec in spec.files}
    for relative, entry in by_path.items():
        if relative in expected:
            continue
        relative_path = Path(relative)
        if (
            relative_path.is_absolute()
            or not relative_path.parts
            or any(part == ".." for part in relative_path.parts)
        ):
            return False
        path = root / relative_path
        if path.is_symlink() or not path.is_file():
            return False
        size = entry.get("size")
        digest = entry.get("sha256")
        if not isinstance(size, int) or path.stat().st_size != size:
            return False
        if not isinstance(digest, str) or len(digest) != 64:
            return False
        if check_hash and sha256_file(path).lower() != digest.lower():
            return False
    return True


def provider_for_url(url: str) -> str:
    host = (urllib.parse.urlsplit(url).hostname or "").lower()
    if host.endswith("modelscope.cn") or host.endswith("modelscope.ai"):
        return "modelscope"
    if host.endswith("huggingface.co") or host.endswith("hf.co"):
        return "huggingface"
    return ""


def normalize_token(value: str) -> str:
    token = value.strip()
    if token.lower().startswith("bearer "):
        token = token[7:].strip()
    return token


def token_from_env(provider: str) -> str | None:
    for name in PROVIDER_INFO[provider][1]:
        value = os.environ.get(name)
        if value and value.strip():
            return normalize_token(value)
    return None


def token_for(provider: str) -> str | None:
    if provider in TOKENS:
        return TOKENS[provider]
    token = token_from_env(provider)
    if token:
        TOKENS[provider] = token
    return token


def prompt_token(provider: str, *, retry: bool = False) -> str | None:
    if (
        provider in TOKEN_DECLINED
        or not ALLOW_TOKEN_PROMPT
        or not sys.stdin.isatty()
    ):
        return None
    label, names = PROVIDER_INFO[provider]
    reason = "authentication required" if retry else "optional"
    prompt = f"{label} token ({reason}; Enter to skip, or set {names[0]}): "
    try:
        value = normalize_token(getpass.getpass(prompt))
    except (EOFError, KeyboardInterrupt):
        value = ""
    if value:
        TOKENS[provider] = value
        return value
    if retry:
        TOKEN_DECLINED.add(provider)
    return None


def prompt_missing_tokens(models: tuple[ModelSpec, ...]) -> None:
    providers = set()
    for spec in models:
        for file_spec in spec.files:
            for url in file_spec.urls:
                provider = provider_for_url(url)
                if provider:
                    providers.add(provider)
    for provider in PROVIDER_INFO:
        if provider in providers and token_for(provider) is None:
            prompt_token(provider)


def fetch_with_token_retry(
    url: str,
    part: Path,
    expected_size: int,
    label: str,
) -> None:
    try:
        fetch(url, part, expected_size, label)
    except urllib.error.HTTPError as error:
        if error.code not in (401, 403):
            raise
        provider = provider_for_url(url)
        if not provider or token_for(provider) is not None:
            raise
        token = prompt_token(provider, retry=True)
        if not token:
            names = " / ".join(PROVIDER_INFO[provider][1])
            raise RuntimeError(
                f"{PROVIDER_INFO[provider][0]} download requires authentication; "
                f"set {names} or run interactively to enter a token"
            ) from error
        fetch(url, part, expected_size, label)


def fetch(url: str, part: Path, expected_size: int, label: str) -> None:
    offset = part.stat().st_size if part.is_file() else 0
    if offset > expected_size:
        part.unlink(missing_ok=True)
        offset = 0
    if offset == expected_size:
        return
    headers = {"User-Agent": "voime-gpu-models/1"}
    request = urllib.request.Request(url, headers=headers)
    provider = provider_for_url(url)
    token = token_for(provider) if provider else None
    if token:
        # Do not forward the credential to a cross-host CDN redirect.
        request.add_unredirected_header("Authorization", f"Bearer {token}")
    if offset:
        request.add_header("Range", f"bytes={offset}-")
    with urllib.request.urlopen(request, timeout=120) as response:
        status = getattr(response, "status", 200)
        if offset and status == 206:
            mode = "ab"
        else:
            mode = "wb"
            offset = 0
        current = offset
        next_report = offset + PROGRESS_BYTES
        with part.open(mode) as handle:
            while True:
                block = response.read(CHUNK_BYTES)
                if not block:
                    break
                handle.write(block)
                current += len(block)
                if current >= next_report and current < expected_size:
                    log(f"  {label}: {human_size(current)} / {human_size(expected_size)}")
                    next_report = current + PROGRESS_BYTES


def download_file(spec: FileSpec, destination: Path, allow_download: bool) -> Path:
    if destination.is_file() and file_ok(destination, spec):
        log(f"cache hit {destination}")
        return destination
    if not allow_download:
        fail(f"missing cached file and downloads are disabled: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    part = destination.with_name(destination.name + ".part")
    if part.is_file() and file_ok(part, spec):
        part.replace(destination)
        log(f"cache hit {destination}")
        return destination
    if part.is_file() and part.stat().st_size == spec.size:
        part.unlink()
    if destination.exists():
        destination.unlink()
    last_error: Exception | str | None = None
    for url in spec.urls:
        try:
            log(f"download {spec.name} ({human_size(spec.size)})")
            fetch_with_token_retry(url, part, spec.size, spec.name)
            if file_ok(part, spec):
                part.replace(destination)
                log(f"downloaded {destination}")
                return destination
            last_error = f"checksum mismatch for {spec.name}"
            log(last_error)
            part.unlink(missing_ok=True)
        except Exception as error:  # try the next mirror; keep the partial file
            last_error = error
            log(f"download failed: {error}")
    fail(f"failed to download {spec.name}: {last_error}")


def write_manifest(stage: Path, spec: ModelSpec) -> None:
    manifest = {
        "schema": SCHEMA,
        "id": spec.id,
        "kind": spec.kind,
        "version": spec.version,
        "license": spec.license,
        "files": [
            {
                "path": file_spec.name,
                "sha256": file_spec.sha256,
                "size": file_spec.size,
            }
            for file_spec in spec.files
        ],
        "runtime": {
            "backend": spec.backend,
            "format": spec.format,
            "precision": spec.precision,
            "max_segment_ms": MAX_SEGMENT_MS,
        },
    }
    (stage / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def stage_from_cache(spec: ModelSpec, cache_dir: Path, model_root: Path) -> Path:
    stage = Path(tempfile.mkdtemp(prefix=f".{spec.id}-", dir=model_root))
    try:
        for file_spec in spec.files:
            source = cache_dir / spec.id / file_spec.name
            if not file_ok(source, file_spec):
                fail(f"cached model file is missing or invalid: {source}")
            shutil.copy2(source, stage / file_spec.name)
        if spec.license_notice is not None:
            (stage / "LICENSE").write_text(spec.license_notice, encoding="utf-8")
        write_manifest(stage, spec)
        if not manifest_matches(stage, spec, check_hash=True):
            fail(f"staged model failed verification: {stage}")
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise
    return stage


def stage_from_source(source: Path, spec: ModelSpec, model_root: Path) -> Path:
    if source.is_symlink() or not source.is_dir():
        fail(f"offline model directory not found: {source}")
    if not manifest_matches(source, spec, check_hash=True):
        fail(
            "offline model is incomplete or does not match the pinned manifest: "
            f"{source}"
        )
    stage = Path(tempfile.mkdtemp(prefix=f".{spec.id}-", dir=model_root))
    try:
        shutil.copytree(source, stage, dirs_exist_ok=True)
        if spec.license_notice is not None and not (stage / "LICENSE").is_file():
            (stage / "LICENSE").write_text(spec.license_notice, encoding="utf-8")
        if not manifest_matches(stage, spec, check_hash=False):
            fail(f"copied model failed verification: {stage}")
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise
    return stage


def remove_path(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink(missing_ok=True)


def replace_target(stage: Path, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    backup = target.with_name(f".{target.name}.old")
    if backup.exists():
        remove_path(backup)
    had_target = target.exists()
    if had_target:
        target.rename(backup)
    try:
        stage.rename(target)
    except Exception:
        if had_target and backup.exists() and not target.exists():
            backup.rename(target)
        raise
    if backup.exists():
        remove_path(backup)


def install_model(
    spec: ModelSpec,
    model_root: Path,
    cache_dir: Path,
    source_dir: Path | None,
    allow_download: bool,
    force: bool,
) -> None:
    target = model_root / spec.id
    if not force and manifest_matches(target, spec, check_hash=False):
        log(f"already installed {target}")
        return
    model_root.mkdir(parents=True, exist_ok=True)
    if source_dir is not None:
        stage = stage_from_source(source_dir / spec.id, spec, model_root)
    else:
        for file_spec in spec.files:
            download_file(file_spec, cache_dir / spec.id / file_spec.name, allow_download)
        stage = stage_from_cache(spec, cache_dir, model_root)
    try:
        replace_target(stage, target)
    except Exception:
        remove_path(stage)
        raise
    log(f"installed -> {target}")


def pending_downloads(
    models: tuple[ModelSpec, ...],
    cache_dir: Path,
) -> list[tuple[ModelSpec, int]]:
    pending = []
    for spec in models:
        remaining = 0
        for file_spec in spec.files:
            cached = cache_dir / spec.id / file_spec.name
            if file_ok(cached, file_spec):
                continue
            part = cached.with_name(cached.name + ".part")
            if part.is_file() and part.stat().st_size <= file_spec.size:
                remaining += file_spec.size - part.stat().st_size
            else:
                remaining += file_spec.size
        if remaining:
            pending.append((spec, remaining))
    return pending


def confirm_downloads(
    pending: list[tuple[ModelSpec, int]],
    assume_yes: bool,
) -> None:
    if not pending:
        return
    log("The following GPU model files will be downloaded from the network:")
    total = 0
    for spec, size in pending:
        total += size
        log(f"  {spec.id}: {human_size(size)}")
    log(f"Total download: {human_size(total)}")
    if assume_yes:
        return
    if not sys.stdin.isatty():
        fail(
            "model download needs confirmation; rerun with --yes, "
            "or use --source-dir / --no-models"
        )
    try:
        answer = input("Continue? [y/N] ").strip().lower()
    except EOFError:
        fail("model download cancelled")
    if answer not in {"y", "yes"}:
        fail("model download cancelled")


def main(argv: list[str] | None = None) -> int:
    global ALLOW_TOKEN_PROMPT
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog=(
            "Tokens are read from MODELSCOPE_API_TOKEN / HF_TOKEN (and aliases). "
            "Interactive runs prompt for missing tokens; --yes skips prompts."
        ),
    )
    parser.add_argument("--model-root", type=Path, default=default_model_root())
    parser.add_argument("--cache-dir", type=Path, default=default_cache_dir())
    parser.add_argument("--source-dir", type=Path)
    parser.add_argument("--firered", action="store_true", help="install FireRed models")
    parser.add_argument("--qwen3", action="store_true", help="install the Qwen3 model")
    parser.add_argument("--no-download", action="store_true")
    parser.add_argument(
        "--yes",
        "-y",
        action="store_true",
        help="skip download confirmation and token prompts",
    )
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args(argv)
    ALLOW_TOKEN_PROMPT = not args.yes

    selected: tuple[ModelSpec, ...]
    if args.firered or args.qwen3:
        selected = tuple(
            (FIRERED_MODELS if args.firered else ())
            + (QWEN_MODELS if args.qwen3 else ())
        )
    else:
        selected = FIRERED_MODELS + QWEN_MODELS

    model_root = args.model_root.expanduser().resolve()
    cache_dir = args.cache_dir.expanduser().resolve()
    source_dir = args.source_dir.expanduser().resolve() if args.source_dir else None
    allow_download = not args.no_download and source_dir is None

    needed = tuple(
        spec
        for spec in selected
        if args.force or not manifest_matches(model_root / spec.id, spec, check_hash=False)
    )
    if source_dir is None and allow_download:
        pending = pending_downloads(needed, cache_dir)
        confirm_downloads(pending, args.yes)
        if pending:
            prompt_missing_tokens(tuple(spec for spec, _ in pending))
    for spec in selected:
        install_model(
            spec,
            model_root,
            cache_dir,
            source_dir,
            allow_download,
            args.force,
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
