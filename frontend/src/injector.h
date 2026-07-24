// injector.h — 模拟键盘文本注入（CPP-08，PRD 4.2 文本注入器）
//
// SendInput + KEYEVENTF_UNICODE 逐字符注入（含代理对），不污染剪贴板。
// 已知限制（PRD 2.4）：UIPI 隔离下无法注入管理员权限窗口，TSF 演进解决。
#pragma once
#include <string>

namespace injector {

// 将 UTF-16 文本注入当前焦点窗口
void inject_text(const std::wstring& text);

} // namespace injector
