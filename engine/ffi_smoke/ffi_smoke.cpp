// 阶段二 M2 冒烟：C++ 经 voime.h 调用 voime_core.dll。
// 合成 30ms 正弦 chunk 推送 3s，验证版本/就绪/start/push/事件/final/释放/销毁。
#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <cmath>
#include <cstring>
#include <string>
#include <thread>
#include <atomic>
#include "../include/voime.h"

static std::atomic<bool> g_got_final{false};

int main() {
    printf("=== voime_core.dll C++ 冒烟 ===\n");
    printf("[cpp] version: %s\n", voime_version());

    void* eng = voime_create_engine();
    if (!eng) { printf("[FAIL] create_engine NULL\n"); return 1; }
    int32_t ready = voime_model_ready(eng);
    printf("[cpp] model_ready=%d\n", ready);
    if (ready != 1) { printf("[FAIL] 模型未就绪\n"); return 1; }

    HANDLE ev = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    voime_register_event(eng, ev);

    std::atomic<bool> done{false};
    std::thread waiter([&]() {
        while (!done.load() || !g_got_final.load()) {
            if (WaitForSingleObject(ev, 200) != WAIT_OBJECT_0) continue;
            for (;;) {
                uint32_t len = 0;
                const char* s = voime_poll_result(eng, &len);
                if (!s) break;
                printf("[result] %s\n", s);
                if (strstr(s, "\"type\":\"final\"")) g_got_final.store(true);
                voime_free_string(s);
            }
        }
    });

    printf("[cpp] start_session rc=%d\n", voime_start_session(eng));
    // 3s 正弦波，30ms 一块，实时节奏
    int16_t chunk[480];
    double phase = 0.0;
    for (int i = 0; i < 100; i++) {
        for (int j = 0; j < 480; j++) {
            phase += 2.0 * 3.14159265 * 440.0 / 16000.0;
            chunk[j] = (int16_t)(sin(phase) * 10000.0);
        }
        int32_t rc = voime_push_audio(eng, chunk, 480);
        if (rc != 0) printf("[cpp] push rc=%d @%d\n", rc, i);
        Sleep(30);
    }
    printf("[cpp] stop_session rc=%d\n", voime_stop_session(eng));
    // 等 final（正弦波无语音，VAD 不产生句段，final 文本为空——验证流程完整性）
    WaitForSingleObject(ev, 8000);
    Sleep(500);
    done.store(true);
    waiter.join();
    CloseHandle(ev);
    voime_destroy_engine(eng);
    printf("[cpp] destroy_engine OK, got_final=%d\n", g_got_final.load());
    printf("=== SMOKE %s ===\n", g_got_final.load() ? "PASS" : "FAIL");
    return g_got_final.load() ? 0 : 1;
}
