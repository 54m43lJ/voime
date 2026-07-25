# qa_driver.py — Voime 阶段四 QA 自动化公共驱动
#
# 提供：Win32 EDIT 焦点窗口（注入目标）、键盘 SendInput（触发真实 PTT 钩子）、
# 应用启停、日志读取。qa01/02/03/04 复用。
import ctypes, os, subprocess, sys, time
from ctypes import wintypes

user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32

# 显式签名（64 位参数，避免 ctypes 默认 int 截断溢出）
user32.DefWindowProcW.argtypes = [wintypes.HWND, ctypes.c_uint, ctypes.c_size_t, ctypes.c_longlong]
user32.DefWindowProcW.restype = ctypes.c_longlong
user32.GetWindowTextW.argtypes = [wintypes.HWND, ctypes.c_wchar_p, ctypes.c_int]
user32.GetFocus.restype = wintypes.HWND
user32.SetFocus.argtypes = [wintypes.HWND]
user32.SetFocus.restype = wintypes.HWND
user32.SetForegroundWindow.argtypes = [wintypes.HWND]
user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
user32.CreateWindowExW.argtypes = [wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR,
                                   wintypes.DWORD, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                   ctypes.c_int, wintypes.HWND, wintypes.HMENU,
                                   wintypes.HINSTANCE, wintypes.LPVOID]
user32.CreateWindowExW.restype = wintypes.HWND
user32.RegisterClassExW.argtypes = [ctypes.c_void_p]
user32.PeekMessageW.argtypes = [ctypes.c_void_p, wintypes.HWND, ctypes.c_uint, ctypes.c_uint, ctypes.c_uint]
user32.TranslateMessage.argtypes = [ctypes.c_void_p]
user32.DispatchMessageW.argtypes = [ctypes.c_void_p]
user32.SendInput.argtypes = [ctypes.c_uint, ctypes.c_void_p, ctypes.c_int]

APP = r"D:\Work_win\voime\frontend\build\voime_input.exe"
APP_CONSOLE = r"D:\Work_win\voime\frontend\build\voime_input_console.exe"
LOG = os.path.expandvars(r"%APPDATA%\VoimeInput\voime.log")
WAV_ZH = r"D:\Work_win\voime\poc\python\out\session_zh_3sent.wav"

# ---------------- 键盘 SendInput（INPUT 40B 对齐） ----------------
class KEYBDINPUT(ctypes.Structure):
    _fields_ = [("wVk", ctypes.c_ushort), ("wScan", ctypes.c_ushort),
                ("dwFlags", ctypes.c_ulong), ("time", ctypes.c_ulong),
                ("dwExtraInfo", ctypes.POINTER(ctypes.c_ulong))]

class INPUT_UNION(ctypes.Union):
    _fields_ = [("ki", KEYBDINPUT), ("pad", ctypes.c_byte * 32)]

class INPUT(ctypes.Structure):
    _fields_ = [("type", ctypes.c_ulong), ("u", INPUT_UNION)]

def key(vk, down):
    i = INPUT()
    i.type = 1
    i.u.ki = KEYBDINPUT(vk, 0, 0 if down else 2, 0, None)
    n = user32.SendInput(1, ctypes.byref(i), ctypes.sizeof(i))
    assert n == 1, f"SendInput 失败: {ctypes.get_last_error()}"

def ptt_hold(seconds):
    """按住 Ctrl+Space seconds 秒后松开（经 WH_KEYBOARD_LL 真实链路）。"""
    key(0xA2, True)   # LCONTROL
    time.sleep(0.08)
    key(0x20, True)   # SPACE
    time.sleep(seconds)
    key(0x20, False)
    time.sleep(0.08)
    key(0xA2, False)

# ---------------- Win32 EDIT 焦点窗口（注入目标） ----------------
_WND_PROC = ctypes.WINFUNCTYPE(ctypes.c_longlong, wintypes.HWND, ctypes.c_uint,
                               ctypes.c_size_t, ctypes.c_longlong)
_edit_hwnd = None

class WNDCLASSEXW(ctypes.Structure):
    _fields_ = [("cbSize", ctypes.c_uint), ("style", ctypes.c_uint),
                ("lpfnWndProc", _WND_PROC), ("cbClsExtra", ctypes.c_int),
                ("cbWndExtra", ctypes.c_int), ("hInstance", wintypes.HINSTANCE),
                ("hIcon", wintypes.HICON), ("hCursor", wintypes.HANDLE),
                ("hbrBackground", wintypes.HBRUSH), ("lpszMenuName", wintypes.LPCWSTR),
                ("lpszClassName", wintypes.LPCWSTR), ("hIconSm", wintypes.HICON)]

