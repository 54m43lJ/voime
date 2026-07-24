// config.h — TOML 子集配置（PRD 5.2）
#pragma once
#include <string>

struct Config {
    // [general]
    std::wstring hotkey = L"Ctrl+Space"; // PTT 快捷键
    std::string  lang   = "auto";        // auto|zh|en -> engine lang_hint 0/1/2
    // [runtime]
    std::string ffmpeg_version = "6.0";  // 硬编码 FFmpeg 版本（依赖校验）
    std::string ffmpeg_url;              // FFmpeg 修复包下载地址
    // [models]
    std::string models_url;              // 模型下载地址（阶段四向导使用）
};

// %APPDATA%\VoimeInput\config.toml
std::wstring config_path();

// 加载配置；文件缺失时写入默认配置并置 created=true
Config load_config(const std::wstring& path, bool& created);

// 语言提示映射: auto->0, zh->1, en->2
int lang_hint_from_string(const std::string& s);
