// selftest.h — 控制台自测（CPP-01~04、07 的可自动化验收）
#pragma once
#include <string>

// 返回 0=全部通过（允许 SKIP），1=存在 FAIL
int run_selftest(const std::wstring& wav_path);

// CPP-04 修复流程专项（真实下载 ~40MB，独立触发）
int run_repair_selftest();

// QA-05 浸泡测试：sessions 次完整会话，内存稳定性 + 延迟统计
int run_soak_selftest(int sessions, const std::wstring& wav_path);