def _wnd_proc(hwnd, msg, wp, lp):
    return user32.DefWindowProcW(hwnd, msg, wp, lp)

_wnd_proc_ref = _WND_PROC(_wnd_proc)  # 全局引用防 GC（窗口过程指针长存）

def make_edit_window(title="Voime QA Target"):
    """创建置顶 EDIT 窗口并置焦点，返回 (hwnd, edit_hwnd)。"""
    global _edit_hwnd
    cls = "VoimeQaTargetWnd"
    wndclass = WNDCLASSEXW()
    wndclass.cbSize = ctypes.sizeof(WNDCLASSEXW)
    wndclass.lpfnWndProc = _wnd_proc_ref
    wndclass.hInstance = kernel32.GetModuleHandleW(None)
    wndclass.lpszClassName = cls
    wndclass.hbrBackground = 5  # COLOR_WINDOW
    if not user32.RegisterClassExW(ctypes.byref(wndclass)):
        err = ctypes.get_last_error()
        if err != 1410:  # 已注册
            raise OSError(f"RegisterClassExW: {err}")
    hwnd = user32.CreateWindowExW(0, cls, title, 0x00CF0000,  # WS_OVERLAPPEDWINDOW
                                  200, 200, 480, 200, None, None,
                                  kernel32.GetModuleHandleW(None), None)
    edit = user32.CreateWindowExW(0, "EDIT", "", 0x50000004,  # WS_CHILD|WS_VISIBLE|ES_MULTILINE
                                  0, 0, 470, 160, hwnd, 1,
                                  kernel32.GetModuleHandleW(None), None)
    user32.ShowWindow(hwnd, 1)
    user32.SetForegroundWindow(hwnd)
    user32.SetFocus(edit)
    _edit_hwnd = edit
    pump(0.3)
    return hwnd, edit

def pump(seconds):
    """泵消息 seconds 秒。"""
    end = time.time() + seconds
    msg = wintypes.MSG()
    while time.time() < end:
        while user32.PeekMessageW(ctypes.byref(msg), None, 0, 0, 1):
            user32.TranslateMessage(ctypes.byref(msg))
            user32.DispatchMessageW(ctypes.byref(msg))
        time.sleep(0.01)

def edit_text():
    buf = ctypes.create_unicode_buffer(4096)
    user32.GetWindowTextW(_edit_hwnd, buf, 4095)
    return buf.value

def focus_ok():
    return user32.GetFocus() == _edit_hwnd

# ---------------- 应用与日志 ----------------
SANDBOX = os.path.expandvars(r"%TEMP%\voime_qa_sandbox")
APP_DLLS = ["voime_core.dll", "onnxruntime.dll", "onnxruntime_providers_shared.dll",
            "swresample-4.dll", "avutil-58.dll"]

def sandbox_app():
    """把 exe + 运行时 DLL 拷到隔离沙盒目录（避开引擎'向上找仓库 models/'开发回退）。"""
    import shutil
    os.makedirs(SANDBOX, exist_ok=True)
    shutil.copy2(APP, SANDBOX)
    for dll in APP_DLLS:
        shutil.copy2(os.path.join(os.path.dirname(APP), dll), SANDBOX)
    return os.path.join(SANDBOX, os.path.basename(APP))

def launch_app(extra_args, env_extra=None, isolated=True, refresh=True):
    env = dict(os.environ)
    env.pop("VOIME_MODEL_DIR", None)  # 走 %APPDATA% 模型（QA-03 已下载）
    if env_extra:
        env.update(env_extra)
    exe = sandbox_app() if isolated and refresh else (
        os.path.join(SANDBOX, os.path.basename(APP)) if isolated else APP)
    return subprocess.Popen([exe] + extra_args, env=env)

def wait_log(marker, timeout=30, since=0):
    """等日志出现 marker，返回 (行号, 行文本)；超时返回 (None, None)。"""
    end = time.time() + timeout
    while time.time() < end:
        if os.path.exists(LOG):
            with open(LOG, encoding="utf-8", errors="replace") as f:
                lines = f.readlines()
            for i in range(since, len(lines)):
                if marker in lines[i]:
                    return i, lines[i].rstrip()
        time.sleep(0.3)
    return None, None

def log_size():
    return sum(1 for _ in open(LOG, encoding="utf-8", errors="replace")) if os.path.exists(LOG) else 0

def report(name, ok, detail=""):
    print(f"  [{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)
    return ok
