// tsf_harness.cpp — qa06 用 TSF 宿主测试程序
//
// 创建置顶 RichEdit/EDIT 窗口模拟“目标应用进程”。Voime TIP 的激活由 qa06
// 经 Win+Space（语言栏真实用户路径）驱动；本程序不自激活（ActivateProfile
// API 在本机对 zh 偶发 E_INVALIDARG、对 en 空成功，且半激活态干扰切换）。
// 控件内容每 500ms 落盘 tsf_harness_out.txt 供自动化断言。
#include <windows.h>
#include <richedit.h>

#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <string>

// 仅自测激活用（与 frontend/tsf/src/guids.cpp 一致）
#include <initguid.h>
#include <msctf.h>
static const CLSID kClsidVoime =
    {0xc7e4b2a1, 0x5f63, 0x4d89, {0x9b, 0x0e, 0x3a, 0x2d, 0x8c, 0x1f, 0x6e, 0x54}};
static const GUID kGuidVoimeProfile =
    {0x1f6e54c7, 0x4b2a, 0x4d89, {0xa5, 0xf6, 0x3a, 0x2d, 0x8c, 0x1f, 0x9b, 0x0e}};
static const CLSID kClsidTfProfiles =
    {0x33c53a50, 0xf456, 0x4884, {0xb0, 0x49, 0x85, 0xfd, 0x64, 0x3e, 0xcf, 0xed}};
static const CLSID kClsidTfThreadMgr =
    {0x529a9e6b, 0x6587, 0x4f23, {0xab, 0x9e, 0x9c, 0x7d, 0x68, 0x3e, 0x3c, 0x50}};

static const wchar_t* kCls = L"VoimeTsfHarnessCls";
static HWND g_edit = nullptr;
static FILE* g_log = nullptr;

static void hlog(const char* fmt, ...); // 前置声明（自测函数在定义前使用）

// ---- 提权自测（P5-07/qa07）：本进程（已提权）自驱动按键 ----
static void send_key(WORD vk, bool down) {
    INPUT in{};
    in.type = INPUT_KEYBOARD;
    in.ki.wVk = vk;
    in.ki.dwFlags = down ? 0 : KEYEVENTF_KEYUP;
    SendInput(1, &in, sizeof(INPUT));
}

// 点击本窗口中心：真实输入事件 -> 点击激活前台（前台锁只认真实输入）
static void click_self(HWND hwnd) {
    RECT rc{};
    GetWindowRect(hwnd, &rc);
    int cx = (rc.left + rc.right) / 2, cy = (rc.top + rc.bottom) / 2;
    INPUT in[2]{};
    in[0].type = INPUT_MOUSE;
    in[0].mi.dwFlags = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE;
    in[0].mi.dx = (LONG)(cx * 65535.0 / (GetSystemMetrics(SM_CXSCREEN) - 1));
    in[0].mi.dy = (LONG)(cy * 65535.0 / (GetSystemMetrics(SM_CYSCREEN) - 1));
    in[1].type = INPUT_MOUSE;
    in[1].mi.dwFlags = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_LEFTDOWN | MOUSEEVENTF_LEFTUP;
    in[1].mi.dx = in[0].mi.dx;
    in[1].mi.dy = in[0].mi.dy;
    SendInput(2, in, sizeof(INPUT));
}

static void pump_for(int ms) {
    DWORD end = GetTickCount() + (DWORD)ms;
    MSG msg;
    while ((int)(GetTickCount() - end) < 0) {
        while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Sleep(10);
    }
}

// 读 %APPDATA%\VoimeInput\voime_tsf.log，判断本进程 TIP 当前是否激活
static bool self_tip_active(DWORD pid) {
    wchar_t dir[MAX_PATH];
    DWORD n = GetEnvironmentVariableW(L"APPDATA", dir, MAX_PATH);
    if (!n || n >= MAX_PATH) return false;
    std::wstring path = std::wstring(dir) + L"\\VoimeInput\\voime_tsf.log";
    FILE* f = _wfopen(path.c_str(), L"rb");
    if (!f) return false;
    fseek(f, 0, SEEK_END);
    long len = ftell(f);
    fseek(f, 0, SEEK_SET);
    std::string chunk((size_t)len + 1, 0);
    fread(chunk.data(), 1, len, f);
    fclose(f);
    char pidtag[32];
    snprintf(pidtag, sizeof(pidtag), "pid=%lu", pid);
    bool active = false, seen = false;
    const char* p = chunk.c_str();
    while (const char* eol = strchr(p, '\n')) {
        std::string ln(p, eol - p);
        if (ln.find(pidtag) != std::string::npos) {
            if (ln.find("Deactivate") != std::string::npos) { active = false; seen = true; }
            else if (ln.find("Activate tid=") != std::string::npos) { active = true; seen = true; }
        }
        p = eol + 1;
    }
    return seen && active;
}

