// injector.cpp — SendInput Unicode 逐字符注入实现
#include "injector.h"
#include <windows.h>
#include <vector>

namespace injector {

void inject_text(const std::wstring& text) {
    if (text.empty()) return;
    const size_t kBatch = 32; // 每批字符数（每字符 down+up 两个 INPUT）
    std::vector<INPUT> inputs;
    inputs.reserve(kBatch * 2);
    size_t pos = 0;
    while (pos < text.size()) {
        inputs.clear();
        size_t end = (pos + kBatch < text.size()) ? pos + kBatch : text.size();
        for (size_t i = pos; i < end; i++) {
            INPUT down{};
            down.type = INPUT_KEYBOARD;
            down.ki.wVk = 0;
            down.ki.wScan = text[i];
            down.ki.dwFlags = KEYEVENTF_UNICODE;
            inputs.push_back(down);
            INPUT up = down;
            up.ki.dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
            inputs.push_back(up);
        }
        SendInput((UINT)inputs.size(), inputs.data(), sizeof(INPUT));
        pos = end;
    }
}

} // namespace injector
