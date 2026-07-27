// ipc_server.cpp — Agent 侧 Named Pipe 服务端实现
#ifndef VOIME_NOTSF

#include "ipc_server.h"
#include "ipc_proto.h"

#include <sddl.h>
#include <cstdarg>
#include <cstdio>
#include <vector>

void IpcServer::log(const char* fmt, ...) {
    if (!ev_.on_log) return;
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    ev_.on_log(buf);
}

// 构建仅允许当前用户 + SYSTEM 的 SD（拒绝 NETWORK/ANONYMOUS）
static PSECURITY_DESCRIPTOR build_pipe_sd() {
    HANDLE tok = nullptr;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &tok)) return nullptr;
    DWORD need = 0;
    GetTokenInformation(tok, TokenUser, nullptr, 0, &need);
    std::vector<char> buf(need);
    BOOL ok = GetTokenInformation(tok, TokenUser, buf.data(), need, &need);
    CloseHandle(tok);
    if (!ok) return nullptr;
    PSID sid = ((TOKEN_USER*)buf.data())->User.Sid;
    LPWSTR sid_str = nullptr;
    if (!ConvertSidToStringSidW(sid, &sid_str)) return nullptr;
    std::wstring sddl = L"D:P(D;;GA;;;NU)(D;;GA;;;AN)(A;;GA;;;";
    sddl += sid_str;
    sddl += L")(A;;GA;;;SY)";
    LocalFree(sid_str);
    PSECURITY_DESCRIPTOR sd = nullptr;
    if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.c_str(), SDDL_REVISION_1, &sd, nullptr))
        return nullptr;
    return sd;
}

bool IpcServer::start(const Events& ev) {
    if (accept_thread_) return true;
    ev_ = ev;
    stop_event_ = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    accept_thread_ = CreateThread(nullptr, 0, &IpcServer::accept_trampoline, this, 0, nullptr);
    if (!accept_thread_) {
        CloseHandle(stop_event_);
        stop_event_ = nullptr;
        return false;
    }
    log("IPC 服务已监听 %ws", ipc::kPipeName);
    return true;
}

DWORD WINAPI IpcServer::accept_trampoline(LPVOID p) {
    ((IpcServer*)p)->accept_loop();
    return 0;
}

DWORD WINAPI IpcServer::client_trampoline(LPVOID p) {
    ThreadArg* a = (ThreadArg*)p;
    a->self->client_loop(a->c);
    delete a;
    return 0;
}

void IpcServer::accept_loop() {
    SECURITY_ATTRIBUTES sa{ sizeof(sa) };
    sa.lpSecurityDescriptor = build_pipe_sd();
    if (!sa.lpSecurityDescriptor) {
        log("管道 ACL 构建失败 gle=%lu，服务终止", GetLastError());
        return; // fail closed：无 ACL 不提供服务
    }

    for (;;) {
        HANDLE pipe = CreateNamedPipeW(ipc::kPipeName,
                                       PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                                       PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                                       PIPE_UNLIMITED_INSTANCES,
                                       ipc::kPipeBuf, ipc::kPipeBuf, 0, &sa);
        if (pipe == INVALID_HANDLE_VALUE) {
            log("CreateNamedPipe 失败 gle=%lu", GetLastError());
            if (WaitForSingleObject(stop_event_, 1000) == WAIT_OBJECT_0) break;
            continue;
        }

        OVERLAPPED ov{};
        ov.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        BOOL connected = FALSE;
        BOOL ok = ConnectNamedPipe(pipe, &ov);
        DWORD err = GetLastError();
        if (ok || err == ERROR_PIPE_CONNECTED) {
            connected = TRUE;
        } else if (err == ERROR_IO_PENDING) {
            HANDLE waits[2] = { stop_event_, ov.hEvent };
            DWORD r = WaitForMultipleObjects(2, waits, FALSE, INFINITE);
            if (r == WAIT_OBJECT_0) {
                CancelIo(pipe);
                CloseHandle(ov.hEvent);
                CloseHandle(pipe);
                break;
            }
            DWORD n = 0;
            connected = GetOverlappedResult(pipe, &ov, &n, FALSE);
        } else {
            log("ConnectNamedPipe gle=%lu", err);
        }
        CloseHandle(ov.hEvent);

        if (!connected) {
            CloseHandle(pipe);
            if (WaitForSingleObject(stop_event_, 0) == WAIT_OBJECT_0) break;
            continue;
        }

        auto c = std::make_shared<Client>();
        c->pipe = pipe;
        {
            std::lock_guard<std::mutex> lk(mu_);
            c->id = next_id_++;
            clients_[c->id] = c;
        }
        ULONG peer_pid = 0;
        GetNamedPipeClientProcessId(pipe, &peer_pid);
        char detail[96];
        snprintf(detail, sizeof(detail), "Client Connected (id=%d pid=%lu)", c->id, peer_pid);
        if (ev_.on_client_state) ev_.on_client_state(c->id, true, detail);

        auto* arg = new ThreadArg{ this, c };
        c->thread = CreateThread(nullptr, 0, &IpcServer::client_trampoline, arg, 0, nullptr);
        if (!c->thread) {
            delete arg;
            std::lock_guard<std::mutex> lk(mu_);
            clients_.erase(c->id);
            CloseHandle(pipe);
        }
    }
    LocalFree(sa.lpSecurityDescriptor);
}

