// wav_source.cpp — WAV 文件音源实现
#include "wav_source.h"
#include "wav.h"

#include <chrono>

static const int kChunkSamples = 480; // 30ms @16k（引擎 chunk 契约）

WavSource::~WavSource() { stop(); }

bool WavSource::load(const std::wstring& path) {
    Wav w = read_wav16(path);
    if (!w.ok || w.rate != 16000 || w.channels != 1) return false;
    pcm_ = std::move(w.pcm);
    return !pcm_.empty();
}

bool WavSource::start(PcmCallback on_pcm, ErrorCallback on_err, std::string* err) {
    if (running_) return true;
    if (pcm_.empty()) {
        if (err) *err = "WavSource 未加载音频";
        return false;
    }
    on_pcm_ = std::move(on_pcm);
    on_err_ = std::move(on_err);
    stop_req_ = false;
    th_ = std::thread(&WavSource::thread_main, this);
    running_ = true;
    return true;
}

void WavSource::stop() {
    stop_req_ = true;
    if (th_.joinable()) th_.join();
    running_ = false;
}

void WavSource::thread_main() {
    size_t pos = 0;
    auto next = std::chrono::steady_clock::now();
    while (!stop_req_) {
        if (pos + kChunkSamples > pcm_.size()) {
            if (loop_) {
                pos = 0;
            } else {
                // 文件播完：保持 running（等 PTT 松开），不再喂数据
                std::this_thread::sleep_for(std::chrono::milliseconds(50));
                continue;
            }
        }
        if (on_pcm_) on_pcm_(pcm_.data() + pos, kChunkSamples);
        pos += kChunkSamples;
        next += std::chrono::milliseconds(30);
        std::this_thread::sleep_until(next);
    }
}
