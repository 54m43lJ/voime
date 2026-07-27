# qa07_uac.py — 阶段五 P5-07 管理员权限（UAC）适配验收（自动化部分）
#
# TSF 解决 UIPI 权限隔离的核心证据链（自动化可覆盖部分）：
#   1) 注册系统级生效（HKLM Profile + SECUREMODE 类别，提权进程可见）；
#   2) 管道 ACL 允许同用户提权客户端（跨 IL）完成全 IPC 会话并收到正确文本
#      —— 这正是 TSF 加载进提权进程后所走的通道；
#   3) TIP 可加载进任意进程（explorer/OpenCode 等已在历次运行中验证加载）。
# 提权窗口内的“语言栏切换 + 物理按键”需真实键盘（SendInput 无法模拟提权
# 上下文中的输入法切换），手工验证步骤见 docs/stage5 报告 P5-07 节。
import os, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qa_driver import (APP_STD, WAV_ZH, launch_app, log_size, report, wait_log)

BUILD = r"D:\Work_win\voime\frontend\build"
PY = r"D:\Work_win\voime\.venv\Scripts\python.exe"
ELEVATED_CLIENT = r"D:\Work_win\voime\frontend\qa\qa07_elevated_client.py"
RESULT = os.path.expandvars(r"%TEMP%\voime_qa_sandbox\qa07_elevated_result.txt")

def main():
    fails = 0
    app = None
    try:
        fails = run()
    finally:
        if app:
            app.kill()
    print(f"[qa07] {'PASS' if fails == 0 else 'FAIL'}", flush=True)
    return 1 if fails else 0

def run():
    fails = 0
    log0 = log_size()

    # 1) 注册系统级可见性（HKLM Profile + SECUREMODE 类别键存在）
    import winreg
    try:
        k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE,
                           r"SOFTWARE\Microsoft\CTF\TIP\{C7E4B2A1-5F63-4D89-9B0E-"
                           r"3A2D8C1F6E54}\Category\Category\{49D2F9CE-1F5E-11D7-"
                           r"A6D3-00065B84435C}")
        securemode = True
        winreg.CloseKey(k)
    except OSError:
        securemode = False
    fails += 0 if report("qa07.securemode_cat", securemode,
                         "HKLM SECUREMODE 类别已注册（提权进程可加载）") else 1

    # 2) Agent（中完整性，WAV 音源）
    global app
    app = launch_app(["--audio-source", WAV_ZH], exe=APP_STD)
    wait_log("IPC 服务已监听", timeout=30, since=log0)

    # 3) 提权 python 客户端跑全 IPC 会话（跨 IL 管道 ACL 验证）
    if os.path.exists(RESULT):
        os.remove(RESULT)
    subprocess.run(
        ["powershell", "-Command",
         f"Start-Process -Verb RunAs -FilePath '{PY}' "
         f"-ArgumentList '{ELEVATED_CLIENT}' -Wait"],
        capture_output=True, timeout=120)
    text = open(RESULT, encoding="utf-8", errors="replace").read() \
        if os.path.exists(RESULT) else ""
    ok = text.startswith("PASS")
    fails += 0 if report("qa07.elevated_fullchain", ok,
                         f"提权客户端全会话: {text[:80]!r}") else 1

    # 4) Agent 指令序列（commit_text 已发给提权客户端）
    seq_ok = True
    for marker in ["IPC start_session", "IPC stop_session", "commit_text 已发送"]:
        if wait_log(marker, timeout=1, since=log0)[0] is None:
            seq_ok = False
    fails += 0 if report("qa07.event_seq", seq_ok, "IPC 指令序列完整") else 1
    return fails

if __name__ == "__main__":
    sys.exit(main())
