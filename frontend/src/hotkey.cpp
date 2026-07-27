// hotkey.cpp — WH_KEYBOARD_LL PTT 实现（仅 NOTSF 模式编译，P5-06）
#include "hotkey.h"
#include "util.h"
#include <cctype>

Hotkey* Hotkey::instance = nullptr;

bool Hotkey::parse_combo() { return parse_hotkey_combo(combo_, mods_, vk_); }

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
