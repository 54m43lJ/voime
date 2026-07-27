// ptt.cpp — TSF 侧 PTT 组合键配置读取
#include "ptt.h"
#include "util.h" // frontend/src（构建时 -I 引入）

#include <cstdio>
#include <cstring>
#include <string>

void load_ptt(UINT& mods, UINT& vk) {
    mods = MOD_CONTROL; // 默认 Ctrl+Space（与 config.h 默认值一致）
    vk = VK_SPACE;

    wchar_t dir[MAX_PATH];
    DWORD n = GetEnvironmentVariableW(L"APPDATA", dir, MAX_PATH);
    if (!n || n >= MAX_PATH) return;
    std::wstring path = std::wstring(dir) + L"\\VoimeInput\\config.toml";
    FILE* fp = _wfopen(path.c_str(), L"rb");
    if (!fp) return;
    char buf[4096];
    size_t len = fread(buf, 1, sizeof(buf) - 1, fp);
    fclose(fp);
    buf[len] = 0;

    // 极简解析：定位 hotkey = "..."
    const char* p = strstr(buf, "hotkey");
    if (!p) return;
    const char* q1 = strchr(p, '"');
    if (!q1) return;
    const char* q2 = strchr(q1 + 1, '"');
    if (!q2) return;
    std::wstring combo = utf8_to_wide(std::string(q1 + 1, q2 - q1 - 1));
    UINT m = 0, k = 0;
    if (parse_hotkey_combo(combo, m, k)) {
        mods = m;
        vk = k;
    }
}
