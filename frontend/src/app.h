// app.h — 应用编排：消息循环 + 各模块粘合（阶段三主流程，PRD 2.3 交互流程）
#pragma once
#include <string>
#include <windows.h>

// 运行选项（CLI 传入，QA 自动化用）
struct AppOptions {
    bool auto_download_models = false; // 模型缺失时跳过提示直接下载
    bool console_log = false;          // 追加控制台日志输出
    std::wstring audio_wav;            // 非空：用 WAV 音源替代麦克风（QA-01/02）
    bool wav_loop = false;             // WAV 音源循环播放（60s 护栏测试）
    std::string models_url;            // 覆盖配置中的模型下载地址
};

class App {
public:
    int run(HINSTANCE inst, const AppOptions& opts);

private:
    bool init(HINSTANCE inst);
    void shutdown();
    void message_loop();

    // 消息处理（隐藏消息窗口）
    static LRESULT CALLBACK wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp);
    LRESULT on_message(UINT msg, WPARAM wp, LPARAM lp);

    void on_ptt_press();
    void on_ptt_release();
    void on_capture_error();            // 采集线程错误（设备拔出等）
    bool start_audio(std::string* err); // 按选项选择麦克风/WAV 音源
    void stop_audio();
    void drain_results();               // 引擎事件触发：poll 直到空
    void handle_result(const std::string& json);
    void log(const char* fmt, ...);

    struct Impl;
    Impl* p_ = nullptr;
};
