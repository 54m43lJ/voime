// Voime POC-06 C++ 宿主：WASAPI 共享模式采集 -> Rust FFI。
//
// 验证点（对照 PRD 3.3/4.2）：
//  - WASAPI 共享模式采集 48k F32，C++ 端重采样为 16kHz s16 mono
//  - voime_push_audio 不阻塞采集线程（微秒级入队）
//  - Windows Event 异步通知 + poll_result/free_string 所有权转移
//  - 无锁队列溢出返回 -2（丢帧策略）、Finalizing 状态 start 返回 -3
//  - 长时间运行内存无增长（队列/FFI 无泄漏）
//
// 构建: g++ -std=c++17 poc_host.cpp -o poc_host.exe -lole32 -lwinmm -lavrt
//       （与 voime_poc_core.dll 同目录运行）
#include <windows.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <functiondiscoverykeys_devpkey.h>
#include <psapi.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

// ---------------- Rust FFI 声明（对照 PRD 5.1） ----------------
extern "C" {
void* voime_create_engine();
void  voime_destroy_engine(void* h);
int32_t voime_start_session(void* h);
int32_t voime_stop_session(void* h);
int32_t voime_push_audio(void* h, const int16_t* data, uint32_t len);
void  voime_register_event(void* h, void* ev);
const char* voime_poll_result(void* h, uint32_t* out_len);
void  voime_free_string(const char* p);
// POC 测试辅助
void  voime_set_verbose(void* h, int on);
void  voime_test_set_worker_delay(void* h, uint32_t ms);
const char* voime_version();
}

static const int TARGET_SR = 16000;
static const int CHUNK_SAMPLES = 480; // 30ms @16k

static std::atomic<bool> g_done{false};

// ---------------- 工具 ----------------
static void log_mem(const char* tag) {
    PROCESS_MEMORY_COUNTERS_EX pmc{};
    pmc.cb = sizeof(pmc);
    GetProcessMemoryInfo(GetCurrentProcess(), (PROCESS_MEMORY_COUNTERS*)&pmc, sizeof(pmc));
    printf("[mem] %-24s working_set=%.1fMB private=%.1fMB\n", tag,
           pmc.WorkingSetSize / 1048576.0, pmc.PrivateUsage / 1048576.0);
}

// 结果拉取线程：等事件 -> poll 直到 NULL -> 打印 -> free
static void result_waiter(void* engine, HANDLE ev) {
    while (!g_done.load()) {
        DWORD r = WaitForSingleObject(ev, 200);
        if (r != WAIT_OBJECT_0) continue;
        for (;;) {
            uint32_t len = 0;
            const char* s = voime_poll_result(engine, &len);
            if (!s) break;
            printf("[cpp] result (%uB): %s\n", len, s);
            voime_free_string(s);
        }
    }
    // 收尾再排空一次
    for (;;) {
        uint32_t len = 0;
        const char* s = voime_poll_result(engine, &len);
        if (!s) break;
        printf("[cpp] result (%uB): %s\n", len, s);
        voime_free_string(s);
    }
}

// 线性插值重采样 + 立体声混单声道 + float->s16
static void resample_push(void* engine, const float* src, uint32_t frames, int channels,
                          int src_sr, std::vector<int16_t>& acc, std::atomic<uint64_t>& pushed) {
    double ratio = (double)src_sr / TARGET_SR;
    uint32_t out_frames = (uint32_t)(frames / ratio);
    for (uint32_t i = 0; i < out_frames; i++) {
        double pos = i * ratio;
        uint32_t i0 = (uint32_t)pos;
        uint32_t i1 = i0 + 1 < frames ? i0 + 1 : frames - 1;
        double frac = pos - i0;
        float mono0 = 0.f, mono1 = 0.f;
        for (int c = 0; c < channels; c++) {
            mono0 += src[i0 * channels + c];
            mono1 += src[i1 * channels + c];
        }
        mono0 /= channels;
        mono1 /= channels;
        float v = mono0 + (float)((mono1 - mono0) * frac);
        if (v > 1.0f) v = 1.0f;
        if (v < -1.0f) v = -1.0f;
        acc.push_back((int16_t)lrintf(v * 32767.0f));
    }
    // 凑满 30ms chunk 就推给 Rust
    while (acc.size() >= CHUNK_SAMPLES) {
        int32_t rc = voime_push_audio(engine, acc.data(), CHUNK_SAMPLES);
        if (rc != 0) printf("[cpp] push_audio rc=%d\n", rc);
        else pushed++;
        acc.erase(acc.begin(), acc.begin() + CHUNK_SAMPLES);
    }
}

