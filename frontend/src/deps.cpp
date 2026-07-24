// deps.cpp — FFmpeg 依赖自检与下载修复实现
#include "deps.h"
#include "resampler.h"
#include "util.h"

#include <windows.h>
#include <winhttp.h>
#include <cstdio>

// FFmpeg 版本 -> swresample 主版本映射（PRD 硬编码 6.0 -> swr 4）
static int swr_major_for(const std::string& ffmpeg_version) {
    if (ffmpeg_version == "6.0") return 4;
    if (ffmpeg_version == "7.0") return 5;
    if (ffmpeg_version == "5.1") return 3;
    return 4;
}

deps::CheckResult deps::check_ffmpeg(const std::string& expect_ffmpeg_version) {
    CheckResult r;
    std::wstring dir = exe_dir();
    const wchar_t* required[] = { L"swresample-4.dll", L"avutil-58.dll" };
    for (auto* name : required) {
        std::wstring full = dir + name;
        if (GetFileAttributesW(full.c_str()) == INVALID_FILE_ATTRIBUTES)
            r.missing_files.push_back(name);
    }
    if (!r.missing_files.empty()) {
        r.detail = "缺失 FFmpeg DLL: " + wide_to_utf8(r.missing_files[0]);
        if (r.missing_files.size() > 1)
            r.detail += " 等 " + std::to_string(r.missing_files.size()) + " 个文件";
        return r;
    }
    std::string d;
    if (!Resampler::available(swr_major_for(expect_ffmpeg_version), &d)) {
        r.detail = "DLL 存在但加载/版本校验失败: " + d;
        r.missing_files.push_back(L"(load)"); // 触发修复流程
        return r;
    }
    r.ok = true;
    r.detail = "FFmpeg 依赖正常: " + d + " (FFmpeg " + expect_ffmpeg_version + ")";
    return r;
}

// ---- WinHTTP 下载（跟随 302 重定向） ----
static bool http_download(const std::string& url, const std::wstring& out_path, std::string& log) {
    // 解析 URL（仅支持 https://host/path 形式）
    URL_COMPONENTSW uc{};
    wchar_t host[256]{}, path[2048]{};
    uc.dwStructSize = sizeof(uc);
    uc.lpszHostName = host; uc.dwHostNameLength = 255;
    uc.lpszUrlPath = path; uc.dwUrlPathLength = 2047;
    std::wstring wurl = utf8_to_wide(url);
    if (!WinHttpCrackUrl(wurl.c_str(), 0, 0, &uc)) {
        log += "[deps] URL 解析失败: " + url + "\n";
        return false;
    }
    HINTERNET ses = WinHttpOpen(L"VoimeInput/1.0", WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                                WINHTTP_NO_PROXY_NAME, WINHTTP_NO_PROXY_BYPASS, 0);
    if (!ses) { log += "[deps] WinHttpOpen 失败\n"; return false; }
    bool ok = false;
    HINTERNET con = WinHttpConnect(ses, host, uc.nPort, 0);
    if (con) {
        HINTERNET req = WinHttpOpenRequest(con, L"GET", path, nullptr, WINHTTP_NO_REFERER,
                                           WINHTTP_DEFAULT_ACCEPT_TYPES, WINHTTP_FLAG_SECURE);
        if (req) {
            DWORD policy = WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS;
            WinHttpSetOption(req, WINHTTP_OPTION_REDIRECT_POLICY, &policy, sizeof(policy));
            if (WinHttpSendRequest(req, WINHTTP_NO_ADDITIONAL_HEADERS, 0, WINHTTP_NO_REQUEST_DATA,
                                   0, 0, 0) &&
                WinHttpReceiveResponse(req, nullptr)) {
                DWORD status = 0, sz = sizeof(status);
                WinHttpQueryHeaders(req, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                                    WINHTTP_HEADER_NAME_BY_INDEX, &status, &sz,
                                    WINHTTP_NO_HEADER_INDEX);
                if (status == 200) {
                    FILE* f = _wfopen(out_path.c_str(), L"wb");
                    if (f) {
                        uint8_t buf[65536];
                        DWORD got = 0;
                        uint64_t total = 0;
                        for (;;) {
                            DWORD avail = 0;
                            if (!WinHttpQueryDataAvailable(req, &avail)) break;
                            if (avail == 0) { ok = true; break; }
                            DWORD want = avail > sizeof(buf) ? sizeof(buf) : avail;
                            if (!WinHttpReadData(req, buf, want, &got) || got == 0) break;
                            fwrite(buf, 1, got, f);
                            total += got;
                        }
                        fclose(f);
                        log += "[deps] 下载 " + std::to_string(total / 1048576) + " MB -> " +
                               wide_to_utf8(out_path) + "\n";
                    } else {
                        log += "[deps] 无法写入: " + wide_to_utf8(out_path) + "\n";
                    }
                } else {
                    log += "[deps] HTTP 状态码 " + std::to_string(status) + "\n";
                }
            } else {
                log += "[deps] 请求发送/响应失败, GetLastError=" + std::to_string(GetLastError()) + "\n";
            }
            WinHttpCloseHandle(req);
        }
        WinHttpCloseHandle(con);
    }
    WinHttpCloseHandle(ses);
    return ok;
}

