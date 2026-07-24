// audio_capture.cpp — WASAPI 共享模式事件驱动采集实现
#include "audio_capture.h"
#include "resampler.h"

#include <windows.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <avrt.h>
#include <functiondiscoverykeys_devpkey.h>

static const int kChunkSamples = 480; // 30ms @16k（引擎 chunk 契约）

AudioCapture::~AudioCapture() { stop(); }

bool AudioCapture::start(PcmCallback on_pcm, ErrorCallback on_err, std::string* err) {
    if (running_) return true;
    on_pcm_ = std::move(on_pcm);
    on_err_ = std::move(on_err);
    stop_req_ = false;
    init_done_ = false;
    format_desc_.clear();
    th_ = std::thread(&AudioCapture::thread_main, this);
    // 等待采集线程初始化结果（成功 running_=true，失败 init_done_=true 且 running_=false）
    for (int i = 0; i < 250 && !running_ && !init_done_; i++) Sleep(20); // 最长 5s
    if (!running_) {
        if (th_.joinable()) th_.join();
        if (err) *err = format_desc_.empty() ? "采集初始化失败（无可用设备?）" : format_desc_;
        return false;
    }
    return true;
}

void AudioCapture::stop() {
    stop_req_ = true;
    if (th_.joinable()) th_.join();
    running_ = false;
}

void AudioCapture::thread_main() {
    CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    DWORD mmcss_task = 0;
    HANDLE mmcss = AvSetMmThreadCharacteristicsW(L"Pro Audio", &mmcss_task);

    HRESULT hr;
    IMMDeviceEnumerator* enumerator = nullptr;
    IMMDevice* device = nullptr;
    IAudioClient* client = nullptr;
    IAudioCaptureClient* capture = nullptr;
    WAVEFORMATEX* mix = nullptr;
    HANDLE ev = CreateEventW(nullptr, FALSE, FALSE, nullptr);

    struct Guard {
        IMMDeviceEnumerator*& e; IMMDevice*& d; IAudioClient*& c;
        IAudioCaptureClient*& cap; WAVEFORMATEX*& m; HANDLE& ev; HANDLE& mmcss;
        ~Guard() {
            if (cap) cap->Release();
            if (c) c->Release();
            if (m) CoTaskMemFree(m);
            if (d) d->Release();
            if (e) e->Release();
            if (ev) CloseHandle(ev);
            if (mmcss) AvRevertMmThreadCharacteristics(mmcss);
            CoUninitialize();
        }
    } guard{enumerator, device, client, capture, mix, ev, mmcss};

    auto fail = [&](const char* what, HRESULT h) {
        char buf[160];
        snprintf(buf, sizeof(buf), "[wasapi] %s hr=0x%08lx", what, (unsigned long)h);
        format_desc_ = buf;
        if (on_err_) on_err_(buf);
        init_done_ = true;
    };

    hr = CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL,
                          __uuidof(IMMDeviceEnumerator), (void**)&enumerator);
    if (FAILED(hr)) return fail("CoCreateInstance", hr), void();
    hr = enumerator->GetDefaultAudioEndpoint(eCapture, eConsole, &device);
    if (FAILED(hr)) return fail("GetDefaultAudioEndpoint(无采集设备?)", hr), void();
    hr = device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, (void**)&client);
    if (FAILED(hr)) return fail("Activate", hr), void();
    hr = client->GetMixFormat(&mix);
    if (FAILED(hr)) return fail("GetMixFormat", hr), void();

    int sr = mix->nSamplesPerSec, ch = mix->nChannels;
    bool is_float = mix->wFormatTag == WAVE_FORMAT_IEEE_FLOAT;
    if (mix->wFormatTag == WAVE_FORMAT_EXTENSIBLE && mix->cbSize >= 22) {
        auto* ext = (WAVEFORMATEXTENSIBLE*)mix;
        is_float = ext->SubFormat.Data1 == 0x00000003; // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    }
    int bits = mix->wBitsPerSample;
    if (!(is_float && bits == 32) && !(!is_float && bits == 16)) {
        format_desc_ = "不支持的混音格式（仅支持 32bit float / 16bit PCM）";
        if (on_err_) on_err_(format_desc_);
        init_done_ = true;
        return;
    }
    {
        char buf[96];
        snprintf(buf, sizeof(buf), "%dHz %dch %dbit %s", sr, ch, bits, is_float ? "float" : "pcm");
        format_desc_ = buf;
    }

    // 事件驱动共享模式，200ms 缓冲
    hr = client->Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                            200000, 0, mix, nullptr);
    if (FAILED(hr)) return fail("Initialize", hr), void();
    hr = client->SetEventHandle(ev);
    if (FAILED(hr)) return fail("SetEventHandle", hr), void();
    hr = client->GetService(__uuidof(IAudioCaptureClient), (void**)&capture);
    if (FAILED(hr)) return fail("GetService", hr), void();

    Resampler rs;
    if (!rs.init(sr, ch, is_float)) {
        format_desc_ = "重采样器初始化失败（FFmpeg 依赖缺失?）";
        if (on_err_) on_err_(format_desc_);
        init_done_ = true;
        return;
    }

    hr = client->Start();
    if (FAILED(hr)) return fail("Start", hr), void();
    running_ = true;
    init_done_ = true;

    std::vector<int16_t> acc; // 16k s16 mono 累积缓冲
    acc.reserve(4096);

    while (!stop_req_) {
        DWORD w = WaitForSingleObject(ev, 200);
        if (w == WAIT_FAILED) break;
        UINT32 packet = 0;
        hr = capture->GetNextPacketSize(&packet);
        if (FAILED(hr)) { // 设备拔出等：AUDCLNT_E_DEVICE_INVALIDATED
            if (on_err_) {
                char buf[96];
                snprintf(buf, sizeof(buf), "[wasapi] 采集中断 hr=0x%08lx", (unsigned long)hr);
                on_err_(buf);
            }
            break;
        }
        while (packet > 0) {
            BYTE* data = nullptr;
            UINT32 frames = 0;
            DWORD flags = 0;
            hr = capture->GetBuffer(&data, &frames, &flags, nullptr, nullptr);
            if (FAILED(hr) || frames == 0) { if (SUCCEEDED(hr)) hr = E_FAIL; break; }
            if (!(flags & AUDCLNT_BUFFERFLAGS_SILENT) && data) {
                rs.convert(data, (int)frames, acc);
            } else { // 静音包：补零保持时序
                size_t base = acc.size();
                int zframes = (int)((int64_t)frames * 16000 / sr);
                acc.resize(base + (size_t)zframes, 0);
            }
            capture->ReleaseBuffer(frames);
            hr = capture->GetNextPacketSize(&packet);
            if (FAILED(hr)) break;
        }
        while (acc.size() >= (size_t)kChunkSamples) {
            if (on_pcm_) on_pcm_(acc.data(), kChunkSamples);
            acc.erase(acc.begin(), acc.begin() + kChunkSamples);
        }
    }
    client->Stop();
    running_ = false;
}
