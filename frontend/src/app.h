// app.h — 应用编排：消息循环 + 各模块粘合
// 阶段五控制反转（PRD 2.2）：
//   标准模式（默认）  —— Agent 作为 IPC 服务端，被动响应 TSF DLL 的 start/stop 指令；
//   NOTSF 调试模式    —— 本地全局热键主动触发 + SendInput 注入（无 TSF DLL 产物）。
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

    // 统一触发入口：NOTSF=本地热键消息，标准=IPC 指令（返回值用于 IPC 应答 code）
    int trigger_start();                // 0=成功；负值=依赖/引擎错误码
    int trigger_stop();                 // 0=成功，-1=未在录音
    void on_capture_error();            // 采集线程错误（设备拔出等）
    bool start_audio(std::string* err); // 按选项选择麦克风/WAV 音源
    void stop_audio();
    void drain_results();               // 引擎事件触发：poll 直到空
    void handle_result(const std::string& json);
    void log(const char* fmt, ...);

    struct Impl;
    Impl* p_ = nullptr;
};