// ---------------- WASAPI 采集 ----------------
static int run_wasapi(void* engine, int seconds) {
    HRESULT hr;
    IMMDeviceEnumerator* enumerator = nullptr;
    IMMDevice* device = nullptr;
    IAudioClient* client = nullptr;
    IAudioCaptureClient* capture = nullptr;
    WAVEFORMATEX* mix = nullptr;
    int ret = -1;

    hr = CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL,
                          __uuidof(IMMDeviceEnumerator), (void**)&enumerator);
    if (FAILED(hr)) { printf("[wasapi] CoCreateInstance hr=0x%08lx\n", hr); return -1; }
    hr = enumerator->GetDefaultAudioEndpoint(eCapture, eConsole, &device);
    if (FAILED(hr)) { printf("[wasapi] GetDefaultAudioEndpoint hr=0x%08lx (无采集设备?)\n", hr); goto cleanup; }

    {   // 打印设备名
        IPropertyStore* props = nullptr;
        if (SUCCEEDED(device->OpenPropertyStore(STGM_READ, &props))) {
            PROPVARIANT v; PropVariantInit(&v);
            if (SUCCEEDED(props->GetValue(PKEY_Device_FriendlyName, &v))) {
                wprintf(L"[wasapi] 采集设备: %s\n", v.pwszVal);
                PropVariantClear(&v);
            }
            props->Release();
        }
    }
    hr = device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, (void**)&client);
    if (FAILED(hr)) { printf("[wasapi] Activate hr=0x%08lx\n", hr); goto cleanup; }
    hr = client->GetMixFormat(&mix);
    if (FAILED(hr)) { printf("[wasapi] GetMixFormat hr=0x%08lx\n", hr); goto cleanup; }
    hr = client->Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 200000 /*200ms*/, 0, mix, nullptr);
    if (FAILED(hr)) { printf("[wasapi] Initialize hr=0x%08lx\n", hr); goto cleanup; }
    hr = client->GetService(__uuidof(IAudioCaptureClient), (void**)&capture);
    if (FAILED(hr)) { printf("[wasapi] GetService hr=0x%08lx\n", hr); goto cleanup; }

    {
        int sr = mix->nSamplesPerSec, ch = mix->nChannels;
        bool is_float = mix->wFormatTag == WAVE_FORMAT_IEEE_FLOAT;
        if (mix->wFormatTag == WAVE_FORMAT_EXTENSIBLE && mix->cbSize >= 22) {
            auto* ext = (WAVEFORMATEXTENSIBLE*)mix;
            is_float = ext->SubFormat.Data1 == 0x00000003; // IEEE_FLOAT
        }
        printf("[wasapi] mix format: %dHz %dch %dbit float=%d\n", sr, ch,
               mix->wBitsPerSample, (int)is_float);
        if (!is_float || mix->wBitsPerSample != 32) {
            printf("[wasapi] 非 32bit float 格式，POC 仅支持 F32，退出采集\n");
            goto cleanup;
        }
        hr = client->Start();
        if (FAILED(hr)) { printf("[wasapi] Start hr=0x%08lx\n", hr); goto cleanup; }
        printf("[wasapi] 开始采集 %ds (共享模式, 重采样 %d->16000)\n", seconds, sr);

        std::vector<int16_t> acc;
        std::atomic<uint64_t> pushed{0};
        auto t_end = std::chrono::steady_clock::now() + std::chrono::seconds(seconds);
        auto t_mem = std::chrono::steady_clock::now();
        int mem_tick = 0;
        while (std::chrono::steady_clock::now() < t_end && !g_done.load()) {
            Sleep(10);
            if (std::chrono::steady_clock::now() - t_mem > std::chrono::seconds(5)) {
                char tag[32]; snprintf(tag, sizeof(tag), "wasapi %ds", ++mem_tick * 5);
                log_mem(tag);
                t_mem = std::chrono::steady_clock::now();
            }
            UINT32 packet = 0;
            hr = capture->GetNextPacketSize(&packet);
            if (FAILED(hr)) { printf("[wasapi] GetNextPacketSize hr=0x%08lx\n", hr); break; }
            while (packet > 0) {
                BYTE* data = nullptr;
                UINT32 frames = 0;
                DWORD flags = 0;
                hr = capture->GetBuffer(&data, &frames, &flags, nullptr, nullptr);
                if (FAILED(hr)) { printf("[wasapi] GetBuffer hr=0x%08lx\n", hr); break; }
                if (!(flags & AUDCLNT_BUFFERFLAGS_SILENT) && data) {
                    resample_push(engine, (const float*)data, frames, ch, sr, acc, pushed);
                }
                capture->ReleaseBuffer(frames);
                hr = capture->GetNextPacketSize(&packet);
                if (FAILED(hr)) break;
            }
        }
        client->Stop();
        printf("[wasapi] 采集结束，推送 chunk 总数=%llu\n", (unsigned long long)pushed.load());
        ret = 0;
    }
