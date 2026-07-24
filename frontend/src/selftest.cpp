// selftest.cpp — 控制台自测实现
//
// 覆盖验收点：
//   CPP-01 FFI 集成（版本打印/引擎创建）
//   CPP-02 WASAPI 采集（3s 实采，无设备则 SKIP）
//   CPP-03 FFmpeg 重采样（48k F32 立体声 -> 16k s16 mono，长度/频率校验）
//   CPP-04 依赖自检（DLL 存在/加载/版本）
//   CPP-07 事件 + poll_result 结果拉取（wav 文件 E2E 回放）
#include "selftest.h"
#include "audio_capture.h"
#include "deps.h"
#include "engine.h"
#include "hotkey.h"
#include "injector.h"
#include "overlay.h"
#include "resampler.h"
#include "util.h"

#include <chrono>
#include <cmath>
#include <cstdio>
#include <thread>
#include <vector>

static int g_pass = 0, g_fail = 0, g_skip = 0;
static void report(bool ok, const char* name, const std::string& detail) {
    printf("  [%s] %s %s\n", ok ? "PASS" : "FAIL", name, detail.c_str());
    if (ok) g_pass++; else g_fail++;
}
static void skip(const char* name, const std::string& why) {
    printf("  [SKIP] %s %s\n", name, why.c_str());
    g_skip++;
}

// ---- 简易 WAV 读取（PCM16 / F32） ----
struct Wav {
    std::vector<int16_t> pcm; // s16 交错
    int rate = 0, channels = 0;
    bool ok = false;
};
static Wav read_wav16(const std::wstring& path) {
    Wav w;
    FILE* f = _wfopen(path.c_str(), L"rb");
    if (!f) return w;
    uint8_t hdr[12];
    if (fread(hdr, 1, 12, f) != 12 || memcmp(hdr, "RIFF", 4) || memcmp(hdr + 8, "WAVE", 4)) {
        fclose(f);
        return w;
    }
    uint16_t fmt_tag = 0, ch = 0;
    uint32_t rate = 0, bits = 0;
    std::vector<uint8_t> data;
    for (;;) {
        uint8_t ch_hdr[8];
        if (fread(ch_hdr, 1, 8, f) != 8) break;
        uint32_t sz = *(uint32_t*)(ch_hdr + 4);
        long next = ftell(f) + sz + (sz & 1);
        if (!memcmp(ch_hdr, "fmt ", 4)) {
            uint8_t buf[40]{};
            fread(buf, 1, sz > 40 ? 40 : sz, f);
            fmt_tag = *(uint16_t*)buf;
            ch = *(uint16_t*)(buf + 2);
            rate = *(uint32_t*)(buf + 4);
            bits = *(uint16_t*)(buf + 14);
        } else if (!memcmp(ch_hdr, "data", 4)) {
            data.resize(sz);
            fread(data.data(), 1, sz, f);
        }
        fseek(f, next, SEEK_SET);
    }
    fclose(f);
    if (fmt_tag != 1 || bits != 16 || data.empty() || ch == 0) return w; // 仅收 PCM16
    w.pcm.assign((int16_t*)data.data(), (int16_t*)(data.data() + data.size()));
    w.rate = (int)rate;
    w.channels = ch;
    w.ok = true;
    return w;
}

// ---- CPP-01：FFI ----
static bool test_ffi(Engine& engine) {
    printf("== CPP-01 FFI 集成 ==\n");
    std::wstring dll = exe_dir() + L"voime_core.dll";
    if (!engine.load(dll.c_str())) {
        report(false, "engine.load", engine.last_error());
        return false;
    }
    printf("  Hello World from Rust core: %s\n", engine.version().c_str());
    report(true, "engine.load/version", engine.version());
    if (!engine.create()) {
        report(false, "engine.create", engine.last_error());
        return false;
    }
    report(true, "engine.create", "实例创建成功");
    printf("  model_ready=%d\n", engine.model_ready());
    return true;
}

// ---- CPP-04：依赖自检 ----
static void test_deps() {
    printf("== CPP-04 FFmpeg 依赖自检 ==\n");
    auto r = deps::check_ffmpeg("6.0");
    report(r.ok, "deps.check_ffmpeg", r.detail);
}

