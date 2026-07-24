// main.cpp — Voime 前端入口
//
// 两个构建产物（同一源码）：
//   voime_input.exe         GUI 子系统（产品形态：托盘 + PTT 悬浮窗）
//   voime_input_console.exe 控制台子系统（开发/自测：默认进入 --selftest 也需显式传参）
//
// 参数：
//   --selftest [wav]  控制台自测（CPP-01~04、07 自动化验收）
//   --wav <path>      指定自测用 wav（16k s16 mono）
#include "app.h"
#include "selftest.h"

#include <windows.h>
#include <shellapi.h>
#include <cstdio>
#include <string>
#include <vector>

static int dispatch(HINSTANCE inst, const std::vector<std::wstring>& args) {
    bool selftest = false, repair = false;
    std::wstring wav = L"..\\poc\\python\\out\\session_zh_3sent.wav";
    for (size_t i = 0; i < args.size(); i++) {
        if (args[i] == L"--selftest") selftest = true;
        else if (args[i] == L"--repair") repair = true;
        else if (args[i] == L"--wav" && i + 1 < args.size()) wav = args[++i];
    }
    if (repair) return run_repair_selftest();
    if (selftest) return run_selftest(wav);

    // DPI 感知（悬浮窗坐标正确）
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    App app;
    return app.run(inst, false);
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