static int self_test(HWND hwnd, bool wait_only) {
    DWORD pid = GetCurrentProcessId();
    hlog("self-test begin pid=%lu wait_only=%d", pid, wait_only);
    // 显式初始化本线程 TSF（真实 TSF 应用的标准动作；提权环境下 ctfmon
    // 不会自动为进程初始化，不建 ThreadMgr 则 profile 激活永远是空成功）
    ITfThreadMgr* tm = nullptr;
    TfClientId mytid = 0;
    HRESULT hr_tm = CoCreateInstance(kClsidTfThreadMgr, nullptr, CLSCTX_INPROC_SERVER,
                                     IID_ITfThreadMgr, (void**)&tm);
    if (SUCCEEDED(hr_tm)) {
        HRESULT hr_a = tm->Activate(&mytid);
        hlog("self-test ThreadMgr Activate hr=0x%08lx tid=%lu", hr_a, mytid);
    } else {
        hlog("self-test ThreadMgr create hr=0x%08lx", hr_tm);
    }
    // 点击自己夺取前台（真实输入事件是前台锁唯一认可的途径）
    for (int i = 0; i < 8 && GetForegroundWindow() != hwnd; i++) {
        click_self(hwnd);
        pump_for(300);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        pump_for(200);
    }
    hlog("self-test foreground=%d", GetForegroundWindow() == hwnd);
    bool active = false;
    if (wait_only) {
        // 路径 1：ActivateProfile API 直接激活（提权进程内 ctfmon 新初始化，
        // 不依赖输入模拟；真实用户物理 Win+Space 在提权窗口同样有效）
        ITfInputProcessorProfileMgr* ppm = nullptr;
        HRESULT hr = CoCreateInstance(kClsidTfProfiles, nullptr, CLSCTX_INPROC_SERVER,
                                      IID_ITfInputProcessorProfileMgr, (void**)&ppm);
        if (SUCCEEDED(hr)) {
            LANGID cur = (LANGID)(uintptr_t)GetKeyboardLayout(0);
            LANGID langs[] = { MAKELANGID(LANG_CHINESE, SUBLANG_CHINESE_SIMPLIFIED),
                               MAKELANGID(LANG_ENGLISH, SUBLANG_ENGLISH_US), cur };
            for (LANGID l : langs) {
                HRESULT hr2 = ppm->ActivateProfile(TF_PROFILETYPE_INPUTPROCESSOR, l,
                                                   kClsidVoime, kGuidVoimeProfile,
                                                   nullptr, 0);
                hlog("self-test ActivateProfile lang=0x%04x hr=0x%08lx", l, hr2);
                if (SUCCEEDED(hr2)) break;
            }
            ppm->Release();
        }
        for (int i = 0; i < 100 && !active; i++) {
            pump_for(250);
            active = self_tip_active(pid);
        }
    } else {
        // 路径 1（优先）：v1 经典序列 = 切语言 + ActivateLanguageProfile
        // （语言栏内部机制同源；ProfileMgr::ActivateProfile 在本机 zh 偶发
        // E_INVALIDARG、en 空成功，v1 序列实测更稳）
        ITfInputProcessorProfiles* ipp = nullptr;
        HRESULT hr = CoCreateInstance(kClsidTfProfiles, nullptr, CLSCTX_INPROC_SERVER,
                                      IID_ITfInputProcessorProfiles, (void**)&ipp);
        if (SUCCEEDED(hr)) {
            LANGID zh = MAKELANGID(LANG_CHINESE, SUBLANG_CHINESE_SIMPLIFIED);
            HRESULT hc = ipp->ChangeCurrentLanguage(zh);
            HRESULT ha = ipp->ActivateLanguageProfile(kClsidVoime, zh,
                                                      kGuidVoimeProfile);
            hlog("self-test ChangeCurrentLanguage hr=0x%08lx ActLangProfile hr=0x%08lx",
                 hc, ha);
            ipp->Release();
        }
        for (int i = 0; i < 40 && !active; i++) {
            pump_for(250);
            active = self_tip_active(pid);
        }
        // 路径 2（回退）：自驱动 Win+Space 直到本进程 Voime 激活（最多 12 步）
        for (int i = 0; i < 12 && !active; i++) {
            send_key(VK_LWIN, true); pump_for(100);
            send_key(VK_SPACE, true); pump_for(100);
            send_key(VK_SPACE, false); pump_for(100);
            send_key(VK_LWIN, false);
            for (int j = 0; j < 12 && !active; j++) {
                pump_for(250);
                active = self_tip_active(pid);
            }
            SetForegroundWindow(hwnd);
            pump_for(200);
        }
    }
    hlog("self-test tip active=%d", active);
    if (!active) {
        hlog("self-test FAIL: tip not activated");
        return 2;
    }
    // 2) 读 PTT 配置（与 TIP 同 config.toml）
    UINT mods = MOD_CONTROL, vk = VK_SPACE;
    {
        wchar_t dir[MAX_PATH];
        DWORD n = GetEnvironmentVariableW(L"APPDATA", dir, MAX_PATH);
        if (n && n < MAX_PATH) {
            std::wstring cp = std::wstring(dir) + L"\\VoimeInput\\config.toml";
            FILE* f = _wfopen(cp.c_str(), L"rb");
            if (f) {
                char buf[4096]{};
                size_t len = fread(buf, 1, sizeof(buf) - 1, f);
                fclose(f);
                buf[len] = 0;
                if (strstr(buf, "F13")) { mods = 0; vk = VK_F13; }
            }
        }
    }
    hlog("self-test ptt vk=0x%x mods=0x%x", vk, mods);
    // 3) 按住 PTT 9.5s（覆盖 WAV 全长）后松开
    if (mods & MOD_CONTROL) send_key(VK_LCONTROL, true);
    if (mods & MOD_ALT) send_key(VK_LMENU, true);
    pump_for(80);
    send_key((WORD)vk, true);
    pump_for(9500);
    send_key((WORD)vk, false);
    pump_for(80);
    if (mods & MOD_ALT) send_key(VK_LMENU, false);
    if (mods & MOD_CONTROL) send_key(VK_LCONTROL, false);
    // 4) 等 EDIT 出现文本（EN_CHANGE 即时 dump + 轮询控件）
    bool got = false;
    wchar_t buf[512]{};
    for (int i = 0; i < 80 && !got; i++) {
        pump_for(250);
        GetWindowTextW(g_edit, buf, 511);
        if (buf[0]) got = true;
    }
    hlog("self-test got=%d", got);
    // 结果落盘供 qa07 断言
    FILE* rf = fopen("tsf_harness_selftest.txt", "wb");
    if (rf) {
        char u8[2048]{};
        WideCharToMultiByte(CP_UTF8, 0, buf, -1, u8, sizeof(u8) - 1, nullptr, nullptr);
        fprintf(rf, "%s %s", got ? "PASS" : "FAIL", u8);
        fclose(rf);
    }
    return got ? 0 : 3;
}


