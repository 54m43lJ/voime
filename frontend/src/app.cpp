// app.cpp — 应用编排实现
#include "app.h"
#include "audio_capture.h"
#include "config.h"
#include "deps.h"
#include "engine.h"
#include "hotkey.h"
#include "injector.h"
#include "overlay.h"
#include "util.h"

#include <shellapi.h>
#include <cstdarg>
#include <cstdio>

static const wchar_t* kMsgWndClass = L"VoimeMsgWnd";
static const UINT WM_TRAY = WM_APP + 10;
static const UINT_PTR kWatchdogTimer = 2;   // 热键守护（5s）
static const UINT_PTR kHideTimer = 3;       // 完成后淡出延时（900ms）

struct App::Impl {
    HINSTANCE inst = nullptr;
    HWND msg_wnd = nullptr;
    Config cfg;
    Engine engine;
    AudioCapture capture;
    Hotkey hotkey;
    Overlay overlay;
    HANDLE engine_event = nullptr;
    NOTIFYICONDATAW tray{};
    bool tray_added = false;
    bool deps_ok = false;
    bool recording = false;
    bool quitting = false;
    std::wstring session_text; // 本会话已累积的 partial 文本
    FILE* log_fp = nullptr;
};

void App::log(const char* fmt, ...) {
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    if (p_->log_fp) {
        SYSTEMTIME st{};
        GetLocalTime(&st);
        fprintf(p_->log_fp, "[%02d:%02d:%02d.%03d] %s\n", st.wHour, st.wMinute, st.wSecond,
                st.wMilliseconds, buf);
        fflush(p_->log_fp);
    }
    OutputDebugStringA(buf);
    OutputDebugStringA("\n");
}

int App::run(HINSTANCE inst, bool with_console) {
    p_ = new Impl();
    int rc = 1;
    if (init(inst)) {
        rc = 0;
        message_loop();
    }
    shutdown();
    delete p_;
    p_ = nullptr;
    (void)with_console;
    return rc;
}

