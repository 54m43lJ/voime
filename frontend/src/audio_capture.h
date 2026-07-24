// audio_capture.h — WASAPI 共享模式采集（CPP-02，PRD 3.4）
//
// 独立高优先级线程（MMCSS "Pro Audio"），事件驱动采集系统默认混音格式，
// 经 Resampler（FFmpeg）转换为 16k s16 mono，按 480 采样（30ms）chunk 回调。
#pragma once
#include <atomic>
#include <cstdint>
#include <functional>
#include <string>
#include <thread>
#include <vector>

class AudioCapture {
public:
    // 16k s16 mono，每次恰好 480 采样（30ms，引擎 chunk 契约）
    using PcmCallback = std::function<void(const int16_t* data, size_t frames)>;
    using ErrorCallback = std::function<void(const std::string& err)>;

    AudioCapture() = default;
    ~AudioCapture();
    AudioCapture(const AudioCapture&) = delete;
    AudioCapture& operator=(const AudioCapture&) = delete;

    // 启动采集线程；失败返回 false 并填 err（如无采集设备）
    bool start(PcmCallback on_pcm, ErrorCallback on_err, std::string* err);
    void stop();
    bool running() const { return running_.load(); }

    // 采集设备混音格式描述，如 "48000Hz 2ch 32bit float"（start 成功后可用）
    std::string device_format() const { return format_desc_; }

private:
    void thread_main();

    PcmCallback on_pcm_;
    ErrorCallback on_err_;
    std::thread th_;
    std::atomic<bool> running_{false};
    std::atomic<bool> stop_req_{false};
    std::atomic<bool> init_done_{false};
    std::string format_desc_;
};
