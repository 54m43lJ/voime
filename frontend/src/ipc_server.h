// ipc_server.h — Agent 侧 Named Pipe 服务端（P5-03，PRD 2.4/3.2）
//
// 单管道名多实例：accept 线程循环建实例，每客户端独占一个实例 + 一个 reader 线程；
// 写（指令应答 + commit_text）经 per-client 锁序列化。
// 安全：SDDL 仅允许当前用户 SID + SYSTEM 连接，拒绝网络/匿名（PRD 通信安全隔离）。
// 仅在标准模式（非 VOIME_NOTSF）参与编译。
#pragma once
#include <functional>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <windows.h>

class IpcServer {
public:
    struct Events {
        // 收到一条完整 JSON 消息（reader 线程上下文；接收方须自行转投主线程）
        std::function<void(int client_id, const std::string& json)> on_message;
        // 客户端连接/断开（accept/reader 线程上下文）
        std::function<void(int client_id, bool connected, const std::string& detail)> on_client_state;
        std::function<void(const std::string& line)> on_log;
    };

    bool start(const Events& ev); // 创建 ACL 受限管道 + accept 线程
    void stop();

    // 线程安全写整条 JSON 消息；客户端不存在/写失败返回 false
    bool send(int client_id, const std::string& json);
    bool has_client(int client_id) const;

private:
    struct Client {
        int id = 0;
        HANDLE pipe = nullptr;   // 受 write_mu 保护（关闭前持锁）
        HANDLE thread = nullptr;
        std::mutex write_mu;
    };
    void accept_loop();
    void client_loop(const std::shared_ptr<Client>& c);
    void log(const char* fmt, ...);

    static DWORD WINAPI accept_trampoline(LPVOID p);
    struct ThreadArg { IpcServer* self; std::shared_ptr<Client> c; };
    static DWORD WINAPI client_trampoline(LPVOID p);

    Events ev_;
    HANDLE stop_event_ = nullptr;
    HANDLE accept_thread_ = nullptr;
    mutable std::mutex mu_;
    std::unordered_map<int, std::shared_ptr<Client>> clients_;
    int next_id_ = 1;
};
