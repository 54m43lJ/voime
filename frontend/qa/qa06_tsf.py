# qa06_tsf.py — 阶段五 P5-01/P5-04/P5-05 TSF 注册与端到端注入验收
#
# 流程：regsvr32 注册 voime_tsf.dll -> 校验注册表 -> 启动标准版 Agent（WAV 音源）
# -> 启动 tsf_harness.exe（激活 Voime TIP 的 EDIT 宿主）-> SendInput 物理按键
# Ctrl+Space 按住/松开 -> TIP 按键拦截 -> IPC -> Agent 识别 -> commit_text
# -> TSF 注入 harness EDIT。断言：注册表项、TIP 日志、注入文本、Agent 指令序列。
# 前置：模型已就位（qa03）。测试结束后注销 TIP（regsvr32 /u）。
import ctypes, os, subprocess, sys, time, winreg
from ctypes import wintypes
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from qa_driver import (APP_STD, TSF_LOG, WAV_ZH, launch_app, log_size, ptt_hold,
                       pump, report, user32, kernel32, wait_log)

# 显式签名（防 64 位句柄截断）
user32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.FindWindowW.restype = wintypes.HWND
user32.FindWindowExW.argtypes = [wintypes.HWND, wintypes.HWND, wintypes.LPCWSTR,
                                 wintypes.LPCWSTR]
user32.FindWindowExW.restype = wintypes.HWND
user32.GetForegroundWindow.restype = wintypes.HWND
user32.AttachThreadInput.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.BOOL]
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND,
                                            ctypes.POINTER(wintypes.DWORD)]
user32.GetWindowThreadProcessId.restype = wintypes.DWORD

BUILD = r"D:\Work_win\voime\frontend\build"
TSF_DLL_BUILD = os.path.join(BUILD, "voime_tsf.dll")
HARNESS = os.path.join(BUILD, "tsf_harness.exe")
# 注册用沙盒副本：常驻进程（explorer 等）激活 TIP 后会锁定 DLL 文件，
# 注册路径指向沙盒可避免仓库 build 目录被锁导致构建失败
TSF_DLL = os.path.expandvars(r"%TEMP%\voime_qa_sandbox\voime_tsf.dll")
CLSID = "{C7E4B2A1-5F63-4D89-9B0E-3A2D8C1F6E54}"
EXPECT = "你好世界，你好世界，你好世界。"

def regsvr(args):
    """提权运行 regsvr32（RegisterProfile 写 HKLM 必须管理员，PRD P5-07）。"""
    argline = ",".join(["'/s'"] + [f"'{a}'" for a in args] + [f"'{TSF_DLL}'"])
    r = subprocess.run(
        ["powershell", "-Command",
         f"$p = Start-Process regsvr32 -Verb RunAs -ArgumentList {argline} "
         "-Wait -PassThru; exit $p.ExitCode"],
        capture_output=True, timeout=60)
    return r.returncode

def reg_exists(path, hive=winreg.HKEY_CURRENT_USER):
    try:
        winreg.OpenKey(hive, path)
        return True
    except OSError:
        return False

def foreground(hwnd):
    """跨进程置前（AttachThreadInput 绕开前景锁）。"""
    cur = user32.GetForegroundWindow()
    tid_cur = user32.GetWindowThreadProcessId(cur, None)
    tid_me = kernel32.GetCurrentThreadId()
    user32.AttachThreadInput(tid_me, tid_cur, True)
    user32.ShowWindow(hwnd, 9)  # SW_RESTORE
    user32.SetForegroundWindow(hwnd)
    user32.AttachThreadInput(tid_me, tid_cur, False)

# ---------------- 配置热键（与 TIP 同源：config.toml，qa_driver 统一实现） ----------------
from qa_driver import key as _key
from qa_driver import load_ptt_combo

def ptt_combo_hold(mods, k, seconds):
    """按住 (mods+k) seconds 秒后松开（SendInput 真实按键链路）。"""
    for m in mods:
        _key(m, True)
        time.sleep(0.06)
    _key(k, True)
    time.sleep(seconds)
    _key(k, False)
    time.sleep(0.06)
    for m in reversed(mods):
        _key(m, False)

def main():
    state = {"app": None, "harness": None, "registered": False, "hwnd": None}
    try:
        fails = run(state)
    finally:
        if state["app"]:
            state["app"].kill()
        # harness 先 WM_CLOSE 让其优雅退出（DeactivateProfile 防止 TIP 残留激活）
        if state["hwnd"]:
            user32.PostMessageW(state["hwnd"], 0x0010, 0, 0)  # WM_CLOSE
            time.sleep(1.5)
        if state["harness"]:
            harness_rc = state["harness"].poll()
            if harness_rc is None:
                state["harness"].kill()
        # 默认保留注册（反复注册/注销会使 ctfmon profile 缓存错乱，实测结论）；
        # 需要清理时手动执行 frontend\tsf\unregister_tsf.ps1
        if state["registered"] and os.environ.get("QA_TSF_UNREGISTER") == "1":
            regsvr(["/u"])
        # 还原用户 hotkey 配置
        if state.get("cfg_bak") and state.get("cfg_path"):
            open(state["cfg_path"], "wb").write(state["cfg_bak"])
    print(f"[qa06] {'PASS' if fails == 0 else 'FAIL'}", flush=True)
    return 1 if fails else 0