void IpcServer::client_loop(const std::shared_ptr<Client>& c) {
    std::string msg;
    char buf[4096];
    HANDLE read_ev = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    for (;;) {
        OVERLAPPED ov{};
        ov.hEvent = read_ev;
        ResetEvent(read_ev);
        DWORD n = 0;
        BOOL ok = ReadFile(c->pipe, buf, sizeof(buf), &n, &ov);
        if (!ok) {
            DWORD e = GetLastError();
            if (e == ERROR_IO_PENDING) {
                HANDLE waits[2] = { stop_event_, read_ev };
                DWORD r = WaitForMultipleObjects(2, waits, FALSE, INFINITE);
                if (r == WAIT_OBJECT_0) { // 服务停止
                    CancelIoEx(c->pipe, &ov);
                    WaitForSingleObject(read_ev, 1000);
                    break;
                }
                if (!GetOverlappedResult(c->pipe, &ov, &n, FALSE)) {
                    e = GetLastError();
                    if (e == ERROR_MORE_DATA) { msg.append(buf, n); continue; }
                    break; // 断开
                }
            } else if (e == ERROR_MORE_DATA) {
                msg.append(buf, n);
                continue;
            } else {
                break;
            }
        }
        msg.append(buf, n); // 消息模式：成功完成 = 一条完整消息
        if (ev_.on_message) ev_.on_message(c->id, msg);
        msg.clear();
    }
    CloseHandle(read_ev);

    {
        std::lock_guard<std::mutex> lk(mu_);
        clients_.erase(c->id);
    }
    if (ev_.on_client_state) ev_.on_client_state(c->id, false, "Client Disconnected");
    {
        std::lock_guard<std::mutex> lk(c->write_mu); // 与 send 互斥后再关句柄
        CloseHandle(c->pipe);
        c->pipe = nullptr;
    }
}

bool IpcServer::send(int client_id, const std::string& json) {
    std::shared_ptr<Client> c;
    {
        std::lock_guard<std::mutex> lk(mu_);
        auto it = clients_.find(client_id);
        if (it == clients_.end()) return false;
        c = it->second;
    }
    std::lock_guard<std::mutex> lk(c->write_mu);
    if (!c->pipe) return false;
    HANDLE wev = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    OVERLAPPED ov{};
    ov.hEvent = wev;
    DWORD n = 0;
    BOOL ok = WriteFile(c->pipe, json.data(), (DWORD)json.size(), &n, &ov);
    DWORD e = GetLastError();
    if (!ok && e == ERROR_IO_PENDING) {
        ok = GetOverlappedResult(c->pipe, &ov, &n, TRUE); // 等本次写完成
        e = GetLastError();
    }
    CloseHandle(wev);
    if (!ok || n != json.size()) {
        log("写管道失败 client=%d gle=%lu", client_id, e);
        return false;
    }
    return true;
}

bool IpcServer::has_client(int client_id) const {
    std::lock_guard<std::mutex> lk(mu_);
    return clients_.find(client_id) != clients_.end();
}

void IpcServer::stop() {
    if (!stop_event_) return;
    SetEvent(stop_event_); // accept 与所有 client 读等待均监听此事件
    if (accept_thread_) {
        WaitForSingleObject(accept_thread_, 3000);
        CloseHandle(accept_thread_);
        accept_thread_ = nullptr;
    }
    std::vector<std::shared_ptr<Client>> snap;
    {
        std::lock_guard<std::mutex> lk(mu_);
        for (auto& kv : clients_) snap.push_back(kv.second);
    }
    for (auto& c : snap) {
        if (c->thread) {
            WaitForSingleObject(c->thread, 2000);
            CloseHandle(c->thread);
            c->thread = nullptr;
        }
    }
    {
        std::lock_guard<std::mutex> lk(mu_);
        clients_.clear();
    }
    CloseHandle(stop_event_);
    stop_event_ = nullptr;
}

#endif // VOIME_NOTSF
