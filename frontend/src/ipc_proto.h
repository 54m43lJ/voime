// ipc_proto.h — Voime IPC 协议（PRD 2.3.3，阶段五 P5-02/P5-03）
//
// Named Pipe（消息模式，PIPE_TYPE_MESSAGE）+ 每条消息一个 JSON 对象：
//   TSF -> Agent: {"action":"start_session|stop_session","request_id":"uuid"}
//   Agent -> TSF: {"action":"commit_text","payload":"...","request_id":"uuid"}
//   应答:        {"request_id":"uuid","status":"success|error","code":N}
// Agent（voime_input.exe）与 TSF（voime_tsf.dll）共用本头文件。
#pragma once
#include "util.h"

#include <windows.h>
#include <bcrypt.h>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <string>

namespace ipc {

inline const wchar_t* kPipeName = L"\\\\.\\pipe\\VoimeInput.IPC";
inline const DWORD kPipeBuf = 64 * 1024; // 管道收发缓冲（消息上限）

// uuid v4 request_id（仅标识用途，无安全语义）
inline std::string new_request_id() {
    uint8_t b[16];
    if (BCryptGenRandom(nullptr, b, sizeof(b), BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0) {
        DWORD t = GetTickCount();
        for (auto& x : b) { x = (uint8_t)(t & 0xFF); t = t * 1664525u + 1013904223u; }
    }
    b[6] = (uint8_t)((b[6] & 0x0F) | 0x40);
    b[8] = (uint8_t)((b[8] & 0x3F) | 0x80);
    char out[37];
    snprintf(out, sizeof(out),
             "%02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x",
             b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
             b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]);
    return out;
}

inline std::string json_escape(const std::string& s) {
    std::string out;
    out.reserve(s.size() + 8);
    for (char c : s) {
        switch (c) {
            case '"': out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if ((unsigned char)c < 0x20) {
                    char buf[8];
                    snprintf(buf, sizeof(buf), "\\u%04x", c);
                    out += buf;
                } else {
                    out += c;
                }
        }
    }
    return out;
}

// {"action":"...","request_id":"..."}
inline std::string make_session_cmd(const char* action, const std::string& request_id) {
    return std::string("{\"action\":\"") + action + "\",\"request_id\":\"" + request_id + "\"}";
}

// {"action":"commit_text","payload":"...","request_id":"..."}
inline std::string make_commit_text(const std::string& payload_utf8,
                                    const std::string& request_id) {
    return "{\"action\":\"commit_text\",\"payload\":\"" + json_escape(payload_utf8) +
           "\",\"request_id\":\"" + request_id + "\"}";
}

// {"request_id":"...","status":"success|error","code":N}
inline std::string make_response(const std::string& request_id, bool ok, int code) {
    char buf[192];
    snprintf(buf, sizeof(buf), "{\"request_id\":\"%s\",\"status\":\"%s\",\"code\":%d}",
             request_id.c_str(), ok ? "success" : "error", code);
    return buf;
}

// 提取整数字段（"code":N），缺失返回 dflt
inline int json_get_int(const std::string& json, const char* key, int dflt) {
    std::string pat = std::string("\"") + key + "\":";
    size_t p = json.find(pat);
    if (p == std::string::npos) return dflt;
    return atoi(json.c_str() + p + pat.size());
}

} // namespace ipc