static void hlog(const char* fmt, ...) {
    if (!g_log) return;
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    fprintf(g_log, "%s\n", buf);
    fflush(g_log);
}

static void dump_text() {
    wchar_t buf[4096]{};
    GetWindowTextW(g_edit, buf, 4095);
    // UTF-8 落盘（qa06 断言主信源；勿用 fwprintf 宽写——无 BOM 无法回读）
    char u8[8192]{};
    WideCharToMultiByte(CP_UTF8, 0, buf, -1, u8, sizeof(u8) - 1, nullptr, nullptr);
    FILE* f = fopen("tsf_harness_out.txt", "wb");
    if (f) {
        fwrite(u8, 1, strlen(u8), f);
        fclose(f);
    }
}

static WNDPROC g_edit_old = nullptr;
static LRESULT CALLBACK edit_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    if (msg == WM_KEYDOWN || msg == WM_CHAR || msg == WM_SETFOCUS ||
        msg == WM_KILLFOCUS || msg == WM_IME_CHAR || msg == WM_IME_STARTCOMPOSITION ||
        msg == WM_IME_ENDCOMPOSITION || msg == WM_IME_COMPOSITION) {
        hlog("edit msg=0x%x wp=0x%zx focus=%p", msg, (size_t)wp, GetFocus());
    }
    return CallWindowProcW(g_edit_old, hwnd, msg, wp, lp);
}

