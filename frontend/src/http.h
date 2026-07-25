// http.h — WinHTTP 下载公共封装（HTTPS、跟随重定向、进度回调）
#pragma once
#include <cstdint>
#include <functional>
#include <string>

// 下载 url 到 out_path；progress(已下载字节) 周期性回调（可为空）。
// log 累积过程输出；成功返回 true。
bool http_download(const std::string& url, const std::wstring& out_path,
                   std::function<void(uint64_t done)> progress, std::string& log);