bool App::init(HINSTANCE inst) {
    p_->inst = inst;
    CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

    // 配置
    bool created = false;
    std::wstring cfg_path = config_path();
    p_->cfg = load_config(cfg_path, created);
    std::wstring log_path = cfg_path.substr(0, cfg_path.find_last_of(L"\\/")) + L"\\voime.log";
    p_->log_fp = _wfopen(log_path.c_str(), L"ab");
    log("==== Voime 启动 ==== 配置: %s%s", wide_to_utf8(cfg_path).c_str(),
        created ? "（新建默认）" : "");

    // 隐藏消息窗口
    WNDCLASSEXW wc{ sizeof(wc) };
    wc.lpfnWndProc = &App::wnd_proc;
    wc.hInstance = inst;
    wc.lpszClassName = kMsgWndClass;
    RegisterClassExW(&wc);
    p_->msg_wnd = CreateWindowExW(0, kMsgWndClass, L"Voime Msg", 0, 0, 0, 0, 0,
                                  HWND_MESSAGE, nullptr, inst, nullptr);
    SetWindowLongPtrW(p_->msg_wnd, GWLP_USERDATA, (LONG_PTR)this);

    // CPP-04：FFmpeg 依赖自检（PRD 4.2 依赖管理器）
    auto chk = deps::check_ffmpeg(p_->cfg.ffmpeg_version);
    log("%s", chk.detail.c_str());
    if (!chk.ok) {
        std::wstring msg = L"未检测到音频组件或组件版本冲突，是否尝试重新下载正确的依赖？\n\n" +
                           utf8_to_wide(chk.detail);
        int choice = MessageBoxW(nullptr, msg.c_str(), L"Voime 依赖自检", MB_YESNO | MB_ICONWARNING);
        if (choice == IDYES) {
            std::string rlog;
            bool ok = deps::repair_ffmpeg(p_->cfg.ffmpeg_url, rlog);
            log("%s", rlog.c_str());
            if (ok) {
                MessageBoxW(nullptr, L"音频组件修复完成，请重启应用。", L"Voime",
                            MB_OK | MB_ICONINFORMATION);
                return false; // 提示重启
            }
            MessageBoxW(nullptr, utf8_to_wide("修复失败：\n" + rlog).c_str(), L"Voime",
                        MB_OK | MB_ICONERROR);
        } else {
            log("用户选择不修复，语音录入功能禁用");
        }
        p_->deps_ok = false; // 允许进入主界面手动处理（录入被禁用）
    } else {
        p_->deps_ok = true;
    }

    // CPP-01：加载引擎
    std::wstring dll = exe_dir() + L"voime_core.dll";
    if (!p_->engine.load(dll.c_str()) || !p_->engine.create()) {
        std::wstring msg = L"核心引擎加载失败：\n" + dll + L"\n" +
                           utf8_to_wide(p_->engine.last_error());
        MessageBoxW(nullptr, msg.c_str(), L"Voime", MB_OK | MB_ICONERROR);
        return false;
    }
    log("引擎版本: %s", p_->engine.version().c_str());
    p_->engine.set_lang_hint(lang_hint_from_string(p_->cfg.lang));

    p_->engine_event = CreateEventW(nullptr, FALSE, FALSE, nullptr); // 自动复位
    p_->engine.register_event(p_->engine_event);

    if (!p_->engine.model_ready()) {
        std::wstring msg =
            L"模型文件缺失或未加载。\n\n请将模型放入：\n%APPDATA%\\VoimeInput\\models\\\n"
            L"或设置环境变量 VOIME_MODEL_DIR 指向模型目录，然后重启应用。";
        MessageBoxW(nullptr, msg.c_str(), L"Voime 模型管理", MB_OK | MB_ICONWARNING);
        log("模型未就绪（start_session 将返回 -4）");
    } else {
        log("模型已就绪");
    }

    // CPP-05：全局热键
    if (!p_->hotkey.install(p_->msg_wnd, p_->cfg.hotkey)) {
        std::wstring msg = L"全局热键注册失败：" + p_->cfg.hotkey +
                           L"\n请检查配置 hotkey 字段。";
        MessageBoxW(nullptr, msg.c_str(), L"Voime", MB_OK | MB_ICONERROR);
        return false;
    }
    SetTimer(p_->msg_wnd, kWatchdogTimer, 5000, nullptr);

    // CPP-06：悬浮窗
    if (!p_->overlay.create(inst)) {
        MessageBoxW(nullptr, L"悬浮窗初始化失败（Direct2D）。", L"Voime", MB_OK | MB_ICONERROR);
        return false;
    }

    // 系统托盘（最小：图标 + 退出）
    p_->tray.cbSize = sizeof(p_->tray);
    p_->tray.hWnd = p_->msg_wnd;
    p_->tray.uID = 1;
    p_->tray.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    p_->tray.uCallbackMessage = WM_TRAY;
    p_->tray.hIcon = LoadIconW(nullptr, IDI_APPLICATION);
    wcscpy_s(p_->tray.szTip, L"Voime 语音输入");
    p_->tray_added = Shell_NotifyIconW(NIM_ADD, &p_->tray) != FALSE;

    log("初始化完成，PTT=%s", wide_to_utf8(p_->cfg.hotkey).c_str());
    return true;
}

void App::message_loop() {
    for (;;) {
        DWORD r = MsgWaitForMultipleObjectsEx(1, &p_->engine_event, INFINITE, QS_ALLINPUT,
                                              MWMO_INPUTAVAILABLE);
        if (r == WAIT_OBJECT_0) {
            drain_results(); // CPP-07：主线程捕获 Event -> poll
        } else if (r == WAIT_OBJECT_0 + 1) {
            MSG msg;
            while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
                if (msg.message == WM_QUIT) return;
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        } else {
            log("消息循环等待失败 r=%lu gle=%lu", r, GetLastError());
            return;
        }
    }
}

LRESULT CALLBACK App::wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    App* self = (App*)GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if (self) return self->on_message(msg, wp, lp);
    return DefWindowProcW(hwnd, msg, wp, lp);
}

