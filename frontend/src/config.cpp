// config.cpp — TOML 子集解析（仅支持 [section]、key = "str" / key = int / 注释）
#include "config.h"
#include "util.h"
#include <cstdio>
#include <cstdlib>
#include <map>

static const char* kDefaultFfmpegUrl =
    "https://github.com/GyanD/codexffmpeg/releases/download/6.0/ffmpeg-6.0-full_build-shared.7z";
static const char* kDefaultModelsUrl = "https://cdn.example.com/voime/models/"; // 占位（阶段四）

std::wstring config_path() {
    wchar_t* appdata = nullptr;
    size_t len = 0;
    std::wstring base;
    if (_wdupenv_s(&appdata, &len, L"APPDATA") == 0 && appdata) {
        base = appdata;
        free(appdata);
    } else {
        base = L".";
    }
    return base + L"\\VoimeInput\\config.toml";
}

static std::string trim(const std::string& s) {
    size_t a = s.find_first_not_of(" \t\r\n");
    if (a == std::string::npos) return "";
    size_t b = s.find_last_not_of(" \t\r\n");
    return s.substr(a, b - a + 1);
}

static void write_default(const std::wstring& path) {
    // 确保目录存在（父目录 %APPDATA% 必存在，单层创建即可）
    std::wstring dir = path.substr(0, path.find_last_of(L"\\/"));
    CreateDirectoryW(dir.c_str(), nullptr);
    FILE* f = _wfopen(path.c_str(), L"wb");
    if (!f) return;
    std::string text =
        "# Voime 配置（PRD 5.2）\n"
        "[general]\n"
        "hotkey = \"Ctrl+Space\"  # PTT 快捷键\n"
        "lang = \"auto\"          # auto|zh|en 语言提示\n"
        "\n"
        "[audio]\n"
        "sample_rate = 16000\n"
        "channels = 1\n"
        "\n"
        "[runtime]\n"
        "ffmpeg_version = \"6.0\"  # 硬编码 FFmpeg 版本，用于依赖校验\n"
        "ffmpeg_url = \"" + std::string(kDefaultFfmpegUrl) + "\"\n"
        "\n"
        "[models]\n"
        "download_url = \"" + std::string(kDefaultModelsUrl) + "\"\n";
    fwrite(text.data(), 1, text.size(), f);
    fclose(f);
}

Config load_config(const std::wstring& path, bool& created) {
    Config cfg;
    cfg.ffmpeg_url = kDefaultFfmpegUrl;
    cfg.models_url = kDefaultModelsUrl;
    created = false;

    FILE* f = _wfopen(path.c_str(), L"rb");
    if (!f) {
        write_default(path);
        created = true;
        return cfg;
    }
    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    std::string buf(sz > 0 ? (size_t)sz : 0, '\0');
    if (sz > 0) fread(buf.data(), 1, (size_t)sz, f);
    fclose(f);

    std::string section;
    size_t pos = 0;
    while (pos < buf.size()) {
        size_t eol = buf.find('\n', pos);
        if (eol == std::string::npos) eol = buf.size();
        std::string line = trim(buf.substr(pos, eol - pos));
        pos = eol + 1;
        if (line.empty() || line[0] == '#') continue;
        if (line[0] == '[') {
            size_t rb = line.find(']');
            if (rb != std::string::npos) section = trim(line.substr(1, rb - 1));
            continue;
        }
        // 去掉行内注释（引号外的 #）
        std::string stripped;
        bool in_q = false;
        for (char c : line) {
            if (c == '"') in_q = !in_q;
            if (c == '#' && !in_q) break;
            stripped += c;
        }
        size_t eq = stripped.find('=');
        if (eq == std::string::npos) continue;
        std::string key = trim(stripped.substr(0, eq));
        std::string val = trim(stripped.substr(eq + 1));
        if (val.size() >= 2 && val.front() == '"' && val.back() == '"')
            val = val.substr(1, val.size() - 2);

        if (section == "general" && key == "hotkey") cfg.hotkey = utf8_to_wide(val);
        else if (section == "general" && key == "lang") cfg.lang = val;
        else if (section == "runtime" && key == "ffmpeg_version") cfg.ffmpeg_version = val;
        else if (section == "runtime" && key == "ffmpeg_url") cfg.ffmpeg_url = val;
        else if (section == "models" && key == "download_url") cfg.models_url = val;
    }
    return cfg;
}

int lang_hint_from_string(const std::string& s) {
    if (s == "zh") return 1;
    if (s == "en") return 2;
    return 0;
}