def run(state):
    fails = 0

    # --- 测试 PTT 改写：config.toml 的 Ctrl+Alt+T 疑似被系统全局热键占用
    #     （实测 Ctrl/Alt 到达 TSF 按键槽但 'T' 被吞）。qa06 临时改写为 F13，
    #     结束后还原。 ---
    cfg_path = os.path.expandvars(r"%APPDATA%\VoimeInput\config.toml")
    cfg_bak = None
    if os.path.exists(cfg_path):
        cfg_bak = open(cfg_path, "rb").read()
        import re as _re
        text = cfg_bak.decode("utf-8", errors="replace")
        if _re.search(r'hotkey\s*=\s*"[^"]*"', text):
            text = _re.sub(r'hotkey\s*=\s*"[^"]*"', 'hotkey = "F13"', text)
        else:
            text += '\nhotkey = "F13"\n'
        open(cfg_path, "wb").write(text.encode("utf-8"))
        state["cfg_bak"] = cfg_bak
        state["cfg_path"] = cfg_path

    # --- P5-01：注册（沙盒副本；唯一文件名规避常驻进程文件锁） ---
    import shutil
    global TSF_DLL
    TSF_DLL = os.path.expandvars(
        rf"%TEMP%\voime_qa_sandbox\voime_tsf_{os.getpid()}.dll")
    os.makedirs(os.path.dirname(TSF_DLL), exist_ok=True)
    shutil.copy2(TSF_DLL_BUILD, TSF_DLL)
    rc = regsvr([])
    state["registered"] = rc == 0
    fails += 0 if report("qa06.regsvr32", rc == 0, f"regsvr32 rc={rc}") else 1
    fails += 0 if report("qa06.reg_com",
                         reg_exists(rf"Software\Classes\CLSID\{CLSID}\InprocServer32"),
                         "HKCU CLSID\\InprocServer32 已写入") else 1
    fails += 0 if report("qa06.reg_tip",
                         reg_exists(rf"SOFTWARE\Microsoft\CTF\TIP\{CLSID}",
                                    winreg.HKEY_LOCAL_MACHINE),
                         "HKLM CTF\\TIP 已写入（Profile 需提权）") else 1
    # ctfmon profile 缓存刷新需要数秒（实测注册后立即激活返回 E_INVALIDARG）
    print("[qa06] 等待 8s 让 ctfmon 刷新 profile 缓存...", flush=True)
    time.sleep(8)

    # --- 启动 Agent（标准版，WAV 音源） ---
    log0 = log_size()
    app = launch_app(["--audio-source", WAV_ZH], exe=APP_STD)
    state["app"] = app
    wait_log("IPC 服务已监听", timeout=30, since=log0)

    # --- 启动 TSF 宿主并激活 TIP ---
    tsf_log0 = os.path.getsize(TSF_LOG) if os.path.exists(TSF_LOG) else 0
    harness = subprocess.Popen([HARNESS, "--seconds", "90"], cwd=BUILD)
    state["harness"] = harness

    # 找 harness 窗口与 EDIT/RichEdit
    hwnd = edit = None
    end = time.time() + 15
    while time.time() < end and not edit:
        hwnd = user32.FindWindowW("VoimeTsfHarnessCls", None)
        if hwnd:
            for cls in ("RICHEDIT50W", "EDIT"):
                edit = user32.FindWindowExW(hwnd, None, cls, None)
                if edit:
                    break
        time.sleep(0.3)
    state["hwnd"] = hwnd
    print(f"[qa06] harness hwnd={hwnd:#x} edit={edit:#x}" if edit
          else "[qa06] harness 窗口未找到", flush=True)
    fails += 0 if report("qa06.harness_wnd", bool(edit), "harness 窗口就绪") else 1

    # TIP 应已在 harness 进程内激活并连上 Agent
    _, line = wait_log("Client Connected", timeout=15, since=log0)
    fails += 0 if report("qa06.tsf_connect", line is not None,
                         "TSF 客户端连上 Agent") else 1

    # --- 激活 Voime（语言栏路径：ActivateProfile API 对 zh/en 行为不稳，
    #     实测 Win+Space 语言栏切换是可靠的真实用户路径） ---
    from qa_driver import key as _kbd
    tsf_log0 = os.path.getsize(TSF_LOG) if os.path.exists(TSF_LOG) else 0
    pid = wintypes.DWORD(0)
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))

    def tip_state():
        """harness 进程 Voime 当前状态：'active' / 'inactive' / None（无记录）。"""
        if not os.path.exists(TSF_LOG):
            return None
        with open(TSF_LOG, "rb") as f:
            f.seek(tsf_log0)
            chunk = f.read().decode("utf-8", errors="replace")
        last = None
        for ln in chunk.splitlines():
            if f"pid={pid.value}" not in ln:
                continue
            if "Deactivate" in ln:
                last = "inactive"
            elif "Activate tid=" in ln:
                last = "active"
        return last

    def winspace_once():
        _kbd(0x5B, True); time.sleep(0.1)
        _kbd(0x20, True); time.sleep(0.1)
        _kbd(0x20, False); time.sleep(0.1)
        _kbd(0x5B, False)

    activated = False
    for _ in range(10):  # 最多 10 步（循环切换总能轮到 Voime）
        if tip_state() == "active":
            activated = True
            break
        foreground(hwnd)
        time.sleep(0.3)
        winspace_once()
        # 每步后等状态稳定（Activate 可能延迟 ~1s；防止按过头把它切走）
        for _ in range(12):
            time.sleep(0.25)
            if tip_state() == "active":
                break
    if tip_state() == "active":
        activated = True
    foreground(hwnd)
    fails += 0 if report("qa06.activate", activated,
                         f"语言栏切换激活 Voime (harness pid={pid.value})") else 1

    # 前景保障：授权 harness 自行 SetForegroundWindow，并轮询确认其在前台
    user32.AllowSetForegroundWindow(pid.value)
    fg_ok = False
    for _ in range(10):
        foreground(hwnd)
        time.sleep(0.5)
        if user32.GetForegroundWindow() == hwnd:
            fg_ok = True
            break
    fails += 0 if report("qa06.foreground", fg_ok, "harness 处于前台") else 1

    # 焦点循环：切走再切回，强制 ctfmon 向 harness 推送 TSF 焦点
    shell = user32.GetShellWindow()
    if shell:
        foreground(shell)
        time.sleep(0.7)
    foreground(hwnd)
    time.sleep(0.8)

    # --- P5-04/P5-05：物理按键全链路 ---
    mods, vk = load_ptt_combo()
    print(f"[qa06] 按住配置 PTT（mods={mods} vk=0x{vk:x}，9.5s WAV 会话）...",
          flush=True)
    out_file = os.path.join(BUILD, "tsf_harness_out.txt")
    if os.path.exists(out_file):
        os.remove(out_file)
    t0 = time.time()
    ptt_combo_hold(mods, vk, 9.5)  # SendInput -> ctfmon -> ITfKeyEventSink
    t_rel = time.time()

    # 等注入文本（主信源：harness 进程内自读 dump 文件，UTF-8）
    text, latency = "", None
    end = time.time() + 25
    buf = ctypes.create_unicode_buffer(4096)
    while time.time() < end:
        if os.path.exists(out_file):
            with open(out_file, encoding="utf-8", errors="replace") as f:
                text = f.read().strip()
            if text:
                latency = time.time() - t_rel
                break
        user32.GetWindowTextW(edit, buf, 4095)
        if buf.value:
            text = buf.value
            latency = time.time() - t_rel
            break
        pump(0.1)
    fails += 0 if report("qa06.inject_text", text == EXPECT,
                         f"注入文本: {text!r}") else 1
    fails += 0 if report("qa06.latency", latency is not None and latency < 0.5,
                         f"松开->上屏 {latency and round(latency * 1000)}ms (目标<500ms)") else 1

    # Agent 指令序列（控制反转证据）
    seq_ok = True
    for marker in ["IPC start_session", "IPC stop_session", "commit_text 已发送"]:
        if wait_log(marker, timeout=1, since=log0)[0] is None:
            seq_ok = False
            print(f"    缺日志事件: {marker}", flush=True)
    fails += 0 if report("qa06.event_seq", seq_ok,
                         "start/stop/commit IPC 指令序列") else 1

    # TIP 侧日志（按键拦截 + 注入）
    tsf_ok = False
    if os.path.exists(TSF_LOG):
        with open(TSF_LOG, "rb") as f:
            f.seek(tsf_log0)
            chunk = f.read().decode("utf-8", errors="replace")
        tsf_ok = "start_session" in chunk and "commit_text inject hr=0x" in chunk
    fails += 0 if report("qa06.tsf_log", tsf_ok,
                         "voime_tsf.log 含按键/注入记录") else 1
    return fails

if __name__ == "__main__":
    sys.exit(main())
