// app.cpp — 应用编排实现
#include "app.h"
#include "audio_capture.h"
#include "config.h"
#include "deps.h"
#include "engine.h"
#ifdef VOIME_NOTSF
#include "hotkey.h"
#include "injector.h"
#else
#include "ipc_proto.h"
#include "ipc_server.h"
#endif
#include "models.h"
#include "overlay.h"
#include "util.h"
#include "wav_source.h"
#include "wizard.h"

#include <shellapi.h>
#include <commctrl.h>
#include <cstdarg>
#include <cstdio>

static const wchar_t* kMsgWndClass = L"VoimeMsgWnd";
static const UINT WM_TRAY = WM_APP + 10;
static const UINT WM_CAPTURE_ERROR = WM_APP + 11;
static const UINT WM_IPC_CMD = WM_APP + 20;  // 标准模式：IPC 指令（lp=堆分配 IpcCmdMsg*）
static const UINT_PTR kWatchdogTimer = 2;    // 热键守护（5s，仅 NOTSF）
static const UINT_PTR kHideTimer = 3;        // 完成后淡出延时（900ms）

// IPC 指令投递体（reader 线程 -> 主线程）
struct IpcCmdMsg {
    int client_id;
    int kind; // 1=start_session 0=stop_session 2=client disconnected
    std::string request_id;
};

struct App::Impl {
    HINSTANCE inst = nullptr;
    HWND msg_wnd = nullptr;
    AppOptions opts;
    Config cfg;
    Engine engine;
    AudioCapture capture;
    WavSource wav_source;
#ifdef VOIME_NOTSF
    Hotkey hotkey;
#else
    IpcServer ipc;
    int session_client = -1; // 发起当前会话的 TSF 客户端（commit_text 路由目标）
#endif
    Overlay overlay;
    HANDLE engine_event = nullptr;
    NOTIFYICONDATAW tray{};
    bool tray_added = false;
    bool deps_ok = false;
    bool recording = false;
    bool wav_mode = false;        // 当前会话是否 WAV 音源
    std::string last_capture_err; // 采集线程 -> 主线程传递
    std::wstring session_text;    // 本会话已累积的 partial 文本
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

int App::run(HINSTANCE inst, const AppOptions& opts) {
    p_ = new Impl();
    p_->opts = opts;
    int rc = 1;
    if (init(inst)) {
        rc = 0;
        message_loop();
    }
    shutdown();
    delete p_;
    p_ = nullptr;
    return rc;
}

bool App::init(HINSTANCE inst) {
    p_->inst = inst;
    CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    InitCommonControls(); // 进度条等通用控件（模型下载向导）

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
        // M4 首启模型下载向导（QA-03）：检测 -> 下载 -> 校验 -> 热加载
        std::wstring mdir = models::models_dir();
        std::string url = p_->opts.models_url.empty() ? p_->cfg.models_url : p_->opts.models_url;
        log("模型未就绪，启动下载向导（%s）", url.c_str());
        int wr = wizard::run(inst, url, mdir, p_->opts.auto_download_models);
        if (wr == wizard::Ok) {
            if (p_->engine.load_models(wide_to_utf8(mdir)) == 0 && p_->engine.model_ready()) {
                log("模型下载并热加载成功");
            } else {
                MessageBoxW(nullptr, L"模型已下载但加载失败，请查看日志。", L"Voime 模型管理",
                            MB_OK | MB_ICONERROR);
                log("模型下载后加载失败");
                return false;
            }
        } else if (wr == wizard::Failed) {
            log("模型下载失败，退出");
            return false;
        } else {
            log("用户取消模型下载，语音识别不可用（start_session 将返回 -4）");
        }
    } else {
        log("模型已就绪");
    }

