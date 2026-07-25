# qa04_startup_repair.py — QA-04：FFmpeg 依赖修复验证（应用启动弹窗流程）
#
# 人为破坏（移走沙盒内 swresample-4.dll）-> 启动应用弹修复提示 ->
# 自动点击“是”-> WinHTTP 下载修复 -> “修复完成”弹窗点确定 -> 应用退出 ->
# 重启应用正常（自检通过，进入主循环）。
import ctypes, os, sys, time
from ctypes import wintypes
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qa_driver import SANDBOX, launch_app, log_size, report, sandbox_app, wait_log

user32 = ctypes.windll.user32
user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.FindWindowW.restype = wintypes.HWND
user32.FindWindowExW.argtypes = [wintypes.HWND, wintypes.HWND, wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.FindWindowExW.restype = wintypes.HWND
user32.SendMessageW.argtypes = [wintypes.HWND, ctypes.c_uint, ctypes.c_size_t, ctypes.c_longlong]
BM_CLICK = 0x00F5

def click_dialog_button(title, btn_text, timeout=10):
    """在超时内找到标题为 title 的对话框并点击 btn_text 按钮。"""
    end = time.time() + timeout
    while time.time() < end:
        dlg = user32.FindWindowW("#32770", title)
        if dlg:
            btn = user32.FindWindowExW(dlg, None, "Button", btn_text)
            if btn:
                user32.SendMessageW(btn, BM_CLICK, 0, 0)
                return True
        time.sleep(0.4)
    return False

def main():
    fails = 0
    exe = sandbox_app()  # 确保沙盒存在
    swr = os.path.join(SANDBOX, "swresample-4.dll")
    bak = swr + ".qa_bak"
    if os.path.exists(bak):
        os.remove(bak)
    os.rename(swr, bak)  # 人为破坏
    print("[qa04] 已破坏沙盒 swresample-4.dll", flush=True)

    log0 = log_size()
    app = launch_app([], refresh=False)  # 启动（不刷新沙盒，保留破坏）-> 自检失败应弹窗
    clicked = click_dialog_button("Voime 依赖自检", "是(&Y)", timeout=20)
    fails += 0 if report("qa04.prompt", clicked, "启动弹修复提示并点击“是”") else 1

    # 修复中（下载 ~38MB），等“修复完成”弹窗
    done = click_dialog_button("Voime", "确定", timeout=180) if clicked else False
    fails += 0 if report("qa04.repair", done, "下载修复完成弹窗") else 1
    try:
        app.wait(timeout=20)
    except Exception:
        app.kill()
    restored = os.path.exists(swr)
    fails += 0 if report("qa04.restored", restored, "swresample-4.dll 已恢复") else 1
    if not restored and os.path.exists(bak):  # 修复失败则还原，保沙盒可用
        os.rename(bak, swr)

    # 重启：自检通过，正常初始化（无弹窗，进入主循环）
    log1 = log_size()
    app2 = launch_app([], refresh=False)
    _, line = wait_log("FFmpeg 依赖正常", timeout=30, since=log1)
    fails += 0 if report("qa04.relaunch_ok", line is not None, "修复后启动自检通过") else 1
    _, line2 = wait_log("初始化完成", timeout=30, since=log1)
    fails += 0 if report("qa04.relaunch_run", line2 is not None, "修复后应用正常运行") else 1
    app2.kill()
    if os.path.exists(bak):
        os.remove(bak)

    print(f"[qa04] {'PASS' if fails == 0 else 'FAIL'}", flush=True)
    return 1 if fails else 0

if __name__ == "__main__":
    sys.exit(main())
