// main.cpp — Voime 前端入口
//
// 两个构建产物（同一源码）：
//   voime_input.exe         GUI 子系统（产品形态：托盘 + PTT 悬浮窗）
//   voime_input_console.exe 控制台子系统（开发/自测：默认进入 --selftest 也需显式传参）
//
// 参数：
//   --selftest            控制台自测（CPP-01~08 自动化验收）
//   --repair              FFmpeg 依赖修复专项自测
//   --wav <path>          指定自测用 wav（16k s16 mono）
//   --auto-download-models 模型缺失时跳过提示直接下载（QA 自动化）
//   --audio-source <wav>  用 WAV 音源替代麦克风（QA 全链路/护栏测试）
//   --wav-loop            WAV 音源循环播放（60s 护栏测试）
//   --models-url <url>    覆盖模型下载基地址（本地 HTTP 服务器模拟 CDN）
#include "app.h"
#include "selftest.h"
#include "util.h"

#include <windows.h>
#include <shellapi.h>
#include <cstdio>
#include <string>
#include <vector>

static int dispatch(HINSTANCE inst, const std::vector<std::wstring>& args) {
    bool selftest = false, repair = false;
    int soak = 0;
    std::wstring wav = L"..\\poc\\python\\out\\session_zh_3sent.wav";
    AppOptions opts;
    for (size_t i = 0; i < args.size(); i++) {
        if (args[i] == L"--selftest") selftest = true;
        else if (args[i] == L"--repair") repair = true;
        else if (args[i] == L"--soak" && i + 1 < args.size()) soak = _wtoi(args[++i].c_str());
        else if (args[i] == L"--wav" && i + 1 < args.size()) wav = args[++i];
        else if (args[i] == L"--auto-download-models") opts.auto_download_models = true;
        else if (args[i] == L"--audio-source" && i + 1 < args.size()) opts.audio_wav = args[++i];
        else if (args[i] == L"--wav-loop") opts.wav_loop = true;
        else if (args[i] == L"--models-url" && i + 1 < args.size())
            opts.models_url = wide_to_utf8(args[++i]);
    }
    if (repair) return run_repair_selftest();
    if (soak > 0) return run_soak_selftest(soak, wav);
    if (selftest) return run_selftest(wav);

    // DPI 感知（悬浮窗坐标正确）
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    App app;
    return app.run(inst, opts);
}

#ifdef VOIME_CONSOLE_MAIN
// 控制台子系统入口（自测/调试用）
int wmain(int argc, wchar_t** argv) {
    SetConsoleOutputCP(65001);
    SetConsoleCP(65001);
    std::vector<std::wstring> args;
    for (int i = 1; i < argc; i++) args.emplace_back(argv[i]);
    return dispatch(GetModuleHandleW(nullptr), args);
}
#else
// GUI 子系统入口（产品形态）
int WINAPI wWinMain(HINSTANCE inst, HINSTANCE, PWSTR, int) {
    int argc = 0;
    LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    std::vector<std::wstring> args;
    for (int i = 1; i < argc; i++) args.emplace_back(argv[i]);
    LocalFree(argv);
    return dispatch(inst, args);
}
#endif
