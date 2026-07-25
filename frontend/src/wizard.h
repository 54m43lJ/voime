// wizard.h — 首启模型下载向导（M4 交付物，PRD 5.3 按需下载）
//
// 简洁模态窗口：提示页（开始下载/退出）-> 下载页（进度条 + 状态 + 取消）
// -> 完成自动关闭 / 失败可重试。下载在后台线程，进度经 PostMessage 更新。
#pragma once
#include <string>
#include <windows.h>

namespace wizard {

enum Result {
    Ok = 0,      // 模型全部就绪（下载完成或本就完整）
    Cancelled = 1, // 用户取消/关闭（调用方可选择继续无模型运行）
    Failed = 2,    // 下载或校验失败且用户选择退出
};

// base_url: 模型下载基地址（config [models] download_url）
// dir:      模型目标目录（%APPDATA%\VoimeInput\models）
// auto_start: true 时跳过提示页直接开始下载（QA 自动化/无人值守首启）
Result run(HINSTANCE inst, const std::string& base_url, const std::wstring& dir, bool auto_start);

} // namespace wizard
