// hotkey.h — 全局热键 PTT（CPP-05，PRD 4.2 全局热键）
//
// WH_KEYBOARD_LL 低级键盘钩子捕获 KeyDown/KeyUp（抑制自动重复），
// 经 PostMessage 投递到消息窗口（回调内不做任何重活，远离 LowLevelHooksTimeout）。
// 守护：定时 watchdog_tick() 主动重装钩子，防超时静默卸载失效。
#pragma once
#include <windows.h>
#include <string>

class Hotkey {
public:
    // 消息 ID（投递给 install 时指定的 msg_wnd）
    static const UINT WM_PRESS = WM_APP + 1;
    static const UINT WM_RELEASE = WM_APP + 2;

    // combo 形如 "Ctrl+Space"（修饰键 Ctrl/Alt/Shift/Win + 一个普通键）
    bool install(HWND msg_wnd, const std::wstring& combo);
    void uninstall();

    // 守护节拍：校验并按需重装（由主线程定时器驱动）
    void watchdog_tick();

    bool installed() const { return hook_ != nullptr; }
    const std::wstring& combo() const { return combo_; }

    static Hotkey* instance; // 供静态钩子弹回到实例

private:
    bool parse_combo();
    static LRESULT CALLBACK hook_proc(int code, WPARAM wp, LPARAM lp);
    LRESULT on_key(int code, WPARAM wp, LPARAM lp);

    HWND msg_wnd_ = nullptr;
    HHOOK hook_ = nullptr;
    std::wstring combo_;
    UINT mods_ = 0;   // MOD_CONTROL|MOD_ALT|MOD_SHIFT|MOD_WIN
    UINT vk_ = 0;     // 主键虚拟键码
    bool pressed_ = false;
    DWORD reinstall_count_ = 0;
};
