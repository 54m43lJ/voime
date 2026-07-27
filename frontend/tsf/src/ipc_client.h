// ipc_client.h — TSF 侧 Named Pipe 客户端（P5-02）
//
// 单线程管理：连接（2s 重试）-> 读循环 -> 断线自动重连。
// 写（start/stop_session、应答）经锁序列化；写失败打断读循环触发重连。
#pragma once
#include <functional>
#include <mutex>
#include <string>
#include <windows.h>

class IpcClient {
public:
    struct Events {
        // 收到 commit_text（reader 线程上下文；payload 为 UTF-8）
        std::function<void(const std::string& payload_utf8,
                           const std::string& request_id)> on_commit_text;
        std::function<void(bool connected)> on_state;
        std::function<void(const std::string& line)> on_log;
    };

    bool start(const Events& ev);
    void stop();

    bool send(const char* action); // "start_session" / "stop_session"（自动生成 request_id）
    void respond(const std::string& request_id, bool ok, int code);
    bool connected() const { return connected_; }

private:
    void run();
    void write_json(const std::string& json);
    void log(const char* fmt, ...);

    static DWORD WINAPI trampoline(LPVOID p);

    Events ev_;
    HANDLE thread_ = nullptr;
    HANDLE stop_event_ = nullptr;
    HANDLE reconnect_event_ = nullptr; // 写失败 -> 打断读等待触发重连
    HANDLE pipe_ = nullptr;            // 受 write_mu_ 保护
    std::mutex write_mu_;
    volatile bool stop_ = false;
    volatile bool connected_ = false;
};
