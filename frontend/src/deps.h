// deps.h — FFmpeg 依赖管理器（CPP-04，PRD 4.2 依赖管理器）
//
// 自检：启动时校验 exe 目录下 FFmpeg DLL（swresample-4.dll / avutil-58.dll）
//   的存在性、可加载性与版本匹配度（config [runtime] ffmpeg_version）。
// 修复：WinHTTP 下载官方指定版本 FFmpeg 包 -> 系统 tar.exe 解压 -> 覆盖本地 DLL。
#pragma once
#include <string>
#include <vector>

namespace deps {

struct CheckResult {
    bool ok = false;
    std::string detail;                       // 人类可读诊断信息
    std::vector<std::wstring> missing_files;  // 缺失/损坏的 DLL 文件名
};

// 自检：expect_ffmpeg_version 形如 "6.0"（映射 swresample 主版本 4）
CheckResult check_ffmpeg(const std::string& expect_ffmpeg_version);

// 修复：下载 url 指向的 FFmpeg 包，提取所需 DLL 覆盖到 exe 目录。
// log 累积过程输出；成功返回 true（调用方提示重启应用）。
bool repair_ffmpeg(const std::string& url, std::string& log);

} // namespace deps
