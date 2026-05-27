#include "ws_client.h"
#include <stdio.h>
#include <vector>

#pragma comment(lib, "winhttp.lib")

WebSocketClient::WebSocketClient() = default;

WebSocketClient::~WebSocketClient() {
    Disconnect();
}

static std::wstring ParseHost(const std::wstring& url) {
    // ws://host:port/path -> host:port
    size_t start = url.find(L"://");
    if (start == std::wstring::npos) return url;
    start += 3;
    size_t end = url.find(L'/', start);
    if (end == std::wstring::npos) return url.substr(start);
    return url.substr(start, end - start);
}

static std::wstring ParsePath(const std::wstring& url) {
    size_t start = url.find(L"://");
    if (start == std::wstring::npos) return L"/";
    start += 3;
    size_t pathStart = url.find(L'/', start);
    if (pathStart == std::wstring::npos) return L"/";
    return url.substr(pathStart);
}

bool WebSocketClient::Connect(const std::wstring& url, MessageCallback onMessage, StatusCallback onStatus) {
    m_onMessage = std::move(onMessage);
    m_onStatus = std::move(onStatus);

    std::wstring host = ParseHost(url);
    std::wstring path = ParsePath(url);

    m_session = WinHttpOpen(L"Voime/1.0",
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
        WINHTTP_NO_PROXY_NAME, WINHTTP_NO_PROXY_BYPASS, 0);
    if (!m_session) return false;

    m_connection = WinHttpConnect(m_session, host.c_str(),
        INTERNET_DEFAULT_HTTP_PORT, 0);
    if (!m_connection) {
        WinHttpCloseHandle(m_session);
        m_session = nullptr;
        return false;
    }

    HINTERNET hRequest = WinHttpOpenRequest(m_connection, L"GET", path.c_str(),
        nullptr, WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, 0);
    if (!hRequest) return false;

    // Request WebSocket upgrade
    DWORD statusCode = 0;
    if (!WinHttpSetOption(hRequest, WINHTTP_OPTION_WEB_SOCKET_CLOSE_TIMEOUT, nullptr, 0)) {
        WinHttpCloseHandle(hRequest);
        return false;
    }

    if (!WinHttpSendRequest(hRequest, WINHTTP_NO_ADDITIONAL_HEADERS, 0,
            WINHTTP_NO_REQUEST_DATA, 0, 0, 0)) {
        WinHttpCloseHandle(hRequest);
        return false;
    }

    if (!WinHttpReceiveResponse(hRequest, nullptr)) {
        WinHttpCloseHandle(hRequest);
        return false;
    }

    m_socket = WinHttpWebSocketCompleteUpgrade(hRequest, 0);
    WinHttpCloseHandle(hRequest);

    if (!m_socket) {
        return false;
    }

    m_connected.store(true);
    m_stopRequested.store(false);

    if (m_onStatus) m_onStatus("connected");

    m_receiveThread = std::thread(&WebSocketClient::ReceiveLoop, this);
    return true;
}

void WebSocketClient::Disconnect() {
    m_stopRequested.store(true);

    if (m_socket) {
        WinHttpWebSocketClose(m_socket,
            WINHTTP_WEB_SOCKET_SUCCESS_CLOSE_STATUS, nullptr, 0);
    }

    if (m_receiveThread.joinable()) {
        m_receiveThread.join();
    }

    if (m_socket) {
        WinHttpCloseHandle(m_socket);
        m_socket = nullptr;
    }
    if (m_connection) {
        WinHttpCloseHandle(m_connection);
        m_connection = nullptr;
    }
    if (m_session) {
        WinHttpCloseHandle(m_session);
        m_session = nullptr;
    }

    m_connected.store(false);
}

bool WebSocketClient::SendText(const std::string& text) {
    if (!m_socket || !m_connected.load()) return false;

    std::lock_guard<std::mutex> lock(m_sendMutex);
    DWORD err = WinHttpWebSocketSend(m_socket,
        WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE,
        (PVOID)text.data(), (DWORD)text.size());
    return err == ERROR_SUCCESS;
}

bool WebSocketClient::SendBinary(const uint8_t* data, size_t size) {
    if (!m_socket || !m_connected.load()) return false;

    std::lock_guard<std::mutex> lock(m_sendMutex);
    DWORD err = WinHttpWebSocketSend(m_socket,
        WINHTTP_WEB_SOCKET_BINARY_MESSAGE_BUFFER_TYPE,
        (PVOID)data, (DWORD)size);
    return err == ERROR_SUCCESS;
}

void WebSocketClient::ReceiveLoop() {
    std::vector<BYTE> buffer(65536);

    while (!m_stopRequested.load()) {
        DWORD bytesRead = 0;
        WINHTTP_WEB_SOCKET_BUFFER_TYPE bufferType;

        DWORD err = WinHttpWebSocketReceive(m_socket,
            buffer.data(), (DWORD)buffer.size(),
            &bytesRead, &bufferType);

        if (err != ERROR_SUCCESS) {
            if (m_onStatus) m_onStatus("disconnected");
            m_connected.store(false);
            break;
        }

        if (bufferType == WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE && m_onMessage) {
            std::string msg(reinterpret_cast<char*>(buffer.data()), bytesRead);
            m_onMessage(msg);
        }

        if (bufferType == WINHTTP_WEB_SOCKET_CLOSE_BUFFER_TYPE) {
            if (m_onStatus) m_onStatus("disconnected");
            m_connected.store(false);
            break;
        }
    }
}
