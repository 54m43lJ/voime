#include "diagnostics.h"

#include <system_error>

namespace {

std::filesystem::path firstExisting(const std::filesystem::path &primary,
                                    const std::filesystem::path &fallback) {
    std::error_code error;
    if (std::filesystem::is_regular_file(primary, error)) {
        return primary;
    }
    if (std::filesystem::is_regular_file(fallback, error)) {
        return fallback;
    }
    return primary;
}

} // namespace

AddonPaths systemAddonPaths() {
#ifndef VOIME_FCITX5_LIBDIR
#define VOIME_FCITX5_LIBDIR "/usr/lib"
#endif
    const auto library = firstExisting(
        std::filesystem::path(VOIME_FCITX5_LIBDIR) / "fcitx5" / "voime.so",
        "/usr/lib/fcitx5/voime.so");
    const auto description =
        firstExisting("/usr/share/fcitx5/addon/voime.conf",
                      "/usr/local/share/fcitx5/addon/voime.conf");
    return AddonPaths{library, description};
}

bool checkAddonAbi(std::string *error) {
    const auto paths = systemAddonPaths();
    std::error_code filesystemError;
    if (!std::filesystem::is_regular_file(paths.library, filesystemError)) {
        if (error) {
            *error = "voime.so 不存在：" + paths.library.string();
        }
        return false;
    }
    if (!std::filesystem::is_regular_file(paths.description, filesystemError)) {
        if (error) {
            *error = "addon 描述不存在：" + paths.description.string();
        }
        return false;
    }
    return true;
}