// ---- CPP-03：重采样正确性 ----
static void test_resampler() {
    printf("== CPP-03 FFmpeg 重采样（48k F32 立体声 -> 16k s16 mono）==\n");
    const int sr = 48000, ch = 2, secs = 1;
    const double freq = 440.0;
    std::vector<float> src((size_t)sr * secs * ch);
    for (int i = 0; i < sr * secs; i++) {
        float v = (float)sin(2.0 * 3.14159265358979 * freq * i / sr) * 0.5f;
        src[(size_t)i * 2] = v;
        src[(size_t)i * 2 + 1] = v;
    }
    Resampler rs;
    if (!rs.init(sr, ch, true)) {
        report(false, "resampler.init", "初始化失败");
        return;
    }
    std::vector<int16_t> out;
    // 分块喂入（模拟 10ms WASAPI 包）
    const int packet = sr / 100;
    for (int off = 0; off < sr * secs; off += packet)
        rs.convert((const uint8_t*)(src.data() + (size_t)off * ch), packet, out);

    // 长度校验：16000 ± 160（10ms 容差）
    bool len_ok = out.size() > 15840 && out.size() < 16160;
    char d1[96];
    snprintf(d1, sizeof(d1), "输出 %zu 采样（期望 ~16000）", out.size());
    report(len_ok, "resampler.length", d1);

    // 频率校验：过零率估算（440Hz -> 每秒 880 次过零）
    int crossings = 0;
    for (size_t i = 1; i < out.size(); i++)
        if ((out[i - 1] < 0) != (out[i] < 0)) crossings++;
    double est = crossings / 2.0 / secs;
    char d2[96];
    snprintf(d2, sizeof(d2), "估算频率 %.1fHz（期望 440±20）", est);
    report(est > 420.0 && est < 460.0, "resampler.freq", d2);

    // 幅度校验：0.5 * 32767 ≈ 16384（±20%）
    int16_t peak = 0;
    for (auto v : out) peak = (int16_t)std::max((int)peak, (int)abs(v));
    char d3[96];
    snprintf(d3, sizeof(d3), "峰值 %d（期望 ~16384）", peak);
    report(peak > 13000 && peak < 20000, "resampler.amplitude", d3);
}

// ---- CPP-07：事件 + poll E2E（wav 回放） ----
static void test_engine_e2e(Engine& engine, const std::wstring& wav_path) {
    printf("== CPP-07 事件/poll 结果拉取（wav E2E）==\n");
    if (!engine.model_ready()) {
        skip("engine.e2e", "模型未就绪");
        return;
    }
    Wav w = read_wav16(wav_path);
    if (!w.ok || w.rate != 16000 || w.channels != 1) {
        skip("engine.e2e", "wav 缺失或非 16k s16 mono: " + wide_to_utf8(wav_path));
        return;
    }
    HANDLE ev = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    engine.register_event(ev);
    if (engine.start() != 0) {
        report(false, "engine.e2e.start", "start_session 非 0");
        CloseHandle(ev);
        return;
    }
    const size_t chunk = 480;
    size_t pushed = 0;
    for (size_t off = 0; off + chunk <= w.pcm.size(); off += chunk) {
        if (engine.push(w.pcm.data() + off, (uint32_t)chunk) == 0) pushed++;
    }
    engine.stop();

    int partials = 0;
    std::string final_text, final_json;
    auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(30);
    while (std::chrono::steady_clock::now() < deadline) {
        DWORD r = WaitForSingleObject(ev, 500);
        if (r != WAIT_OBJECT_0) continue;
        std::string json;
        while (engine.poll(json)) {
            std::string type = json_get_string(json, "type");
            if (type == "partial") {
                partials++;
                printf("  [partial] %s\n", json_get_string(json, "text").c_str());
            } else if (type == "final") {
                final_json = json;
                final_text = json_get_string(json, "text");
            }
        }
        if (!final_json.empty()) break;
    }
    CloseHandle(ev);
    char d[64];
    snprintf(d, sizeof(d), "推送 %zu chunk，partial=%d", pushed, partials);
    report(partials > 0, "engine.e2e.partials", d);
    printf("  [final] %s\n", final_text.c_str());
    report(!final_text.empty(), "engine.e2e.final", final_json.empty() ? "超时未收到 final" : "收到 final");
}

