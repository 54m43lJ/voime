// main.cpp — Voime 前端入口
//
// 构建产物（同一源码，P5-06 工程配置分离）：
//   voime_input.exe         GUI 标准模式（TSF/IPC 架构；无热键/SendInput 代码）
//   voime_input_notsf.exe   GUI NOTSF 调试模式（本地热键 + SendInput，无 IPC）
//   voime_input_console.exe 控制台 NOTSF 模式（开发/自测：--selftest 等）
//   voime_tsf.dll           TSF 文本服务（frontend/tsf/ 单独构建）
//
// 参数：
//   --selftest            控制台自测（CPP-01~08 自动化验收，仅 NOTSF 控制台版）
//   --repair              FFmpeg 依赖修复专项自测（仅 NOTSF 控制台版）
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
#ifdef VOIME_NOTSF
    if (repair) return run_repair_selftest();
    if (soak > 0) return run_soak_selftest(soak, wav);
    if (selftest) return run_selftest(wav);
#else
    if (repair || soak > 0 || selftest) {
        // 标准版不含自测/修复逻辑（P5-06 无 SendInput/热键代码），请用 voime_input_console.exe
        fprintf(stderr, "selftest/repair/soak only available in voime_input_console.exe (NOTSF)\n");
        return 2;
    }
#endif

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