LRESULT App::on_message(UINT msg, WPARAM wp, LPARAM lp) {
    if (msg == Hotkey::WM_PRESS) { on_ptt_press(); return 0; }
    if (msg == Hotkey::WM_RELEASE) { on_ptt_release(); return 0; }
    if (msg == WM_TIMER) {
        if (wp == kWatchdogTimer) p_->hotkey.watchdog_tick();
        else if (wp == kHideTimer) { KillTimer(p_->msg_wnd, kHideTimer); p_->overlay.hide(); }
        return 0;
    }
    if (msg == WM_TRAY) {
        if (LOWORD(lp) == WM_RBUTTONUP || LOWORD(lp) == WM_CONTEXTMENU) {
            HMENU menu = CreatePopupMenu();
            AppendMenuW(menu, MF_STRING, 1, L"退出");
            POINT pt{};
            GetCursorPos(&pt);
            SetForegroundWindow(p_->msg_wnd); // 菜单失焦自动消失所需
            int cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, 0,
                                     p_->msg_wnd, nullptr);
            DestroyMenu(menu);
            if (cmd == 1) PostQuitMessage(0);
        }
        return 0;
    }
    return DefWindowProcW(p_->msg_wnd, msg, wp, lp);
}

void App::on_ptt_press() {
    if (p_->recording) return;
    if (!p_->deps_ok) {
        p_->overlay.show(L"音频组件缺失，语音录入已禁用");
        SetTimer(p_->msg_wnd, kHideTimer, 1200, nullptr);
        return;
    }
    int rc = p_->engine.start();
    if (rc == -3) {
        p_->overlay.show(L"正在收尾，请稍候...");
        SetTimer(p_->msg_wnd, kHideTimer, 900, nullptr);
        return;
    }
    if (rc == -4) {
        p_->overlay.show(L"模型未加载，无法识别");
        SetTimer(p_->msg_wnd, kHideTimer, 1200, nullptr);
        return;
    }
    if (rc != 0) {
        log("start_session rc=%d", rc);
        return;
    }
    // 启动采集（FFmpeg 重采样在采集线程内完成）
    std::string err;
    bool started = p_->capture.start(
        [this](const int16_t* d, size_t n) {
            int prc = p_->engine.push(d, (uint32_t)n);
            if (prc == -2) log("队列溢出丢帧(-2)");
        },
        [this](const std::string& e) { log("采集错误: %s", e.c_str()); }, &err);
    if (!started) {
        log("采集启动失败: %s", err.c_str());
        p_->engine.stop();
        p_->overlay.show(L"无可用采集设备");
        SetTimer(p_->msg_wnd, kHideTimer, 1200, nullptr);
        return;
    }
    p_->recording = true;
    p_->session_text.clear();
    KillTimer(p_->msg_wnd, kHideTimer);
    p_->overlay.show(L"正在听...");
    log("PTT 按下，开始录音（%s）", p_->capture.device_format().c_str());
}

void App::on_ptt_release() {
    if (!p_->recording) return;
    p_->recording = false;
    p_->capture.stop();
    p_->engine.stop(); // 异步 Finalizing，结果经事件回报
    p_->overlay.set_status(L"识别中...");
    log("PTT 松开，进入 Finalizing");
}

void App::drain_results() {
    std::string json;
    while (p_->engine.poll(json)) handle_result(json);
}

void App::handle_result(const std::string& json) {
    std::string type = json_get_string(json, "type");
    log("结果: %s", json.c_str());
    if (type == "partial") {
        p_->session_text += utf8_to_wide(json_get_string(json, "text"));
        p_->overlay.set_text(p_->session_text);
    } else if (type == "final") {
        std::wstring text = utf8_to_wide(json_get_string(json, "text"));
        if (p_->recording) { // 60s 护栏自动收尾：停止采集
            p_->recording = false;
            p_->capture.stop();
        }
        p_->overlay.set_status(L"完成");
        p_->overlay.set_text(text);
        if (!text.empty()) {
            injector::inject_text(text); // CPP-08
            log("已注入 %zu 字符", text.size());
        }
        SetTimer(p_->msg_wnd, kHideTimer, 900, nullptr);
    } else if (type == "warning") {
        p_->overlay.set_status(L"已达 60s 上限，自动收尾");
    } else if (type == "error") {
        p_->overlay.set_status(L"引擎错误");
        log("引擎错误: %s", json.c_str());
    }
}

void App::shutdown() {
    if (!p_) return;
    log("==== Voime 退出 ====");
    if (p_->tray_added) Shell_NotifyIconW(NIM_DELETE, &p_->tray);
    p_->hotkey.uninstall();
    p_->capture.stop();
    p_->overlay.destroy();
    if (p_->engine_event) CloseHandle(p_->engine_event);
    p_->engine.destroy();
    if (p_->msg_wnd) DestroyWindow(p_->msg_wnd);
    if (p_->log_fp) fclose(p_->log_fp);
    CoUninitialize();
}