// ---- CPP-02：WASAPI 实采 ----
static void test_wasapi() {
    printf("== CPP-02 WASAPI 共享模式采集（3s）==\n");
    AudioCapture cap;
    std::string err;
    uint64_t chunks = 0, samples = 0;
    bool started = cap.start(
        [&](const int16_t*, size_t n) { chunks++; samples += n; },
        [&](const std::string& e) { printf("  [wasapi-err] %s\n", e.c_str()); }, &err);
    if (!started) {
        skip("wasapi.capture", "无采集设备或初始化失败: " + err);
        return;
    }
    printf("  设备格式: %s\n", cap.device_format().c_str());
    Sleep(3000);
    cap.stop();
    char d[96];
    snprintf(d, sizeof(d), "3s 采集 chunk=%llu 采样=%llu（期望 ~100 chunk）",
             (unsigned long long)chunks, (unsigned long long)samples);
    report(chunks > 80, "wasapi.capture", d);
}

// ---- CPP-05：热键钩子（SendInput 注入 Ctrl+Space 验证捕获） ----
static const wchar_t* kTestWnd = L"VoimeSelftestWnd";
static int g_press = 0, g_release = 0;
static LRESULT CALLBACK test_wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    if (msg == Hotkey::WM_PRESS) g_press++;
    if (msg == Hotkey::WM_RELEASE) g_release++;
    return DefWindowProcW(hwnd, msg, wp, lp);
}
static HWND make_test_wnd() {
    WNDCLASSEXW wc{ sizeof(wc) };
    wc.lpfnWndProc = test_wnd_proc;
    wc.hInstance = GetModuleHandleW(nullptr);
    wc.lpszClassName = kTestWnd;
    RegisterClassExW(&wc);
    return CreateWindowExW(0, kTestWnd, L"t", 0, 0, 0, 0, 0, HWND_MESSAGE, nullptr,
                           GetModuleHandleW(nullptr), nullptr);
}
static void pump(int ms) {
    auto end = std::chrono::steady_clock::now() + std::chrono::milliseconds(ms);
    MSG msg;
    while (std::chrono::steady_clock::now() < end) {
        while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Sleep(5);
    }
}
static void send_key(UINT vk, bool down) {
    INPUT in{};
    in.type = INPUT_KEYBOARD;
    in.ki.wVk = (WORD)vk;
    in.ki.dwFlags = down ? 0 : KEYEVENTF_KEYUP;
    SendInput(1, &in, sizeof(INPUT));
}
static void test_hotkey() {
    printf("== CPP-05 全局热键（WH_KEYBOARD_LL 捕获 Ctrl+Space）==\n");
    HWND wnd = make_test_wnd();
    Hotkey hk;
    if (!hk.install(wnd, L"Ctrl+Space")) {
        report(false, "hotkey.install", "SetWindowsHookEx 失败");
        DestroyWindow(wnd);
        return;
    }
    report(true, "hotkey.install", "钩子安装成功");
    send_key(VK_LCONTROL, true);
    Sleep(30);
    send_key(VK_SPACE, true);
    Sleep(30);
    send_key(VK_SPACE, false);
    Sleep(30);
    send_key(VK_LCONTROL, false);
    pump(500);
    char d[64];
    snprintf(d, sizeof(d), "press=%d release=%d", g_press, g_release);
    report(g_press == 1 && g_release == 1, "hotkey.capture", d);
    hk.uninstall();
    DestroyWindow(wnd);
}

