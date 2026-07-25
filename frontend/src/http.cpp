// http.cpp — WinHTTP 下载公共封装实现
#include "http.h"
#include "util.h"

#include <windows.h>
#include <winhttp.h>
#include <cstdio>

bool http_download(const std::string& url, const std::wstring& out_path,
                   std::function<void(uint64_t done)> progress, std::string& log) {
    // 解析 URL（仅支持 https://host/path 形式）
    URL_COMPONENTSW uc{};
    wchar_t host[256]{}, path[2048]{};
    uc.dwStructSize = sizeof(uc);
    uc.lpszHostName = host; uc.dwHostNameLength = 255;
    uc.lpszUrlPath = path; uc.dwUrlPathLength = 2047;
    std::wstring wurl = utf8_to_wide(url);
    if (!WinHttpCrackUrl(wurl.c_str(), 0, 0, &uc)) {
        log += "[http] URL 解析失败: " + url + "\n";
        return false;
    }
    HINTERNET ses = WinHttpOpen(L"VoimeInput/1.0", WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                                WINHTTP_NO_PROXY_NAME, WINHTTP_NO_PROXY_BYPASS, 0);
    if (!ses) { log += "[http] WinHttpOpen 失败\n"; return false; }
    bool ok = false;
    HINTERNET con = WinHttpConnect(ses, host, uc.nPort, 0);
    if (con) {
        HINTERNET req = WinHttpOpenRequest(con, L"GET", path, nullptr, WINHTTP_NO_REFERER,
                                           WINHTTP_DEFAULT_ACCEPT_TYPES,
                                           uc.nScheme == INTERNET_SCHEME_HTTPS ? WINHTTP_FLAG_SECURE : 0);
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
                            if (progress) progress(total);
                        }
                        fclose(f);
                        log += "[http] 下载 " + std::to_string(total / 1048576) + " MB -> " +
                               wide_to_utf8(out_path) + "\n";
                        if (!ok) { // 中途断流：删除残件
                            DeleteFileW(out_path.c_str());
                            log += "[http] 传输中断，已删除残件\n";
                        }
                    } else {
                        log += "[http] 无法写入: " + wide_to_utf8(out_path) + "\n";
                    }
                } else {
                    log += "[http] HTTP 状态码 " + std::to_string(status) + "\n";
                }
            } else {
                log += "[http] 请求发送/响应失败, GetLastError=" + std::to_string(GetLastError()) + "\n";
            }
            WinHttpCloseHandle(req);
        }
        WinHttpCloseHandle(con);
    }
    WinHttpCloseHandle(ses);
    return ok;
}
