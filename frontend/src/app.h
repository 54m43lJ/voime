// app.h — 应用编排：消息循环 + 各模块粘合（阶段三主流程，PRD 2.3 交互流程）
#pragma once
#include <string>
#include <windows.h>

class App {
public:
    int run(HINSTANCE inst, bool with_console);

private:
    bool init(HINSTANCE inst);
    void shutdown();
    void message_loop();

    // 消息处理（隐藏消息窗口）
    static LRESULT CALLBACK wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp);
    LRESULT on_message(UINT msg, WPARAM wp, LPARAM lp);

    void on_ptt_press();
    void on_ptt_release();
    void drain_results();               // 引擎事件触发：poll 直到空
    void handle_result(const std::string& json);
    void log(const char* fmt, ...);

    struct Impl;
    Impl* p_ = nullptr;
};