static LRESULT CALLBACK wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    if (msg == WM_TIMER && wp == 1) {
        // 自用焦点保障：跨进程 SetForegroundWindow 不一定生效，
        // 且跨进程 SetFocus 对异线程窗口无效 —— 本线程定时自检置顶+持焦
        if (GetForegroundWindow() != hwnd)
            SetForegroundWindow(hwnd); // 需 qa06 AllowSetForegroundWindow 授权
        if (GetForegroundWindow() == hwnd && GetFocus() != g_edit)
            SetFocus(g_edit);
        dump_text();
        return 0;
    }
    if (msg == WM_KEYDOWN || msg == WM_CHAR || msg == WM_SETFOCUS ||
        msg == WM_KILLFOCUS || msg == WM_ACTIVATE) {
        hlog("top wnd msg=0x%x wp=0x%zx fg=%p focus=%p", msg, (size_t)wp,
             GetForegroundWindow(), GetFocus());
    }
    if (msg == WM_COMMAND && HIWORD(wp) == EN_CHANGE) {
        dump_text(); // 内容变化即时落盘（延迟测量主信源）
        return 0;
    }
    if (msg == WM_DESTROY) {
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcW(hwnd, msg, wp, lp);
}

int WINAPI wWinMain(HINSTANCE inst, HINSTANCE, PWSTR, int) {
    g_log = fopen("tsf_harness.log", "wb");
    CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

    int secs = 60; // --seconds N
    LPWSTR cmd = GetCommandLineW();
    wchar_t* p = wcsstr(cmd, L"--seconds");
    if (p) secs = _wtoi(p + 9);
    if (secs <= 0) secs = 60;
    bool selftest = wcsstr(cmd, L"--self-test") != nullptr;
    bool selftest_wait = wcsstr(cmd, L"--self-test-wait") != nullptr;

    WNDCLASSEXW wc{ sizeof(wc) };
    wc.lpfnWndProc = wnd_proc;
    wc.hInstance = inst;
    wc.lpszClassName = kCls;
    wc.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    RegisterClassExW(&wc);

    // 注意：不在此自激活（ActivateProfile）。实测该 API 在本机对 zh 偶发
    // E_INVALIDARG、对 en 返回 S_OK 但 TIP 不实例化，且遗留的半激活态会
    // 干扰语言栏切换路径。激活统一由 qa06 经 Win+Space（真实用户路径）驱动。

    // 把线程键盘切到 zh：Win+Space 选中“zh-Voime”时，en 键盘线程会
    // 因语言不匹配在 ~0.5s 后被 ctfmon 回收（实测 Activate 后立即 Deactivate）
    LoadKeyboardLayoutW(L"00000804", KLF_ACTIVATE);

    HWND hwnd = CreateWindowExW(WS_EX_TOPMOST, kCls, L"Voime TSF Harness",
                                WS_OVERLAPPEDWINDOW, 300, 300, 480, 200, nullptr, nullptr,
                                inst, nullptr);
    // RichEdit（原生 TSF/ITextStoreACP）；失败回退标准 EDIT（系统 ACP 包装）
    HMODULE hRte = LoadLibraryW(L"msftedit.dll");
    if (hRte)
        g_edit = CreateWindowExW(0, L"RICHEDIT50W", L"",
                                 WS_CHILD | WS_VISIBLE | ES_MULTILINE,
                                 0, 0, 470, 160, hwnd, (HMENU)1, inst, nullptr);
    if (!g_edit)
        g_edit = CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE | ES_MULTILINE,
                                 0, 0, 470, 160, hwnd, (HMENU)1, inst, nullptr);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetFocus(g_edit);
    g_edit_old = (WNDPROC)SetWindowLongPtrW(g_edit, GWLP_WNDPROC, (LONG_PTR)edit_proc);
    // RichEdit 需要显式开启 EN_CHANGE 通知；EDIT 默认开启
    SendMessageW(g_edit, EM_SETEVENTMASK, 0, ENM_CHANGE);
    SetTimer(hwnd, 1, 250, nullptr);
    hlog("harness ready hwnd=%p edit=%p richedit=%d seconds=%d", hwnd, g_edit,
         hRte != nullptr, secs);

    if (selftest || selftest_wait) {
        pump_for(1000); // 等窗口/ctfmon 稳定
        int rc = self_test(hwnd, selftest_wait);
        hlog("self-test end rc=%d", rc);
        fclose(g_log);
        CoUninitialize();
        return rc;
    }

    DWORD end = GetTickCount() + (DWORD)secs * 1000;
    MSG msg;
    while (GetMessage(&msg, nullptr, 0, 0)) {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
        if ((int)(GetTickCount() - end) > 0) break;
    }
    dump_text();
    hlog("harness exit");
    fclose(g_log);
    CoUninitialize();
    return 0;
}
