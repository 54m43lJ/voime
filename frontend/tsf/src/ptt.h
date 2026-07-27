// ptt.h — TSF 侧 PTT 组合键配置（与 Agent 共用 %APPDATA%\VoimeInput\config.toml）
#pragma once
#include <windows.h>

// 读取 config.toml 的 hotkey 字段并解析；文件缺失/解析失败时回退默认 Ctrl+Space。
// 返回 mods(MOD_* 组合) + vk；始终返回有效值。
void load_ptt(UINT& mods, UINT& vk);