cleanup:
    if (capture) capture->Release();
    if (client) client->Release();
    if (mix) CoTaskMemFree(mix);
    if (device) device->Release();
    if (enumerator) enumerator->Release();
    return ret;
}

// ---------------- 合成源（无麦/对比测试） ----------------
static void run_synthetic(void* engine, int seconds, bool realtime) {
    printf("[synthetic] 440Hz 正弦 %ds (%s)\n", seconds, realtime ? "实时节奏" : "突发");
    std::vector<int16_t> chunk(CHUNK_SAMPLES);
    uint64_t pushed = 0;
    double phase = 0.0;
    auto t_end = std::chrono::steady_clock::now() + std::chrono::seconds(seconds);
    auto t_mem = std::chrono::steady_clock::now();
    int mem_tick = 0;
    while (std::chrono::steady_clock::now() < t_end) {
        for (int i = 0; i < CHUNK_SAMPLES; i++) {
            phase += 2.0 * 3.14159265 * 440.0 / TARGET_SR;
            chunk[i] = (int16_t)(sin(phase) * 12000.0);
        }
        int32_t rc = voime_push_audio(engine, chunk.data(), CHUNK_SAMPLES);
        if (rc != 0 && rc != -2) printf("[synthetic] push rc=%d\n", rc);
        if (rc == 0) pushed++;
        if (realtime) Sleep(30);
        if (std::chrono::steady_clock::now() - t_mem > std::chrono::seconds(5)) {
            char tag[32]; snprintf(tag, sizeof(tag), "synthetic %ds", ++mem_tick * 5);
            log_mem(tag);
            t_mem = std::chrono::steady_clock::now();
        }
    }
    printf("[synthetic] 结束，推送 chunk 总数=%llu\n", (unsigned long long)pushed);
}

