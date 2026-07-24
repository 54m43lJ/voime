// resampler.cpp — FFmpeg libswresample 延迟加载封装实现
#include "resampler.h"

extern "C" {
#include <libswresample/swresample.h>
#include <libavutil/channel_layout.h>
#include <libavutil/samplefmt.h>
}

#include <windows.h>

// ---- 延迟加载的符号表 ----
static HMODULE g_swr = nullptr;
static unsigned (*p_swresample_version)() = nullptr;
static SwrContext* (*p_swr_alloc)() = nullptr;
static int (*p_swr_alloc_set_opts2)(SwrContext**, const AVChannelLayout*, enum AVSampleFormat, int,
                                    const AVChannelLayout*, enum AVSampleFormat, int, int, void*) = nullptr;
static int (*p_swr_init)(SwrContext*) = nullptr;
static int (*p_swr_convert)(SwrContext*, uint8_t**, int, const uint8_t**, int) = nullptr;
static void (*p_swr_free)(SwrContext**) = nullptr;

static bool load_swr(std::string* detail) {
    if (g_swr) return true;
    g_swr = LoadLibraryW(L"swresample-4.dll"); // 标准搜索：exe 目录优先
    if (!g_swr) {
        if (detail) *detail = "swresample-4.dll 加载失败, GetLastError=" + std::to_string(GetLastError());
        return false;
    }
#define R(name) p_##name = (decltype(p_##name))GetProcAddress(g_swr, #name)
    R(swresample_version); R(swr_alloc); R(swr_alloc_set_opts2); R(swr_init); R(swr_convert); R(swr_free);
#undef R
    if (!p_swresample_version || !p_swr_alloc || !p_swr_alloc_set_opts2 || !p_swr_init ||
        !p_swr_convert || !p_swr_free) {
        if (detail) *detail = "swresample-4.dll 导出符号不完整（版本不符?）";
        FreeLibrary(g_swr);
        g_swr = nullptr;
        return false;
    }
    return true;
}

bool Resampler::available(int expect_swr_major, std::string* detail) {
    if (!load_swr(detail)) return false;
    unsigned v = p_swresample_version();
    int major = (int)(v >> 16), minor = (int)((v >> 8) & 0xff), micro = (int)(v & 0xff);
    if (detail) {
        *detail = "libswresample " + std::to_string(major) + "." + std::to_string(minor) +
                  "." + std::to_string(micro);
    }
    if (major != expect_swr_major) {
        if (detail) *detail += "（期望主版本 " + std::to_string(expect_swr_major) + "，版本不匹配）";
        return false;
    }
    return true;
}

// 填充声道布局：1/2 声道用 NATIVE 掩码，其它声道数用 UNSPEC（仅声道计数）
static AVChannelLayout make_layout(int channels) {
    AVChannelLayout l{};
    if (channels == 1) {
        l.order = AV_CHANNEL_ORDER_NATIVE; l.nb_channels = 1; l.u.mask = AV_CH_LAYOUT_MONO;
    } else if (channels == 2) {
        l.order = AV_CHANNEL_ORDER_NATIVE; l.nb_channels = 2; l.u.mask = AV_CH_LAYOUT_STEREO;
    } else {
        l.order = AV_CHANNEL_ORDER_UNSPEC; l.nb_channels = channels; l.u.mask = 0;
    }
    return l;
}

Resampler::Resampler() = default;

Resampler::~Resampler() {
    if (ctx_ && p_swr_free) {
        SwrContext* c = (SwrContext*)ctx_;
        p_swr_free(&c);
        ctx_ = nullptr;
    }
}

bool Resampler::init(int in_rate, int in_channels, bool in_fmt_is_float) {
    if (ctx_) return true;
    if (!load_swr(nullptr)) return false;
    AVChannelLayout out_l = make_layout(1);
    AVChannelLayout in_l = make_layout(in_channels);
    SwrContext* c = nullptr;
    int rc = p_swr_alloc_set_opts2(&c, &out_l, AV_SAMPLE_FMT_S16, 16000, &in_l,
                                   in_fmt_is_float ? AV_SAMPLE_FMT_FLT : AV_SAMPLE_FMT_S16,
                                   in_rate, 0, nullptr);
    if (rc < 0 || !c) return false;
    if (p_swr_init(c) < 0) {
        p_swr_free(&c);
        return false;
    }
    ctx_ = c;
    in_rate_ = in_rate;
    return true;
}

bool Resampler::convert(const uint8_t* data, int frames, std::vector<int16_t>& out) {
    if (!ctx_ || frames <= 0) return false;
    // 输出上限估算：frames * 16000 / in_rate + 余量
    int max_out = (int)((int64_t)frames * 16000 / in_rate_) + 64;
    size_t base = out.size();
    out.resize(base + (size_t)max_out);
    uint8_t* dst[1] = { (uint8_t*)(out.data() + base) };
    const uint8_t* src[1] = { data };
    int n = p_swr_convert((SwrContext*)ctx_, dst, max_out, src, frames);
    if (n < 0) {
        out.resize(base);
        return false;
    }
    out.resize(base + (size_t)n);
    return true;
}
