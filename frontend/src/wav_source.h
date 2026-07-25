// wav_source.h — WAV 文件音源（QA-01 全链路自动化 / QA-02 护栏测试）
//
// 与 AudioCapture 相同的回调接口：按实时节奏（30ms/480 采样）把 16k s16 mono
// wav 喂给上层，完整走“采集线程 -> voime_push_audio -> 引擎 -> 事件 -> UI/注入”
// 产品代码路径。支持 --loop 循环（60s 护栏测试用）。
#pragma once
#include <atomic>
#include <cstdint>
#include <functional>
#include <string>
#include <thread>
#include <vector>

class WavSource {
public:
    using PcmCallback = std::function<void(const int16_t* data, size_t frames)>;
    using ErrorCallback = std::function<void(const std::string& err)>;

    WavSource() = default;
    ~WavSource();
    WavSource(const WavSource&) = delete;
    WavSource& operator=(const WavSource&) = delete;

    bool load(const std::wstring& path); // 16k s16 mono wav
    void set_loop(bool l) { loop_ = l; }

    bool start(PcmCallback on_pcm, ErrorCallback on_err, std::string* err);
    void stop();
    bool running() const { return running_.load(); }
    std::string device_format() const { return "wav 16000Hz 1ch 16bit pcm"; }

private:
    void thread_main();

    std::vector<int16_t> pcm_;
    PcmCallback on_pcm_;
    ErrorCallback on_err_;
    std::thread th_;
    std::atomic<bool> running_{false};
    std::atomic<bool> stop_req_{false};
    bool loop_ = false;
};