// ---------------- 溢出 / Finalizing 拒绝 确定性测试 ----------------
static void test_overflow(void* engine) {
    printf("\n=== 测试: 无锁队列溢出 -2 (Worker 延时 50ms + 突发推送) ===\n");
    voime_test_set_worker_delay(engine, 50);
    printf("[test] start_session rc=%d\n", voime_start_session(engine));
    std::vector<int16_t> chunk(CHUNK_SAMPLES, 1000);
    int ok = 0, overflow = 0;
    // 队列容量 2048 chunk；突发推 3000 必溢出
    for (int i = 0; i < 3000; i++) {
        int32_t rc = voime_push_audio(engine, chunk.data(), CHUNK_SAMPLES);
        if (rc == 0) ok++;
        else if (rc == -2) overflow++;
    }
    printf("[test] 推送 3000 chunk: ok=%d overflow(-2)=%d %s\n", ok, overflow,
           overflow > 0 ? "=> PASS" : "=> FAIL(未触发溢出)");
    voime_test_set_worker_delay(engine, 0);
    printf("[test] stop_session rc=%d (等待 Worker 排空)\n", voime_stop_session(engine));
}

static void test_finalizing_reject(void* engine) {
    printf("\n=== 测试: Finalizing 期间 start_session 返回 -3 ===\n");
    voime_test_set_worker_delay(engine, 200); // 拖慢 Worker, 保持 Finalizing 窗口
    printf("[test] start_session rc=%d\n", voime_start_session(engine));
    std::vector<int16_t> chunk(CHUNK_SAMPLES, 2000);
    for (int i = 0; i < 20; i++) voime_push_audio(engine, chunk.data(), CHUNK_SAMPLES);
    printf("[test] stop_session rc=%d\n", voime_stop_session(engine));
    int32_t rc = voime_start_session(engine);
    printf("[test] Finalizing 中再次 start_session rc=%d %s\n", rc,
           rc == -3 ? "=> PASS" : "=> (窗口已过, 非-3)");
    voime_test_set_worker_delay(engine, 0);
}

// ---------------- main ----------------
int main(int argc, char** argv) {
    int seconds = 15;
    std::string mode = "synthetic"; // synthetic | wasapi
    bool verbose = false, overflow_test = false, finalizing_test = false, realtime = true;
    for (int i = 1; i < argc; i++) {
        std::string a = argv[i];
        if (a == "--mode" && i + 1 < argc) mode = argv[++i];
        else if (a == "--seconds" && i + 1 < argc) seconds = atoi(argv[++i]);
        else if (a == "--verbose") verbose = true;
        else if (a == "--burst") realtime = false;
        else if (a == "--overflow-test") overflow_test = true;
        else if (a == "--finalizing-test") finalizing_test = true;
    }
    printf("=== Voime POC-06 C++/Rust FFI + WASAPI 验证 ===\n");
    printf("[cpp] rust 核心版本: %s\n", voime_version());

    CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    void* engine = voime_create_engine();
    printf("[cpp] engine=%p 创建成功\n", engine);
    HANDLE ev = CreateEventW(nullptr, FALSE /*auto-reset*/, FALSE, nullptr);
    voime_register_event(engine, ev);
    std::thread waiter(result_waiter, engine, ev);
    voime_set_verbose(engine, verbose ? 1 : 0);
    log_mem("engine created");

    if (overflow_test) test_overflow(engine);
    if (finalizing_test) test_finalizing_reject(engine);
    if (overflow_test || finalizing_test) {
        // 等 Worker 把测试数据排空、回到 Idle
        Sleep(1500);
    }

    printf("\n[cpp] start_session rc=%d (0=成功)\n", voime_start_session(engine));
    if (mode == "wasapi") {
        if (run_wasapi(engine, seconds) != 0) {
            printf("[cpp] WASAPI 不可用，回退合成源\n");
            run_synthetic(engine, seconds, realtime);
        }
    } else {
        run_synthetic(engine, seconds, realtime);
    }
    printf("[cpp] stop_session rc=%d\n", voime_stop_session(engine));

    // 等待 session_end 结果（简单等几秒让 waiter 打印）
    Sleep(2500);
    log_mem("after session");
    g_done.store(true);
    waiter.join();
    CloseHandle(ev);
    voime_destroy_engine(engine);
    log_mem("engine destroyed");
    CoUninitialize();
    printf("=== POC-06 运行结束 ===\n");
    return 0;
}
