// resampler.h — FFmpeg libswresample 重采样封装（CPP-03，PRD 3.4）
//
// 延迟加载（LoadLibrary + GetProcAddress）swresample-4.dll：
//   1) 缺 DLL 时进程仍可启动，由依赖管理器（CPP-04）提示修复；
//   2) 可读取 swresample_version 做版本匹配度校验。
// 输入：WASAPI 混音格式（F32/S16 交错，任意采样率，1/2 声道）
// 输出：16kHz s16 mono（引擎契约）
#pragma once
#include <cstdint>
#include <string>
#include <vector>

class Resampler {
public:
    // swresample 是否可用；expect_swr_major 为期望的库主版本（FFmpeg 6.0 -> swr 4）
    static bool available(int expect_swr_major, std::string* detail);

    Resampler();
    ~Resampler();
    Resampler(const Resampler&) = delete;
    Resampler& operator=(const Resampler&) = delete;

    // in_fmt_is_float: true=F32 交错, false=S16 交错；输出固定 16000Hz s16 mono
    bool init(int in_rate, int in_channels, bool in_fmt_is_float);

    // 追加转换结果到 out（16k s16 mono 采样点）
    bool convert(const uint8_t* data, int frames, std::vector<int16_t>& out);

    bool ok() const { return ctx_ != nullptr; }
    int out_rate() const { return 16000; }

private:
    void* ctx_ = nullptr; // SwrContext*
    int in_rate_ = 0;
};
