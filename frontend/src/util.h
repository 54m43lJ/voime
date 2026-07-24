// util.h — 通用小工具（UTF-8/UTF-16 转换、路径、简易 JSON 字段提取）
#pragma once
#include <windows.h>
#include <string>

inline std::wstring utf8_to_wide(const std::string& s) {
    if (s.empty()) return L"";
    int n = MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), nullptr, 0);
    std::wstring w(n, L'\0');
    MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), w.data(), n);
    return w;
}

inline std::string wide_to_utf8(const std::wstring& w) {
    if (w.empty()) return "";
    int n = WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), nullptr, 0, nullptr, nullptr);
    std::string s(n, '\0');
    WideCharToMultiByte(CP_UTF8, 0, w.data(), (int)w.size(), s.data(), n, nullptr, nullptr);
    return s;
}

// 可执行文件所在目录（末尾带反斜杠）
inline std::wstring exe_dir() {
    wchar_t buf[MAX_PATH]{};
    GetModuleFileNameW(nullptr, buf, MAX_PATH);
    std::wstring p(buf);
    size_t pos = p.find_last_of(L"\\/");
    return pos == std::wstring::npos ? L".\\" : p.substr(0, pos + 1);
}

// 简易 JSON 字符串字段提取："key":"value"，处理 \" \\ \n \r \t 转义；未找到返回空串。
// 仅用于引擎结果（type/text/reason/detail），不解析嵌套结构。
inline std::string json_get_string(const std::string& json, const char* key) {
    std::string pat = std::string("\"") + key + "\":\"";
    size_t p = json.find(pat);
    if (p == std::string::npos) return "";
    p += pat.size();
    std::string out;
    for (size_t i = p; i < json.size(); i++) {
        char c = json[i];
        if (c == '"') return out;
        if (c == '\\' && i + 1 < json.size()) {
            char e = json[++i];
            switch (e) {
                case 'n': out += '\n'; break;
                case 'r': out += '\r'; break;
                case 't': out += '\t'; break;
                case '"': out += '"'; break;
                case '\\': out += '\\'; break;
                case '/': out += '/'; break;
                default: out += e; break; // \uXXXX 等罕见情形按原样保留
            }
        } else {
            out += c;
        }
    }
    return out;
}
