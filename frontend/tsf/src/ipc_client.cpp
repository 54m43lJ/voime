// ipc_client.cpp — TSF 侧 Named Pipe 客户端实现
//
// 重叠 I/O 模型（关键设计）：
//   句柄以 FILE_FLAG_OVERLAPPED 打开 —— 同步句柄会在“读阻塞时写”死锁
//  （单句柄同时只允许一个 I/O，本 bug 已在 qa05 实测复现）。
//   读：ReadFile + OVERLAPPED，等 stop/reconnect/read 三事件；
//   写：WriteFile + 独立 OVERLAPPED，GetOverlappedResult 等完成；
//   写失败 -> 置 reconnect 事件打断读等待 -> 走重连流程。
#include "ipc_client.h"
#include "ipc_proto.h" // frontend/src（构建时 -I 引入）
#include "tsf_defs.h"

#include <cstdarg>
#include <cstdio>

void IpcClient::log(const char* fmt, ...) {
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    if (ev_.on_log) ev_.on_log(buf);
}

DWORD WINAPI IpcClient::trampoline(LPVOID p) {
    ((IpcClient*)p)->run();
    return 0;
}

bool IpcClient::start(const Events& ev) {
    if (thread_) return true;
    ev_ = ev;
    stop_ = false;
    stop_event_ = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    reconnect_event_ = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    thread_ = CreateThread(nullptr, 0, &IpcClient::trampoline, this, 0, nullptr);
    if (!thread_) {
        CloseHandle(stop_event_);
        CloseHandle(reconnect_event_);
        stop_event_ = reconnect_event_ = nullptr;
        return false;
    }
    return true;
}

void IpcClient::run() {
    std::string msg;
    char buf[4096];
    HANDLE read_ev = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    while (!stop_) {
        HANDLE p = CreateFileW(ipc::kPipeName, GENERIC_READ | GENERIC_WRITE, 0, nullptr,
                               OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
        if (p == INVALID_HANDLE_VALUE) {
            if (WaitForSingleObject(stop_event_, 2000) == WAIT_OBJECT_0) break;
            continue; // Agent 未启动：2s 后重试
        }
        DWORD mode = PIPE_READMODE_MESSAGE;
        SetNamedPipeHandleState(p, &mode, nullptr, nullptr);
        ResetEvent(reconnect_event_);
        {
            std::lock_guard<std::mutex> lk(write_mu_);
            pipe_ = p;
            connected_ = true;
        }
        log("已连接 Agent 管道");
        if (ev_.on_state) ev_.on_state(true);

        msg.clear();
        for (;;) {
            OVERLAPPED ov{};
            ov.hEvent = read_ev;
            ResetEvent(read_ev);
            DWORD n = 0;
            BOOL ok = ReadFile(p, buf, sizeof(buf), &n, &ov);
            if (!ok) {
                DWORD e = GetLastError();
                if (e == ERROR_IO_PENDING) {
                    HANDLE waits[3] = { stop_event_, reconnect_event_, read_ev };
                    DWORD r = WaitForMultipleObjects(3, waits, FALSE, INFINITE);
                    if (r == WAIT_OBJECT_0) { // stop
                        CancelIoEx(p, &ov);
                        WaitForSingleObject(read_ev, 1000);
                        break;
                    }
                    if (r == WAIT_OBJECT_0 + 1) { // 写失败要求重连
                        CancelIoEx(p, &ov);
                        WaitForSingleObject(read_ev, 1000);
                        break;
                    }
                    if (!GetOverlappedResult(p, &ov, &n, FALSE)) {
                        e = GetLastError();
                        if (e == ERROR_MORE_DATA) { msg.append(buf, n); continue; }
                        break;
                    }
                } else if (e == ERROR_MORE_DATA) {
                    msg.append(buf, n);
                    continue;
                } else {
                    break;
                }
            }
            msg.append(buf, n); // 消息模式：成功完成 = 一条完整消息
            std::string action = json_get_string(msg, "action");
            if (action == "commit_text") {
                log("dispatch commit_text (%zu 字节)", msg.size());
                if (ev_.on_commit_text)
                    ev_.on_commit_text(json_get_string(msg, "payload"),
                                       json_get_string(msg, "request_id"));
            } else {
                log("收到: %s", msg.c_str()); // start/stop 的应答
            }
            msg.clear();
        }

        {
            std::lock_guard<std::mutex> lk(write_mu_);
            pipe_ = nullptr;
            connected_ = false;
        }
        CloseHandle(p);
        if (ev_.on_state) ev_.on_state(false);
        if (!stop_) log("连接断开，2s 后自动重连");
        if (WaitForSingleObject(stop_event_, 2000) == WAIT_OBJECT_0) break;
    }
    CloseHandle(read_ev);
}

void IpcClient::write_json(const std::string& json) {
    std::lock_guard<std::mutex> lk(write_mu_);
    if (!pipe_) return;
    HANDLE wev = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    OVERLAPPED ov{};
    ov.hEvent = wev;
    DWORD n = 0;
    BOOL ok = WriteFile(pipe_, json.data(), (DWORD)json.size(), &n, &ov);
    DWORD e = GetLastError();
    if (!ok && e == ERROR_IO_PENDING) {
        ok = GetOverlappedResult(pipe_, &ov, &n, TRUE); // 等本次写完成
        e = GetLastError();
    }
    CloseHandle(wev);
    if (!ok || n != json.size()) {
        log("写管道失败 gle=%lu", e);
        SetEvent(reconnect_event_); // 打断读等待 -> 重连
    }
}

bool IpcClient::send(const char* action) {
    if (!connected_) {
        log("IPC 未连接，丢弃 %s", action);
        return false;
    }
    write_json(ipc::make_session_cmd(action, ipc::new_request_id()));
    return true;
}

void IpcClient::respond(const std::string& request_id, bool ok, int code) {
    write_json(ipc::make_response(request_id, ok, code));
}

void IpcClient::stop() {
    if (!thread_) return;
    stop_ = true;
    SetEvent(stop_event_);
    WaitForSingleObject(thread_, 3000);
    CloseHandle(thread_);
    thread_ = nullptr;
    {
        std::lock_guard<std::mutex> lk(write_mu_);
        if (pipe_) CloseHandle(pipe_);
        pipe_ = nullptr;
        connected_ = false;
    }
    CloseHandle(stop_event_);
    CloseHandle(reconnect_event_);
    stop_event_ = reconnect_event_ = nullptr;
}
