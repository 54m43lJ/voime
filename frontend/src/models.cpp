// models.cpp — 模型下载管理器实现
#include "models.h"
#include "config.h"
#include "http.h"
#include "util.h"

#include <windows.h>
#include <bcrypt.h>
#include <cstdio>

// 清单对齐 engine/voime_core/src/models.rs 的 8 个文件（SHA-256 为发布基线）
static const std::vector<models::FileEntry> kManifest = {
    {"FireRedVAD-ONNX/fireredvad_stream_vad_with_cache.onnx", 2306042,
     "b3c97836130dc34fc32d56fab551e88cf9454511de2b3c250a5e6578dee74b93"},
    {"FireRedVAD-ONNX/cmvn.ark", 1311,
     "c87f6f13edf0f0ec7535ddfc9cc3387d9268cb234b70182d566c5e2edf3ca473"},
    {"FireRedASR2-AED-CTC-ONNX/model.int8.onnx", 775861420,
     "ca3dbabd82170110cc0b343c2890866d449984bc9cd92b9a18371ff80a81bb99"},
    {"FireRedASR2-AED-CTC-ONNX/tokens.txt", 79172,
     "1bc613de2112d257e61a349c3e72d1b1a9cf19c33d3ca954197ad2171e5ea07b"},
    {"FireRedASR2-AED-CTC-ONNX/cmvn.ark", 1311,
     "6efba6105429d1630c05d818d956bfe4edfad37a04b3b27bb5a029b9adb37945"},
    {"FireRedPunc-ONNX/punc_bert.onnx", 406945955,
     "bf0e5946347e795043c333895e61bbcbb3d9570a549cf0d031ed4f5c8976e0cf"},
    {"FireRedPunc-ONNX/tokenizer.json", 268961,
     "53ff61207898738bbdc000f38abebef01041c8d23b6270c11855fc692d0a3ad6"},
    {"FireRedPunc-ONNX/out_dict", 33,
     "6f0f7e0004881d617bc6e1d7b5b39972da80dcb49576bca489b1603ee55e20bb"},
};

const std::vector<models::FileEntry>& models::manifest() { return kManifest; }

uint64_t models::manifest_total_bytes() {
    uint64_t t = 0;
    for (auto& e : kManifest) t += e.size;
    return t;
}

std::wstring models::models_dir() {
    std::wstring cfg = config_path(); // %APPDATA%\VoimeInput\config.toml
    std::wstring dir = cfg.substr(0, cfg.find_last_of(L"\\/")) + L"\\models";
    CreateDirectoryW(dir.c_str(), nullptr);
    return dir;
}

static std::wstring full_path(const std::wstring& dir, const char* rel) {
    std::wstring w = utf8_to_wide(rel);
    for (auto& c : w) if (c == L'/') c = L'\\';
    return dir + L"\\" + w;
}

std::vector<models::FileEntry> models::check(const std::wstring& dir) {
    std::vector<FileEntry> bad;
    for (auto& e : kManifest) {
        std::wstring p = full_path(dir, e.rel);
        WIN32_FILE_ATTRIBUTE_DATA ad{};
        if (!GetFileAttributesExW(p.c_str(), GetFileExInfoStandard, &ad) ||
            ((uint64_t)ad.nFileSizeHigh << 32 | ad.nFileSizeLow) != e.size) {
            bad.push_back(e);
        }
    }
    return bad;
}

bool models::sha256_file(const std::wstring& path, std::string& out_hex) {
    BCRYPT_ALG_HANDLE alg = nullptr;
    BCRYPT_HASH_HANDLE h = nullptr;
    bool ok = false;
    if (BCryptOpenAlgorithmProvider(&alg, BCRYPT_SHA256_ALGORITHM, nullptr, 0) != 0) return false;
    DWORD obj_len = 0, tmp = 0;
    BCryptGetProperty(alg, BCRYPT_OBJECT_LENGTH, (PUCHAR)&obj_len, sizeof(obj_len), &tmp, 0);
    std::vector<uint8_t> obj(obj_len);
    if (BCryptCreateHash(alg, &h, obj.data(), obj_len, nullptr, 0, 0) == 0) {
        FILE* f = _wfopen(path.c_str(), L"rb");
        if (f) {
            uint8_t buf[1 << 16];
            size_t n;
            bool read_ok = true;
            while ((n = fread(buf, 1, sizeof(buf), f)) > 0) {
                if (BCryptHashData(h, buf, (ULONG)n, 0) != 0) { read_ok = false; break; }
            }
            fclose(f);
            if (read_ok) {
                uint8_t digest[32];
                if (BCryptFinishHash(h, digest, sizeof(digest), 0) == 0) {
                    static const char* hex = "0123456789abcdef";
                    out_hex.clear();
                    for (auto b : digest) {
                        out_hex += hex[b >> 4];
                        out_hex += hex[b & 15];
                    }
                    ok = true;
                }
            }
        }
        BCryptDestroyHash(h);
    }
    BCryptCloseAlgorithmProvider(alg, 0);
    return ok;
}

bool models::download(const std::string& base_url, const std::wstring& dir,
                      std::function<void(uint64_t, uint64_t, const std::string&)> progress,
                      std::atomic<bool>& cancel, std::string& err) {
    auto bad = check(dir);
    if (bad.empty()) return true;
    uint64_t total = 0, done_before = 0;
    for (auto& e : bad) total += e.size;

    std::string base = base_url;
    while (!base.empty() && base.back() == '/') base.pop_back();

    for (auto& e : bad) {
        if (cancel) { err = "用户取消"; return false; }
        std::wstring dst = full_path(dir, e.rel);
        std::wstring part = dst + L".part";
        // 确保子目录存在
        std::wstring sub = dst.substr(0, dst.find_last_of(L"\\/"));
        CreateDirectoryW(sub.c_str(), nullptr);

        std::string url = base + "/" + e.rel;
        std::string hlog;
        uint64_t base_done = done_before;
        auto file_progress = [&](uint64_t file_done) {
            if (progress) progress(base_done + file_done, total, e.rel);
        };
        // 下载（支持 http:// 本地源与 https:// CDN）
        DeleteFileW(part.c_str());
        if (!http_download(url, part, file_progress, hlog)) {
            err = "下载失败 " + std::string(e.rel) + ": " + hlog;
            return false;
        }
        // 校验：大小 + SHA-256
        WIN32_FILE_ATTRIBUTE_DATA ad{};
        uint64_t sz = 0;
        if (GetFileAttributesExW(part.c_str(), GetFileExInfoStandard, &ad))
            sz = ((uint64_t)ad.nFileSizeHigh << 32 | ad.nFileSizeLow);
        if (sz != e.size) {
            DeleteFileW(part.c_str());
            err = "大小校验失败 " + std::string(e.rel);
            return false;
        }
        std::string hash;
        if (!sha256_file(part, hash) || hash != e.sha256) {
            DeleteFileW(part.c_str());
            err = "SHA-256 校验失败 " + std::string(e.rel);
            return false;
        }
        DeleteFileW(dst.c_str());
        if (!MoveFileW(part.c_str(), dst.c_str())) {
            err = "就位失败 " + std::string(e.rel);
            return false;
        }
        done_before += e.size;
        if (progress) progress(done_before, total, e.rel);
    }
    return true;
}
