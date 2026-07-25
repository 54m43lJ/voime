// wav.h — 简易 WAV 读取（PCM16），selftest 与 WavSource 共用
#pragma once
#include <cstdint>
#include <cstring>
#include <vector>
#include <string>
#include <cstdio>

struct Wav {
    std::vector<int16_t> pcm; // s16 交错
    int rate = 0, channels = 0;
    bool ok = false;
};

inline Wav read_wav16(const std::wstring& path) {
    Wav w;
    FILE* f = _wfopen(path.c_str(), L"rb");
    if (!f) return w;
    uint8_t hdr[12];
    if (fread(hdr, 1, 12, f) != 12 || memcmp(hdr, "RIFF", 4) || memcmp(hdr + 8, "WAVE", 4)) {
        fclose(f);
        return w;
    }
    uint16_t fmt_tag = 0, ch = 0;
    uint32_t rate = 0, bits = 0;
    std::vector<uint8_t> data;
    for (;;) {
        uint8_t ch_hdr[8];
        if (fread(ch_hdr, 1, 8, f) != 8) break;
        uint32_t sz = *(uint32_t*)(ch_hdr + 4);
        long next = ftell(f) + sz + (sz & 1);
        if (!memcmp(ch_hdr, "fmt ", 4)) {
            uint8_t buf[40]{};
            fread(buf, 1, sz > 40 ? 40 : sz, f);
            fmt_tag = *(uint16_t*)buf;
            ch = *(uint16_t*)(buf + 2);
            rate = *(uint32_t*)(buf + 4);
            bits = *(uint16_t*)(buf + 14);
        } else if (!memcmp(ch_hdr, "data", 4)) {
            data.resize(sz);
            fread(data.data(), 1, sz, f);
        }
        fseek(f, next, SEEK_SET);
    }
    fclose(f);
    if (fmt_tag != 1 || bits != 16 || data.empty() || ch == 0) return w; // 仅收 PCM16
    w.pcm.assign((int16_t*)data.data(), (int16_t*)(data.data() + data.size()));
    w.rate = (int)rate;
    w.channels = ch;
    w.ok = true;
    return w;
}
