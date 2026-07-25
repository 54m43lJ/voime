// models.h — 模型下载管理器（QA-03 / M4 首启模型下载向导，PRD 5.3）
//
// 内嵌 8 个模型文件的清单（相对路径/大小/SHA-256，对齐 engine models.rs 约定）；
// 下载后逐文件校验哈希，全部就绪后由调用方通知 Rust voime_load_models 热加载。
#pragma once
#include <atomic>
#include <cstdint>
#include <functional>
#include <string>
#include <vector>

namespace models {

struct FileEntry {
    const char* rel;   // 相对模型目录路径（'/' 分隔）
    uint64_t size;     // 期望字节数
    const char* sha256; // 期望 SHA-256（hex 小写）
};

const std::vector<FileEntry>& manifest();
uint64_t manifest_total_bytes();

// %APPDATA%\VoimeInput\models（不存在则创建）
std::wstring models_dir();

// 检查目录下模型完整性；返回缺失/尺寸不符的条目（quick 模式不验哈希）
std::vector<FileEntry> check(const std::wstring& dir);

// 下载缺失/损坏文件（.part 临时文件 + 校验通过后改名就位）。
// progress(已完成字节, 总字节, 当前文件相对路径)；cancel 置位即中断。
// err 填充失败原因；成功（全部就绪）返回 true。
bool download(const std::string& base_url, const std::wstring& dir,
              std::function<void(uint64_t done, uint64_t total, const std::string& file)> progress,
              std::atomic<bool>& cancel, std::string& err);

// 计算文件 SHA-256（hex 小写）
bool sha256_file(const std::wstring& path, std::string& out_hex);

} // namespace models
