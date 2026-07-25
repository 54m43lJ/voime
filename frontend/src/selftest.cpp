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
#include "wav.h"

#include <chrono>
#include <cmath>
#include <cstdio>
#include <psapi.h>
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

// ---- QA-05：浸泡测试（内存稳定性 + 延迟统计 + 动态内存） ----
struct MemInfo { double working_set_mb, private_mb; };
static MemInfo mem_now() {
    PROCESS_MEMORY_COUNTERS_EX pmc{};
    pmc.cb = sizeof(pmc);
    GetProcessMemoryInfo(GetCurrentProcess(), (PROCESS_MEMORY_COUNTERS*)&pmc, sizeof(pmc));
    return { pmc.WorkingSetSize / 1048576.0, pmc.PrivateUsage / 1048576.0 };
}

int run_soak_selftest(int sessions, const std::wstring& wav_path) {
    printf("==== QA-05 浸泡测试（%d 会话）====\n", sessions);
    Engine engine;
    std::wstring dll = exe_dir() + L"voime_core.dll";
    if (!engine.load(dll.c_str()) || !engine.create()) {
        printf("  [FAIL] 引擎初始化: %s\n", engine.last_error().c_str());
        return 1;
    }
    if (!engine.model_ready()) {
        printf("  [SKIP] 模型未就绪\n");
        return 1;
    }
    Wav w = read_wav16(wav_path);
    if (!w.ok || w.rate != 16000 || w.channels != 1) {
        printf("  [FAIL] wav 无效（需 16k s16 mono）\n");
        return 1;
    }
    MemInfo base = mem_now();
    printf("  基线（模型已加载）: WS=%.0fMB Private=%.0fMB\n", base.working_set_mb, base.private_mb);

    HANDLE ev = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    engine.register_event(ev);
    std::vector<double> latencies;
    std::vector<double> priv_series;
    double peak_private = 0;
    int fails = 0;
    const size_t chunk = 480;

    for (int s = 0; s < sessions; s++) {
        if (engine.start() != 0) { printf("  [FAIL] 会话 %d start 非 0\n", s); fails++; break; }
        // 实时节奏推送（PRD 延迟场景：松开时仅剩末句待处理）
        auto next = std::chrono::steady_clock::now();
        for (size_t off = 0; off + chunk <= w.pcm.size(); off += chunk) {
            engine.push(w.pcm.data() + off, (uint32_t)chunk);
            next += std::chrono::milliseconds(30);
            std::this_thread::sleep_until(next);
        }
        auto t0 = std::chrono::steady_clock::now();
        engine.stop();
        // 等 final
        bool got_final = false;
        auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(20);
        while (std::chrono::steady_clock::now() < deadline) {
            if (WaitForSingleObject(ev, 300) != WAIT_OBJECT_0) continue;
            std::string json;
            while (engine.poll(json)) {
                if (json_get_string(json, "type") == "final") got_final = true;
            }
            if (got_final) break;
        }
        double lat = std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count();
        if (!got_final) { printf("  [FAIL] 会话 %d 超时未收到 final\n", s); fails++; break; }
        latencies.push_back(lat);
        MemInfo m = mem_now();
        priv_series.push_back(m.private_mb);
        if (m.private_mb > peak_private) peak_private = m.private_mb;
        if (s < 3 || s == sessions - 1 || (s + 1) % 5 == 0)
            printf("  会话 %2d: final 延迟 %5.0fms  WS=%6.0fMB Private=%6.0fMB\n",
                   s + 1, lat, m.working_set_mb, m.private_mb);
    }
    CloseHandle(ev);

    // 延迟统计
    double sum = 0, mx = 0;
    for (double l : latencies) { sum += l; mx = std::max(mx, l); }
    double avg = latencies.empty() ? 0 : sum / latencies.size();
    char d1[128];
    snprintf(d1, sizeof(d1), "avg=%.0fms max=%.0fms (n=%zu, 目标<500ms)", avg, mx, latencies.size());
    report(mx < 500 && mx > 0, "soak.latency", d1);

    // 内存稳定性：跳过前 2 次预热后，末次相对第 3 次增长 < 50MB
    if (priv_series.size() >= 4) {
        double growth = priv_series.back() - priv_series[2];
        char d2[128];
        snprintf(d2, sizeof(d2), "预热后增长 %.1fMB (阈值<50MB)", growth);
        report(growth < 50.0, "soak.memory_stable", d2);
    } else {
        skip("soak.memory_stable", "会话数不足");
    }
    // 动态内存（PRD <200MB，不含模型）：会话峰值相对基线增量
    double dyn = peak_private - base.private_mb;
    char d3[128];
    snprintf(d3, sizeof(d3), "会话动态增量 %.1fMB (目标<200MB)", dyn);
    report(dyn < 200.0, "soak.dynamic_mem", d3);

    printf("==== 汇总: PASS=%d FAIL=%d SKIP=%d ====\n", g_pass, g_fail + fails, g_skip);
    engine.destroy();
    return (g_fail + fails) > 0 ? 1 : 0;
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
