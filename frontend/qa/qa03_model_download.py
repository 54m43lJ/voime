# qa03_model_download.py — QA-03：模型下载与加载验证
#
# 流程：清空 %APPDATA% 模型 -> 本地 HTTP 服务器模拟 CDN（仓库 models/）->
# 启动应用（--auto-download-models）-> 下载 8 文件(哈希校验) -> 热加载 ->
# PTT 全链路识别验证（证明下载的模型可用）。
import os, shutil, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qa_driver import (APP, LOG, WAV_ZH, edit_text, focus_ok, launch_app, log_size,
                       make_edit_window, pump, ptt_hold, report, wait_log)

MODELS_APPDATA = os.path.expandvars(r"%APPDATA%\VoimeInput\models")
MODELS_REPO = r"D:\Work_win\voime\models"
PORT = 8321

def main():
    fails = 0

    # 1) 清空 %APPDATA% 模型（保留备份以防万一）
    if os.path.isdir(MODELS_APPDATA):
        bak = MODELS_APPDATA + ".qa_bak"
        if os.path.isdir(bak):
            shutil.rmtree(bak)
        shutil.move(MODELS_APPDATA, bak)
    print("[qa03] %APPDATA% 模型已移空", flush=True)

    # 2) 本地 HTTP 服务器模拟 CDN
    srv = subprocess.Popen([sys.executable, "-m", "http.server", str(PORT),
                            "--bind", "127.0.0.1"], cwd=MODELS_REPO,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.5)
    log0 = log_size()

    # 3) 启动应用：自动下载向导（无人值守）
    app = launch_app(["--auto-download-models", "--models-url",
                      f"http://127.0.0.1:{PORT}", "--audio-source", WAV_ZH])
    t0 = time.time()
    _, line = wait_log("模型下载并热加载成功", timeout=300, since=log0)
    dl_secs = time.time() - t0
    fails += 0 if report("qa03.download", line is not None,
                         f"下载+校验+热加载 {dl_secs:.0f}s") else 1

    # 校验 8 个文件就位
    need = [
        r"FireRedVAD-ONNX\fireredvad_stream_vad_with_cache.onnx", r"FireRedVAD-ONNX\cmvn.ark",
        r"FireRedASR2-AED-CTC-ONNX\model.int8.onnx", r"FireRedASR2-AED-CTC-ONNX\tokens.txt",
        r"FireRedASR2-AED-CTC-ONNX\cmvn.ark", r"FireRedPunc-ONNX\punc_bert.onnx",
        r"FireRedPunc-ONNX\tokenizer.json", r"FireRedPunc-ONNX\out_dict",
    ]
    missing = [f for f in need if not os.path.exists(os.path.join(MODELS_APPDATA, f))]
    fails += 0 if report("qa03.files", not missing,
                         f"8 文件就位 (missing={missing})") else 1

    # 4) PTT 全链路验证下载的模型可用（注入到 EDIT 窗口）
    make_edit_window()
    time.sleep(0.5)
    ok_focus = focus_ok()
    t_rel = time.time()
    ptt_hold(9.5)
    text, latency = "", None
    end = time.time() + 15
    while time.time() < end:
        pump(0.1)
        text = edit_text()
        if text:
            latency = time.time() - t_rel - 9.5
            break
    fails += 0 if report("qa03.recognize",
                         ok_focus and "你好世界" in text,
                         f"注入文本: {text!r} 延迟 {latency and round(latency, 2)}s") else 1

    app.kill()
    srv.kill()
    print(f"[qa03] {'PASS' if fails == 0 else 'FAIL'}", flush=True)
    return 1 if fails else 0

if __name__ == "__main__":
    sys.exit(main())
