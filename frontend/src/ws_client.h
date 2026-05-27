#pragma once

#include <windows.h>
#include <winhttp.h>
#include <string>
#include <functional>
#include <thread>
#include <atomic>
#include <queue>
#include <mutex>
#include <condition_variable>

class WebSocketClient {
public:
    using MessageCallback = std::function<void(const std::string& message)>;
    using StatusCallback = std::function<void(const std::string& status)>;

    WebSocketClient();
    ~WebSocketClient();

    bool Connect(const std::wstring& url, MessageCallback onMessage, StatusCallback onStatus);
    void Disconnect();
    bool IsConnected() const { return m_connected.load(); }

    bool SendText(const std::string& text);
    bool SendBinary(const uint8_t* data, size_t size);

private:
    void ReceiveLoop();

    HINTERNET m_session = nullptr;
    HINTERNET m_connection = nullptr;
    HINTERNET m_socket = nullptr;

    MessageCallback m_onMessage;
    StatusCallback m_onStatus;

    std::thread m_receiveThread;
    std::atomic<bool> m_connected{false};
    std::atomic<bool> m_stopRequested{false};

    // Send queue for thread safety
    std::mutex m_sendMutex;
};
