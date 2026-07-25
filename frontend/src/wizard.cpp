// wizard.cpp — 首启模型下载向导实现
#include "wizard.h"
#include "models.h"
#include "util.h"

#include <atomic>
#include <commctrl.h>
#include <thread>

static const wchar_t* kWizClass = L"VoimeWizardWnd";
static const UINT WM_PROGRESS = WM_APP + 20; // wParam=千分比, lParam=当前文件序号
static const UINT WM_DONE = WM_APP + 21;     // wParam: 1=成功 0=失败
static const int kBtnStart = 100;
static const int kBtnExit = 101;
static const int kBtnCancel = 102;
static const int kBtnRetry = 103;

struct WizState {
    std::string base_url;
    std::wstring dir;
    bool auto_start = false;
    bool had_error = false;
    wizard::Result result = wizard::Cancelled;
    std::atomic<bool> cancel{false};
    std::thread worker;
    std::string err;
    char file_name[128]{};
    HWND status = nullptr, bar = nullptr, btn1 = nullptr, btn2 = nullptr;
};

// 唤醒向导模态循环（不能用 PostQuitMessage：会污染调用方 App 的消息循环）
static void wake_loop() {
    PostThreadMessageW(GetCurrentThreadId(), WM_NULL, 0, 0);
}

static void set_buttons(WizState* st, HWND hwnd, int mode) {
    // mode: 0=提示(开始/退出) 1=下载中(取消) 2=失败(重试/退出)
    DestroyWindow(st->btn1); st->btn1 = nullptr;
    DestroyWindow(st->btn2); st->btn2 = nullptr;
    if (mode == 0) {
        st->btn1 = CreateWindowExW(0, L"BUTTON", L"开始下载", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                                   130, 150, 100, 28, hwnd, (HMENU)kBtnStart, nullptr, nullptr);
        st->btn2 = CreateWindowExW(0, L"BUTTON", L"退出", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                                   250, 150, 100, 28, hwnd, (HMENU)kBtnExit, nullptr, nullptr);
    } else if (mode == 1) {
        st->btn1 = CreateWindowExW(0, L"BUTTON", L"取消", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                                   190, 150, 100, 28, hwnd, (HMENU)kBtnCancel, nullptr, nullptr);
    } else {
        st->btn1 = CreateWindowExW(0, L"BUTTON", L"重试", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                                   130, 150, 100, 28, hwnd, (HMENU)kBtnRetry, nullptr, nullptr);
        st->btn2 = CreateWindowExW(0, L"BUTTON", L"退出", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
                                   250, 150, 100, 28, hwnd, (HMENU)kBtnExit, nullptr, nullptr);
    }
}

static void start_download(WizState* st, HWND hwnd) {
    st->cancel = false;
    st->err.clear();
    set_buttons(st, hwnd, 1);
    SetWindowTextW(st->status, L"正在连接...");
    st->worker = std::thread([st, hwnd]() {
        std::string err;
        bool ok = models::download(
            st->base_url, st->dir,
            [hwnd](uint64_t done, uint64_t total, const std::string& file) {
                WPARAM permille = total ? (WPARAM)(done * 1000 / total) : 0;
                LPARAM idx = (LPARAM)0;
                // 当前文件名经静态缓冲传递（UI 线程读取）
                auto* s = (WizState*)GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if (s) {
                    strncpy_s(s->file_name, file.c_str(), sizeof(s->file_name) - 1);
                }
                PostMessageW(hwnd, WM_PROGRESS, permille, idx);
            },
            st->cancel, err);
        st->err = err;
        PostMessageW(hwnd, WM_DONE, ok ? 1 : 0, 0);
    });
}