    // WAV 音源预加载（QA 自动化）
    if (!p_->opts.audio_wav.empty()) {
        if (!p_->wav_source.load(p_->opts.audio_wav)) {
            std::wstring msg = L"WAV 音源加载失败（需 16k s16 mono）：\n" + p_->opts.audio_wav;
            MessageBoxW(nullptr, msg.c_str(), L"Voime", MB_OK | MB_ICONERROR);
            return false;
        }
        p_->wav_source.set_loop(p_->opts.wav_loop);
        log("WAV 音源: %s%s", wide_to_utf8(p_->opts.audio_wav).c_str(),
            p_->opts.wav_loop ? "（循环）" : "");
    }

#ifdef VOIME_NOTSF
    // CPP-05：全局热键（NOTSF 模式：Agent 主动触发）
    if (!p_->hotkey.install(p_->msg_wnd, p_->cfg.hotkey)) {
        std::wstring msg = L"全局热键注册失败：" + p_->cfg.hotkey +
                           L"\n请检查配置 hotkey 字段。";
        MessageBoxW(nullptr, msg.c_str(), L"Voime", MB_OK | MB_ICONERROR);
        return false;
    }
    SetTimer(p_->msg_wnd, kWatchdogTimer, 5000, nullptr);
#else
    // P5-03：IPC 服务端（标准模式：控制反转，被动响应 TSF DLL 指令）
    IpcServer::Events ev;
    ev.on_message = [this](int client_id, const std::string& json) {
        // reader 线程上下文：仅解析动作类型并转投主线程
        std::string action = json_get_string(json, "action");
        if (action == "start_session" || action == "stop_session") {
            auto* m = new IpcCmdMsg{ client_id, action == "start_session" ? 1 : 0,
                                     json_get_string(json, "request_id") };
            PostMessageW(p_->msg_wnd, WM_IPC_CMD, 0, (LPARAM)m);
        } else {
            log("IPC 收到: %s", json.c_str()); // commit_text 应答等
        }
    };
    ev.on_client_state = [this](int client_id, bool connected, const std::string& detail) {
        log("IPC %s", detail.c_str());
        if (!connected) {
            auto* m = new IpcCmdMsg{ client_id, 2, "" };
            PostMessageW(p_->msg_wnd, WM_IPC_CMD, 0, (LPARAM)m);
        }
    };
    ev.on_log = [this](const std::string& line) { log("%s", line.c_str()); };
    if (!p_->ipc.start(ev))
        log("IPC 服务启动失败（管道被占用？），等待 TSF 触发将不可用");
#endif

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

#ifdef VOIME_NOTSF
    log("初始化完成（NOTSF 模式），PTT=%s", wide_to_utf8(p_->cfg.hotkey).c_str());
#else
    log("初始化完成（TSF 模式，等待 IPC 触发），PTT=%s（由 TSF DLL 拦截）",
        wide_to_utf8(p_->cfg.hotkey).c_str());
#endif
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
#ifdef VOIME_NOTSF
    if (msg == Hotkey::WM_PRESS) { trigger_start(); return 0; }
    if (msg == Hotkey::WM_RELEASE) { trigger_stop(); return 0; }
#else
    if (msg == WM_IPC_CMD) { // P5-04：TSF 指令 -> 统一触发入口 -> IPC 应答
        auto* m = (IpcCmdMsg*)lp;
        if (m->kind == 1) {
            int rc = trigger_start();
            p_->ipc.send(m->client_id, ipc::make_response(m->request_id, rc == 0, rc));
            if (rc == 0) p_->session_client = m->client_id;
            log("IPC start_session client=%d rc=%d", m->client_id, rc);
        } else if (m->kind == 0) {
            int rc = trigger_stop();
            p_->ipc.send(m->client_id, ipc::make_response(m->request_id, rc == 0, rc));
            log("IPC stop_session client=%d rc=%d", m->client_id, rc);
        } else { // kind==2：客户端断开
            if (p_->session_client == m->client_id) p_->session_client = -1;
        }
        delete m;
        return 0;
    }
#endif
    if (msg == WM_TIMER) {
#ifdef VOIME_NOTSF
        if (wp == kWatchdogTimer) p_->hotkey.watchdog_tick();
        else
#endif
        if (wp == kHideTimer) { KillTimer(p_->msg_wnd, kHideTimer); p_->overlay.hide(); }
        return 0;
    }
    if (msg == WM_CAPTURE_ERROR) { on_capture_error(); return 0; }
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

int App::trigger_start() {
    if (p_->recording) return -1;
    if (!p_->deps_ok) {
        p_->overlay.show(L"音频组件缺失，语音录入已禁用");
        SetTimer(p_->msg_wnd, kHideTimer, 1200, nullptr);
        return -5;
    }
    int rc = p_->engine.start();
    if (rc == -3) {
        p_->overlay.show(L"正在收尾，请稍候...");
        SetTimer(p_->msg_wnd, kHideTimer, 900, nullptr);
        return -3;
    }
    if (rc == -4) {
        p_->overlay.show(L"模型未加载，无法识别");
        SetTimer(p_->msg_wnd, kHideTimer, 1200, nullptr);
        return -4;
    }
    if (rc != 0) {
        log("start_session rc=%d", rc);
        return rc;
    }
    // 启动音源（麦克风或 WAV；FFmpeg 重采样在采集线程内完成）
    std::string err;
    if (!start_audio(&err)) {
        log("音源启动失败: %s", err.c_str());
        p_->engine.stop();
        p_->overlay.show(L"无可用采集设备");
        SetTimer(p_->msg_wnd, kHideTimer, 1200, nullptr);
        return -6;
    }
    p_->recording = true;
    p_->session_text.clear();
    KillTimer(p_->msg_wnd, kHideTimer);
    p_->overlay.show(L"正在听...");
    log("会话开始（%s）",
        p_->wav_mode ? p_->wav_source.device_format().c_str()
                     : p_->capture.device_format().c_str());
    return 0;
}

bool App::start_audio(std::string* err) {
    auto on_pcm = [this](const int16_t* d, size_t n) {
        int prc = p_->engine.push(d, (uint32_t)n);
        if (prc == -2) log("队列溢出丢帧(-2)");
    };
    auto on_err = [this](const std::string& e) {
        p_->last_capture_err = e;
        PostMessageW(p_->msg_wnd, WM_CAPTURE_ERROR, 0, 0);
    };
    if (!p_->opts.audio_wav.empty()) {
        p_->wav_mode = true;
        return p_->wav_source.start(on_pcm, on_err, err);
    }
    p_->wav_mode = false;
    return p_->capture.start(on_pcm, on_err, err);
}

void App::stop_audio() {
    if (p_->wav_mode) p_->wav_source.stop();
    else p_->capture.stop();
}

void App::on_capture_error() {
    // 设备拔出/采集中断（QA-02）：停止会话，提示用户，不崩溃
    log("采集中断: %s", p_->last_capture_err.c_str());
    if (p_->recording) {
        p_->recording = false;
        stop_audio();
        p_->engine.stop(); // 尽力收尾（Finalizing 结果经事件回报）
        p_->overlay.set_status(L"采集设备中断");
        SetTimer(p_->msg_wnd, kHideTimer, 1500, nullptr);
    }
}

int App::trigger_stop() {
    if (!p_->recording) return -1;
    p_->recording = false;
    stop_audio();
    p_->engine.stop(); // 异步 Finalizing，结果经事件回报
    p_->overlay.set_status(L"识别中...");
    log("会话结束，进入 Finalizing");
    return 0;
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
        if (p_->recording) { // 60s 护栏自动收尾：停止音源
            p_->recording = false;
            stop_audio();
        }
        p_->overlay.set_status(L"完成");
        p_->overlay.set_text(text);
        if (!text.empty()) {
#ifdef VOIME_NOTSF
            injector::inject_text(text); // CPP-08
            log("已注入 %zu 字符", text.size());
#else
            // P5-05：标准模式经 IPC 提交给发起会话的 TSF 客户端注入
            if (p_->session_client >= 0) {
                std::string rid = ipc::new_request_id();
                if (p_->ipc.send(p_->session_client,
                                 ipc::make_commit_text(wide_to_utf8(text), rid)))
                    log("commit_text 已发送 client=%d（%zu 字符）", p_->session_client,
                        text.size());
                else
                    log("commit_text 发送失败 client=%d", p_->session_client);
            } else {
                log("无 TSF 客户端连接，commit_text 丢弃");
            }
#endif
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
#ifdef VOIME_NOTSF
    p_->hotkey.uninstall();
#else
    p_->ipc.stop();
#endif
    p_->capture.stop();
    p_->wav_source.stop();
    p_->overlay.destroy();
    if (p_->engine_event) CloseHandle(p_->engine_event);
    p_->engine.destroy();
    if (p_->msg_wnd) DestroyWindow(p_->msg_wnd);
    if (p_->log_fp) fclose(p_->log_fp);
    CoUninitialize();
}
