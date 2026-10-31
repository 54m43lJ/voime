#include "config.h"

#include <algorithm>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <sstream>

#include <unistd.h>

#include <nlohmann/json.hpp>

namespace {

constexpr char ACTIVE_FILE[] = "active";
constexpr char MANIFEST_FILE[] = "manifest.json";
constexpr char LEGACY_ID[] = "legacy";

bool validPackageId(const std::string &id) {
    if (id.empty() || id == "." || id == "..") {
        return false;
    }
    if (id.find('/') != std::string::npos ||
        id.find('\\') != std::string::npos) {
        return false;
    }
    return std::none_of(id.begin(), id.end(), [](unsigned char ch) {
        return ch < 0x20 || ch == 0x7f;
    });
}

std::string trim(std::string value) {
    if (value.rfind("\xEF\xBB\xBF", 0) == 0) {
        value.erase(0, 3);
    }
    const auto first = value.find_first_not_of(" \t\r\n");
    if (first == std::string::npos) {
        return {};
    }
    const auto last = value.find_last_not_of(" \t\r\n");
    return value.substr(first, last - first + 1);
}

std::optional<std::string> readManifestLabel(const std::filesystem::path &dir,
                                             const std::string &id) {
    std::ifstream input(dir / MANIFEST_FILE);
    if (!input) {
        return std::nullopt;
    }
    try {
        nlohmann::json manifest;
        input >> manifest;
        std::string label = id;
        if (manifest.contains("display_name") &&
            manifest["display_name"].is_string()) {
            const auto value = manifest["display_name"].get<std::string>();
            if (!value.empty()) {
                label = value;
            }
        }
        std::string kind = "unknown";
        if (manifest.contains("asr") && manifest["asr"].is_object()) {
            const auto &asr = manifest["asr"];
            if (asr.contains("kind") && asr["kind"].is_string()) {
                kind = asr["kind"].get<std::string>();
            }
        }
        return label + "  (" + kind + ")";
    } catch (...) {
        return std::nullopt;
    }
}

} // namespace

std::string modelRootPath() {
    if (const char *xdg = std::getenv("XDG_DATA_HOME"); xdg && *xdg) {
        return (std::filesystem::path(xdg) / "voime" / "models").string();
    }
    if (const char *home = std::getenv("HOME"); home && *home) {
        return (std::filesystem::path(home) / ".local" / "share" / "voime" /
                "models")
            .string();
    }
    return "models";
}

std::vector<std::pair<std::string, std::string>> scanInstalledModels() {
    std::vector<std::pair<std::string, std::string>> models;
    const auto root = std::filesystem::path(modelRootPath());
    std::error_code error;
    if (std::filesystem::is_regular_file(root / MANIFEST_FILE, error)) {
        if (auto label = readManifestLabel(root, LEGACY_ID)) {
            models.emplace_back(LEGACY_ID, *label);
        }
    }
    if (!std::filesystem::is_directory(root, error)) {
        return models;
    }
    for (const auto &entry : std::filesystem::directory_iterator(root, error)) {
        if (error || !entry.is_directory()) {
            continue;
        }
        const auto id = entry.path().filename().string();
        if (id == LEGACY_ID || !validPackageId(id) ||
            !std::filesystem::is_regular_file(entry.path() / MANIFEST_FILE)) {
            continue;
        }
        if (auto label = readManifestLabel(entry.path(), id)) {
            models.emplace_back(id, *label);
        }
    }
    std::sort(models.begin(), models.end(), [](const auto &left,
                                               const auto &right) {
        return left.first < right.first;
    });
    return models;
}

std::optional<std::string> readActiveModel() {
    std::ifstream input(std::filesystem::path(modelRootPath()) / ACTIVE_FILE);
    if (!input) {
        return std::nullopt;
    }
    std::stringstream buffer;
    buffer << input.rdbuf();
    const auto line = trim(buffer.str());
    if (line.empty()) {
        return std::nullopt;
    }
    if (line.find_first_of("\r\n") != std::string::npos ||
        !validPackageId(line)) {
        return std::nullopt;
    }
    return line;
}

bool writeActiveModel(const std::string &packageId) {
    if (!validPackageId(packageId)) {
        return false;
    }
    const auto models = scanInstalledModels();
    const bool installed =
        std::any_of(models.begin(), models.end(),
                    [&](const auto &model) { return model.first == packageId; });
    if (!installed) {
        return false;
    }
    std::error_code error;
    const auto root = std::filesystem::path(modelRootPath());
    std::filesystem::create_directories(root, error);
    if (error) {
        return false;
    }
    const auto target = root / ACTIVE_FILE;
    const auto temporary =
        root / ("." + std::string(ACTIVE_FILE) + ".tmp-" +
                std::to_string(::getpid()));
    {
        std::ofstream output(temporary, std::ios::trunc);
        if (!output) {
            return false;
        }
        output << packageId << '\n';
        if (!output) {
            std::filesystem::remove(temporary, error);
            return false;
        }
    }
    std::filesystem::rename(temporary, target, error);
    if (error) {
        std::filesystem::remove(temporary, error);
        return false;
    }
    return true;
}

const char *modeName(Mode mode) {
    switch (mode) {
    case Mode::Ptt:
        return "ptt";
    case Mode::Toggle:
        return "toggle";
    case Mode::Continuous:
        return "continuous";
    }
    return "ptt";
}

std::string hotkeyString(const fcitx::KeyList &keys) {
    std::string result;
    for (const auto &key : keys) {
        if (!result.empty()) {
            result += ",";
        }
        result += key.toString();
    }
    return result;
}
