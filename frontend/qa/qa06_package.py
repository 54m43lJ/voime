# qa06_package.py — QA-06：极简绿色版 Zip 验收
#
# 1) 校验 Zip 内容（含运行时依赖、不含模型）
# 2) 解压到全新目录 -> 全新 %APPDATA% 模型目录 -> 首启向导下载(本地服务器) ->
#    PTT 全链路识别注入（M4 端到端验收）
import os, shutil, subprocess, sys, time, zipfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qa_driver import (WAV_ZH, edit_text, focus_ok, log_size, make_edit_window,
                       pump, ptt_hold, report, wait_log)

ZIP = r"D:\Work_win\voime\frontend\dist\voime_input-1.0.0-mvp-win64.zip"
EXTRACT = os.path.expandvars(r"%TEMP%\voime_pkg_test")
MODELS_APPDATA = os.path.expandvars(r"%APPDATA%\VoimeInput\models")
MODELS_REPO = r"D:\Work_win\voime\models"
PORT = 8322
NEED = ["voime_input.exe", "voime_core.dll", "onnxruntime.dll",
        "onnxruntime_providers_shared.dll", "swresample-4.dll", "avutil-58.dll", "README.txt"]

def main():
    fails = 0

    # 1) Zip 内容
    with zipfile.ZipFile(ZIP) as z:
        names = [os.path.basename(n) for n in z.namelist() if not n.endswith("/")]
        total_mb = sum(i.file_size for i in z.infolist()) / 1048576
    missing = [f for f in NEED if f not in names]
    has_model = any(n.lower().endswith(".onnx") or "tokenizer" in n.lower() or
                    "tokens.txt" in n.lower() or "cmvn" in n.lower() for n in names)
    fails += 0 if report("qa06.zip_contents", not missing and not has_model,
                         f"{len(names)} 文件 {total_mb:.0f}MB，无模型") else 1

    # 2) 解压到全新目录
    if os.path.isdir(EXTRACT):
        shutil.rmtree(EXTRACT)
    with zipfile.ZipFile(ZIP) as z:
        z.extractall(EXTRACT)
    exe = None
    for root, _, files in os.walk(EXTRACT):
        if "voime_input.exe" in files:
            exe = os.path.join(root, "voime_input.exe")
    fails += 0 if report("qa06.extract", exe is not None, exe or "") else 1

    # 3) 全新模型目录 + 本地 CDN + 首启下载 + 全链路
    if os.path.isdir(MODELS_APPDATA):
        bak = MODELS_APPDATA + ".qa_bak"
        if os.path.isdir(bak):
            shutil.rmtree(bak)
        shutil.move(MODELS_APPDATA, bak)
    srv = subprocess.Popen([sys.executable, "-m", "http.server", str(PORT),
                            "--bind", "127.0.0.1"], cwd=MODELS_REPO,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.5)
    env = dict(os.environ)
    env.pop("VOIME_MODEL_DIR", None)
    app = subprocess.Popen([exe, "--auto-download-models", "--models-url",
                            f"http://127.0.0.1:{PORT}", "--audio-source", WAV_ZH], env=env)
    log0 = log_size()
    _, line = wait_log("模型下载并热加载成功", timeout=300, since=log0)
    fails += 0 if report("qa06.first_run_wizard", line is not None, "首启向导下载+热加载") else 1

    make_edit_window()
    time.sleep(0.5)
    ok_focus = focus_ok()
    ptt_hold(9.5)
    text = ""
    end = time.time() + 15
    while time.time() < end:
        pump(0.05)
        text = edit_text()
        if text:
            break
    fails += 0 if report("qa06.full_use", ok_focus and "你好世界" in text,
                         f"绿色版全流程: {text!r}") else 1

    app.kill()
    srv.kill()
    print(f"[qa06] {'PASS' if fails == 0 else 'FAIL'}", flush=True)
    return 1 if fails else 0

if __name__ == "__main__":
    sys.exit(main())
