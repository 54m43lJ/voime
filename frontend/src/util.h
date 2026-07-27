// util.h — 通用小工具（UTF-8/UTF-16 转换、路径、简易 JSON 字段提取、热键组合解析）
#pragma once
#include <windows.h>
#include <cctype>
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

// ---- 热键组合解析（NOTSF 全局热键与 TSF PTT 复用，阶段五控制反转） ----

// 常见键名 -> VK（单字符 A-Z/0-9 或功能键名，大小写不敏感）
inline UINT vk_from_name(const std::wstring& name) {
    if (name.size() == 1) {
        wchar_t c = towupper(name[0]);
        if ((c >= L'A' && c <= L'Z') || (c >= L'0' && c <= L'9')) return (UINT)c;
        return 0;
    }
    struct { const wchar_t* name; UINT vk; } table[] = {
        {L"SPACE", VK_SPACE}, {L"TAB", VK_TAB}, {L"ENTER", VK_RETURN},
        {L"ESC", VK_ESCAPE}, {L"ESCAPE", VK_ESCAPE}, {L"BACKSPACE", VK_BACK},
        {L"INSERT", VK_INSERT}, {L"DELETE", VK_DELETE}, {L"HOME", VK_HOME},
        {L"END", VK_END}, {L"PAGEUP", VK_PRIOR}, {L"PAGEDOWN", VK_NEXT},
        {L"LEFT", VK_LEFT}, {L"RIGHT", VK_RIGHT}, {L"UP", VK_UP}, {L"DOWN", VK_DOWN},
        {L"CAPSLOCK", VK_CAPITAL}, {L"NUMLOCK", VK_NUMLOCK}, {L"SCROLLLOCK", VK_SCROLL},
        {L"PRINTSCREEN", VK_SNAPSHOT}, {L"PAUSE", VK_PAUSE},
        {L"F1", VK_F1}, {L"F2", VK_F2}, {L"F3", VK_F3}, {L"F4", VK_F4}, {L"F5", VK_F5},
        {L"F6", VK_F6}, {L"F7", VK_F7}, {L"F8", VK_F8}, {L"F9", VK_F9}, {L"F10", VK_F10},
        {L"F11", VK_F11}, {L"F12", VK_F12}, {L"F13", VK_F13}, {L"F14", VK_F14},
        {L"F15", VK_F15}, {L"F16", VK_F16}, {L"F17", VK_F17}, {L"F18", VK_F18},
        {L"F19", VK_F19}, {L"F20", VK_F20}, {L"F21", VK_F21}, {L"F22", VK_F22},
        {L"F23", VK_F23}, {L"F24", VK_F24},
    };
    std::wstring n(name);
    for (auto& c : n) c = towupper(c);
    for (auto& t : table)
        if (n == t.name) return t.vk;
    return 0;
}

// 解析 "Ctrl+Space" 组合 -> mods(MOD_CONTROL|MOD_ALT|MOD_SHIFT|MOD_WIN) + vk；失败 false
inline bool parse_hotkey_combo(const std::wstring& combo, UINT& mods, UINT& vk) {
    mods = 0;
    vk = 0;
    size_t pos = 0;
    while (pos < combo.size()) {
        size_t plus = combo.find(L'+', pos);
        std::wstring tok = combo.substr(pos, plus == std::wstring::npos ? plus : plus - pos);
        pos = (plus == std::wstring::npos) ? combo.size() : plus + 1;
        std::wstring t;
        for (auto c : tok) if (!iswspace(c)) t += (wchar_t)towupper(c);
        if (t.empty()) continue;
        if (t == L"CTRL" || t == L"CONTROL") mods |= MOD_CONTROL;
        else if (t == L"ALT") mods |= MOD_ALT;
        else if (t == L"SHIFT") mods |= MOD_SHIFT;
        else if (t == L"WIN" || t == L"WINDOWS") mods |= MOD_WIN;
        else {
            if (vk) return false; // 多个主键
            vk = vk_from_name(tok);
            if (!vk) return false;
        }
    }
    return vk != 0;
}
