// hotkey.cpp — WH_KEYBOARD_LL PTT 实现
#include "hotkey.h"
#include <cctype>

Hotkey* Hotkey::instance = nullptr;

// 常见键名 -> VK
static UINT vk_from_name(const std::wstring& name) {
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
    for (auto& t : table) {
        std::wstring n(name);
        for (auto& c : n) c = towupper(c);
        if (n == t.name) return t.vk;
    }
    return 0;
}

bool Hotkey::parse_combo() {
    mods_ = 0;
    vk_ = 0;
    size_t pos = 0;
    std::wstring s = combo_;
    while (pos < s.size()) {
        size_t plus = s.find(L'+', pos);
        std::wstring tok = s.substr(pos, plus == std::wstring::npos ? plus : plus - pos);
        pos = (plus == std::wstring::npos) ? s.size() : plus + 1;
        // 去空格并转大写比较
        std::wstring t;
        for (auto c : tok) if (!iswspace(c)) t += (wchar_t)towupper(c);
        if (t.empty()) continue;
        if (t == L"CTRL" || t == L"CONTROL") mods_ |= MOD_CONTROL;
        else if (t == L"ALT") mods_ |= MOD_ALT;
        else if (t == L"SHIFT") mods_ |= MOD_SHIFT;
        else if (t == L"WIN" || t == L"WINDOWS") mods_ |= MOD_WIN;
        else {
            if (vk_) return false; // 多个主键
            vk_ = vk_from_name(tok);
            if (!vk_) return false;
        }
    }
    return vk_ != 0;
}

bool Hotkey::install(HWND msg_wnd, const std::wstring& combo) {
    uninstall();
    msg_wnd_ = msg_wnd;
    combo_ = combo;
    if (!parse_combo()) return false;
    instance = this;
    hook_ = SetWindowsHookExW(WH_KEYBOARD_LL, &Hotkey::hook_proc, GetModuleHandleW(nullptr), 0);
    if (!hook_) return false;
    pressed_ = false;
    return true;
}

void Hotkey::uninstall() {
    if (hook_) UnhookWindowsHookEx(hook_);
    hook_ = nullptr;
    if (instance == this) instance = nullptr;
}

void Hotkey::watchdog_tick() {
    if (!msg_wnd_) return;
    // 钩子句柄丢失（被系统超时静默卸载）或定时预防性重装
    if (!hook_) {
        reinstall_count_++;
        hook_ = SetWindowsHookExW(WH_KEYBOARD_LL, &Hotkey::hook_proc,
                                  GetModuleHandleW(nullptr), 0);
        if (hook_) instance = this;
        return;
    }
    if (++reinstall_count_ >= 12) { // 每 12 拍（默认 5s/拍 ≈ 60s）预防性重装
        reinstall_count_ = 0;
        UnhookWindowsHookEx(hook_);
        hook_ = SetWindowsHookExW(WH_KEYBOARD_LL, &Hotkey::hook_proc,
                                  GetModuleHandleW(nullptr), 0);
        if (hook_) instance = this;
        pressed_ = false; // 重装后复位状态机，避免卡键
    }
}

LRESULT CALLBACK Hotkey::hook_proc(int code, WPARAM wp, LPARAM lp) {
    if (instance) return instance->on_key(code, wp, lp);
    return CallNextHookEx(nullptr, code, wp, lp);
}

LRESULT Hotkey::on_key(int code, WPARAM wp, LPARAM lp) {
    if (code == HC_ACTION && msg_wnd_) {
        auto* kb = (KBDLLHOOKSTRUCT*)lp;
        bool down = (wp == WM_KEYDOWN || wp == WM_SYSKEYDOWN);
        bool up = (wp == WM_KEYUP || wp == WM_SYSKEYUP);
        if ((down || up) && kb->vkCode == vk_) {
            bool ctrl = (GetAsyncKeyState(VK_CONTROL) & 0x8000) != 0;
            bool alt = (GetAsyncKeyState(VK_MENU) & 0x8000) != 0;
            bool shift = (GetAsyncKeyState(VK_SHIFT) & 0x8000) != 0;
            bool win = ((GetAsyncKeyState(VK_LWIN) | GetAsyncKeyState(VK_RWIN)) & 0x8000) != 0;
            UINT cur = (ctrl ? MOD_CONTROL : 0) | (alt ? MOD_ALT : 0) |
                       (shift ? MOD_SHIFT : 0) | (win ? MOD_WIN : 0);
            if (down) {
                // pressed_ 状态机天然抑制系统自动重复
                if (!pressed_ && cur == mods_) {
                    pressed_ = true;
                    PostMessageW(msg_wnd_, WM_PRESS, 0, 0);
                }
            } else if (pressed_) {
                pressed_ = false;
                PostMessageW(msg_wnd_, WM_RELEASE, 0, 0);
            }
        }
    }
    return CallNextHookEx(nullptr, code, wp, lp);
}