// 递归查找文件，返回完整路径（未找到返回空）
static std::wstring find_file(const std::wstring& dir, const wchar_t* name) {
    WIN32_FIND_DATAW fd{};
    HANDLE h = FindFirstFileW((dir + L"\\*").c_str(), &fd);
    if (h == INVALID_HANDLE_VALUE) return L"";
    std::wstring hit;
    do {
        if (fd.cFileName[0] == L'.') continue;
        std::wstring full = dir + L"\\" + fd.cFileName;
        if (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) {
            hit = find_file(full, name);
        } else if (_wcsicmp(fd.cFileName, name) == 0) {
            hit = full;
        }
        if (!hit.empty()) break;
    } while (FindNextFileW(h, &fd));
    FindClose(h);
    return hit;
}

static void remove_tree(const std::wstring& dir) {
    std::wstring cmd = L"cmd.exe /c rmdir /s /q \"" + dir + L"\"";
    STARTUPINFOW si{ sizeof(si) };
    PROCESS_INFORMATION pi{};
    if (CreateProcessW(nullptr, cmd.data(), nullptr, nullptr, FALSE, CREATE_NO_WINDOW, nullptr,
                       nullptr, &si, &pi)) {
        WaitForSingleObject(pi.hProcess, 15000);
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
    }
}

bool deps::repair_ffmpeg(const std::string& url, std::string& log) {
    wchar_t tmp[MAX_PATH]{};
    GetTempPathW(MAX_PATH, tmp);
    std::wstring pkg = std::wstring(tmp) + L"voime_ffmpeg_pkg.7z";
    std::wstring xdir = std::wstring(tmp) + L"voime_ffmpeg_x";

    log += "[deps] 开始下载 FFmpeg 修复包...\n";
    if (!http_download(url, pkg, log)) {
        log += "[deps] 下载失败\n";
        return false;
    }
    // 系统 tar.exe（Win10 1903+ 自带 bsdtar，支持 7z 解压）
    remove_tree(xdir);
    CreateDirectoryW(xdir.c_str(), nullptr);
    std::wstring cmd = L"tar.exe -xf \"" + pkg + L"\" -C \"" + xdir + L"\"";
    STARTUPINFOW si{ sizeof(si) };
    PROCESS_INFORMATION pi{};
    DWORD exit_code = 1;
    if (CreateProcessW(nullptr, cmd.data(), nullptr, nullptr, FALSE, CREATE_NO_WINDOW, nullptr,
                       nullptr, &si, &pi)) {
        WaitForSingleObject(pi.hProcess, 120000);
        GetExitCodeProcess(pi.hProcess, &exit_code);
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
    }
    if (exit_code != 0) {
        log += "[deps] 解压失败(tar.exe 退出码 " + std::to_string(exit_code) + ")\n";
        return false;
    }
    // 提取所需 DLL 覆盖到 exe 目录
    const wchar_t* required[] = { L"swresample-4.dll", L"avutil-58.dll" };
    std::wstring dst_dir = exe_dir();
    bool all = true;
    for (auto* name : required) {
        std::wstring src = find_file(xdir, name);
        if (src.empty()) {
            log += "[deps] 包内未找到 " + wide_to_utf8(name) + "\n";
            all = false;
            continue;
        }
        if (!CopyFileW(src.c_str(), (dst_dir + name).c_str(), FALSE)) {
            log += "[deps] 覆盖 " + wide_to_utf8(name) + " 失败, GetLastError=" +
                   std::to_string(GetLastError()) + "\n";
            all = false;
        } else {
            log += "[deps] 已恢复 " + wide_to_utf8(name) + "\n";
        }
    }
    DeleteFileW(pkg.c_str());
    remove_tree(xdir);
    log += all ? "[deps] 修复完成，请重启应用\n" : "[deps] 修复未全部成功\n";
    return all;
}
