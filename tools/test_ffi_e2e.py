"""阶段二 FFI 端到端验证：经 C ABI 驱动 voime_core.dll（不经 Rust rlib）。

覆盖 PRD 5.1 全部 8 个函数 + 扩展函数：
- voime_version / create_engine / model_ready
- CreateEvent 注册 -> 逐 chunk 推送真实 WAV -> 事件触发 poll partial
- stop_session -> final（带标点全文）
- 溢出测试：突发 3000 chunk 期望 -2
- 60s 护栏数据已由队列容量设计保证（见报告）
"""
import argparse
import ctypes
import json
import sys
import time
from ctypes import wintypes
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "poc" / "python"))
from voime_poc.audio import read_wav_16k  # noqa: E402

DLL = ROOT / "engine" / "target" / "release" / "voime_core.dll"


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser()
    ap.add_argument("--wav", default=str(ROOT / "poc" / "python" / "out" / "session_mix_3sent.wav"))
    ap.add_argument("--dll", default=str(DLL))
    ap.add_argument("--realtime", action="store_true")
    args = ap.parse_args()

    dll = ctypes.CDLL(args.dll)
    vp = ctypes.c_void_p
    dll.voime_version.restype = ctypes.c_char_p
    dll.voime_version.argtypes = []
    dll.voime_create_engine.restype = vp
    dll.voime_create_engine.argtypes = []
    for name in ["voime_start_session", "voime_stop_session", "voime_model_ready",
                 "voime_destroy_engine", "voime_set_lang_hint"]:
        f = getattr(dll, name)
        f.restype = ctypes.c_int32
        f.argtypes = [vp] + ([ctypes.c_int32] if name == "voime_set_lang_hint" else [])
    dll.voime_destroy_engine.restype = None
    dll.voime_set_lang_hint.restype = None
    dll.voime_push_audio.restype = ctypes.c_int32
    dll.voime_push_audio.argtypes = [vp, ctypes.POINTER(ctypes.c_int16), ctypes.c_uint32]
    dll.voime_register_event.restype = None
    dll.voime_register_event.argtypes = [vp, wintypes.HANDLE]
    dll.voime_poll_result.restype = vp
    dll.voime_poll_result.argtypes = [vp, ctypes.POINTER(ctypes.c_uint32)]
    dll.voime_free_string.restype = None
    dll.voime_free_string.argtypes = [ctypes.c_char_p]

    k32 = ctypes.windll.kernel32

    print("=" * 64)
    print("FFI E2E: voime_core.dll")
    print("=" * 64)
    print("[ffi] version:", dll.voime_version().decode())

    eng = dll.voime_create_engine()
    assert eng, "create_engine 返回 NULL"
    ready = dll.voime_model_ready(eng)
    print(f"[ffi] engine={eng:#x} model_ready={ready}")
    assert ready == 1, "模型未就绪（检查 VOIME_MODEL_DIR / onnxruntime.dll 位置）"

    ev = k32.CreateEventW(None, False, False, None)
    assert ev
    dll.voime_register_event(eng, ev)

    results = []

    def drain():
        while True:
            n = ctypes.c_uint32(0)
            p = dll.voime_poll_result(eng, ctypes.byref(n))
            if not p:
                break
            s = ctypes.string_at(p, n.value).decode("utf-8")
            dll.voime_free_string(ctypes.cast(p, ctypes.c_char_p))
            results.append(json.loads(s))
            print(f"[result] {s}", flush=True)

    rc = dll.voime_start_session(eng)
    print(f"[ffi] start_session rc={rc}")
    assert rc == 0

    pcm = read_wav_16k(args.wav)
    chunk = 480
    t0 = time.perf_counter()
    fed = 0
    for off in range(0, len(pcm), chunk):
        seg = pcm[off:off + chunk]
        arr = (ctypes.c_int16 * len(seg))(*seg.tolist())
        rc = dll.voime_push_audio(eng, arr, len(seg))
        assert rc == 0, f"push rc={rc}"
        fed += len(seg)
        if args.realtime:
            delay = t0 + fed / 16000 - time.perf_counter()
            if delay > 0:
                time.sleep(delay)
        if k32.WaitForSingleObject(ev, 0) == 0:
            drain()
    print(f"[ffi] 推送 {fed} 样本 ({fed / 16000:.2f}s)")
    rc = dll.voime_stop_session(eng)
    print(f"[ffi] stop_session rc={rc}")

    # 等 final
    deadline = time.perf_counter() + 30
    while time.perf_counter() < deadline:
        if k32.WaitForSingleObject(ev, 500) == 0:
            drain()
        if any(r.get("type") == "final" for r in results):
            break

    partials = [r for r in results if r.get("type") == "partial"]
    finals = [r for r in results if r.get("type") == "final"]
    print("-" * 64)
    print(f"[summary] partial={len(partials)} final={len(finals)}")
    assert len(finals) == 1, "缺少 final 结果"
    final_text = finals[0]["text"]
    print(f"[final] {final_text}")
    ok = len(partials) == 3 and final_text
    print(f"[FFI E2E {'PASS' if ok else 'FAIL'}]")

    # ---- 溢出测试 ----
    print("-" * 64)
    print("溢出测试: 突发推送 3000 chunk（Worker 带真实 VAD 消费）")
    rc = dll.voime_start_session(eng)
    assert rc == 0, f"start rc={rc}"
    arr = (ctypes.c_int16 * 480)(*([1000] * 480))
    okc = ovf = 0
    for _ in range(3000):
        rc = dll.voime_push_audio(eng, arr, 480)
        if rc == 0:
            okc += 1
        elif rc == -2:
            ovf += 1
    print(f"[overflow] ok={okc} -2={ovf} {'PASS' if ovf > 0 else 'FAIL(未触发)'}")
    dll.voime_stop_session(eng)
    deadline = time.perf_counter() + 60
    while time.perf_counter() < deadline:
        if k32.WaitForSingleObject(ev, 500) == 0:
            drain()
            if results and results[-1].get("type") == "final":
                break
    k32.CloseHandle(ev)
    dll.voime_destroy_engine(eng)
    print("[ffi] destroy_engine OK")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