// ---- CPP-06：悬浮窗冒烟 ----
static void test_overlay() {
    printf("== CPP-06 Direct2D 悬浮窗（创建/显示/绘制/淡出 冒烟）==\n");
    Overlay ov;
    if (!ov.create(GetModuleHandleW(nullptr))) {
        report(false, "overlay.create", "D2D 初始化失败");
        return;
    }
    DWORD ex = GetWindowLongW(ov.hwnd(), GWL_EXSTYLE);
    bool style_ok = (ex & WS_EX_TOPMOST) && (ex & WS_EX_LAYERED) && (ex & WS_EX_NOACTIVATE) &&
                    (ex & WS_EX_TOOLWINDOW);
    report(style_ok, "overlay.style", "无边框/分层/置顶/不抢焦点样式");
    ov.show(L"正在听...");
    ov.set_text(L"你好世界 hello speech 测试文本");
    pump(400);
    bool vis = IsWindowVisible(ov.hwnd()) != 0;
    report(vis, "overlay.show", "淡入后可见");
    ov.hide();
    pump(400);
    report(!IsWindowVisible(ov.hwnd()), "overlay.hide", "淡出后隐藏");
    ov.destroy();
}

// ---- CPP-08：SendInput 注入（自建 EDIT 控件验证文本） ----
static void test_injector() {
    printf("== CPP-08 模拟键盘注入（EDIT 控件收文本）==\n");
    HWND host = make_test_wnd();
    HWND edit = CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE | ES_MULTILINE,
                                0, 0, 200, 60, host, (HMENU)1, GetModuleHandleW(nullptr), nullptr);
    ShowWindow(host, SW_SHOW);
    SetForegroundWindow(host);
    SetFocus(edit);
    pump(100);
    if (GetFocus() != edit) { // 防止注入到其它窗口（如终端）
        report(false, "injector.unicode", "EDIT 未获得焦点，放弃注入");
        DestroyWindow(host);
        return;
    }
    const wchar_t* expect = L"你好世界Hello123。";
    injector::inject_text(expect);
    pump(400);
    wchar_t buf[128]{};
    GetWindowTextW(edit, buf, 127);
    report(wcscmp(buf, expect) == 0, "injector.unicode",
           std::string("EDIT 内容: ") + wide_to_utf8(buf));
    DestroyWindow(host);
}

// ---- CPP-04 修复流程（--repair 时调用；会真实下载 ~40MB） ----
int run_repair_selftest() {
    printf("== CPP-04 依赖修复流程（移动 DLL -> 自检失败 -> 下载修复 -> 自检通过）==\n");
    std::wstring dir = exe_dir();
    std::wstring swr = dir + L"swresample-4.dll";
    std::wstring bak = dir + L"swresample-4.dll.bak";
    DeleteFileW(bak.c_str());
    if (!MoveFileW(swr.c_str(), bak.c_str())) {
        report(false, "repair.setup", "无法移走 swresample-4.dll");
        return 1;
    }
    auto r1 = deps::check_ffmpeg("6.0");
    report(!r1.ok, "repair.detect", "缺失被检出: " + r1.detail);
    std::string log;
    bool ok = deps::repair_ffmpeg(
        "https://github.com/GyanD/codexffmpeg/releases/download/6.0/ffmpeg-6.0-full_build-shared.7z",
        log);
    printf("%s", log.c_str());
    if (!ok && GetFileAttributesW(swr.c_str()) == INVALID_FILE_ATTRIBUTES) {
        MoveFileW(bak.c_str(), swr.c_str()); // 修复失败则还原
    }
    DeleteFileW(bak.c_str());
    report(ok, "repair.download", "下载+解压+覆盖");
    auto r2 = deps::check_ffmpeg("6.0");
    report(r2.ok, "repair.verify", r2.detail);
    printf("==== 汇总: PASS=%d FAIL=%d SKIP=%d ====\n", g_pass, g_fail, g_skip);
    return g_fail > 0 ? 1 : 0;
}

int run_selftest(const std::wstring& wav_path) {
    printf("==== Voime 阶段三 C++ 前端自测 ====\n\n");
    Engine engine;
    if (!test_ffi(engine)) {
        printf("\n引擎不可用，终止后续测试\n");
        return 1;
    }
    printf("\n");
    test_deps();
    printf("\n");
    test_resampler();
    printf("\n");
    test_engine_e2e(engine, wav_path);
    printf("\n");
    test_hotkey();
    printf("\n");
    test_overlay();
    printf("\n");
    test_injector();
    printf("\n");
    test_wasapi();
    printf("\n==== 汇总: PASS=%d FAIL=%d SKIP=%d ====\n", g_pass, g_fail, g_skip);
    engine.destroy();
    return g_fail > 0 ? 1 : 0;
}