static LRESULT CALLBACK wiz_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    WizState* st = (WizState*)GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    switch (msg) {
    case WM_COMMAND: {
        if (!st) break;
        int id = LOWORD(wp);
        if (id == kBtnStart || id == kBtnRetry) {
            if (!st->worker.joinable()) start_download(st, hwnd);
        } else if (id == kBtnExit) {
            st->cancel = true;
            if (st->worker.joinable()) st->worker.join();
            st->result = st->had_error ? wizard::Failed : wizard::Cancelled;
            DestroyWindow(hwnd);
            wake_loop();
        } else if (id == kBtnCancel) {
            st->cancel = true; // 等 worker 退出后 WM_DONE 收尾
            EnableWindow(st->btn1, FALSE);
            SetWindowTextW(st->status, L"正在取消...");
        }
        return 0;
    }
    case WM_PROGRESS: {
        if (!st) break;
        SendMessageW(st->bar, PBM_SETPOS, wp, 0);
        wchar_t buf[256];
        swprintf(buf, 256, L"正在下载 (%llu‰): %hs", (unsigned long long)wp, st->file_name);
        SetWindowTextW(st->status, buf);
        return 0;
    }
    case WM_DONE: {
        if (!st) break;
        if (st->worker.joinable()) st->worker.join();
        if (wp == 1) {
            SetWindowTextW(st->status, L"下载完成，正在加载模型...");
            st->result = wizard::Ok;
            DestroyWindow(hwnd);
            wake_loop();
        } else {
            if (st->cancel) {
                st->result = wizard::Cancelled;
                DestroyWindow(hwnd);
                wake_loop();
            } else {
                st->had_error = true;
                SetWindowTextW(st->status, utf8_to_wide("失败: " + st->err).c_str());
                set_buttons(st, hwnd, 2);
            }
        }
        return 0;
    }
    case WM_CLOSE:
        if (st) {
            st->cancel = true;
            if (st->worker.joinable()) st->worker.join();
            st->result = st->had_error ? wizard::Failed : wizard::Cancelled;
        }
        DestroyWindow(hwnd);
        wake_loop();
        return 0;
    }
    return DefWindowProcW(hwnd, msg, wp, lp);
}

wizard::Result wizard::run(HINSTANCE inst, const std::string& base_url, const std::wstring& dir,
                           bool auto_start) {
    // 已完整则直接返回（不弹窗）
    if (models::check(dir).empty()) return Ok;

    WizState st;
    st.base_url = base_url;
    st.dir = dir;
    st.auto_start = auto_start;

    WNDCLASSEXW wc{ sizeof(wc) };
    wc.lpfnWndProc = wiz_proc;
    wc.hInstance = inst;
    wc.lpszClassName = kWizClass;
    wc.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    RegisterClassExW(&wc);

    RECT rc{ 0, 0, 480, 220 };
    AdjustWindowRect(&rc, WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU, FALSE);
    HWND hwnd = CreateWindowExW(0, kWizClass, L"Voime 模型下载",
                                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                                CW_USEDEFAULT, CW_USEDEFAULT, rc.right - rc.left,
                                rc.bottom - rc.top, nullptr, nullptr, inst, nullptr);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, (LONG_PTR)&st);

    uint64_t total_mb = models::manifest_total_bytes() / 1048576;
    wchar_t hint[256];
    swprintf(hint, 256, L"首次运行需要下载语音识别模型（约 %llu MB）。", (unsigned long long)total_mb);
    CreateWindowExW(0, L"STATIC", hint, WS_CHILD | WS_VISIBLE,
                    20, 20, 440, 22, hwnd, nullptr, nullptr, nullptr);
    st.status = CreateWindowExW(0, L"STATIC", L"模型文件缺失，尚未下载。", WS_CHILD | WS_VISIBLE,
                                20, 60, 440, 22, hwnd, nullptr, nullptr, nullptr);
    st.bar = CreateWindowExW(0, PROGRESS_CLASSW, nullptr, WS_CHILD | WS_VISIBLE,
                             20, 100, 440, 20, hwnd, nullptr, nullptr, nullptr);
    SendMessageW(st.bar, PBM_SETRANGE, 0, MAKELPARAM(0, 1000));
    set_buttons(&st, hwnd, 0);

    if (auto_start) PostMessageW(hwnd, WM_COMMAND, kBtnStart, 0);

    // 模态消息循环
    MSG msg;
    while (IsWindow(hwnd) && GetMessageW(&msg, nullptr, 0, 0) > 0) {
        if (msg.hwnd == hwnd || IsChild(hwnd, msg.hwnd)) {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    if (st.worker.joinable()) st.worker.join();
    return st.result == Ok ? Ok : (st.result == Failed ? Failed : st.result);
}
