#pragma once

#include <optional>
#include <string>
#include <utility>
#include <vector>

#include <fcitx-config/configuration.h>
#include <fcitx-config/enum.h>
#include <fcitx-config/option.h>
#include <fcitx-utils/i18n.h>
#include <fcitx-utils/key.h>

enum class Mode { Ptt, Toggle, Continuous };

FCITX_CONFIG_ENUM_NAME(Mode, "ptt", "toggle", "continuous")

std::string modelRootPath();
std::vector<std::pair<std::string, std::string>> scanInstalledModels();
std::optional<std::string> readActiveModel();
bool writeActiveModel(const std::string &packageId);

/// Dynamic enum annotation backed by the installed model packages.
struct ModelEnumAnnotation : public fcitx::EnumAnnotation {
    void dumpDescription(fcitx::RawConfig &config) const {
        fcitx::EnumAnnotation::dumpDescription(config);
        size_t index = 0;
        for (const auto &[id, label] : scanInstalledModels()) {
            config.setValueByPath("Enum/" + std::to_string(index), id);
            config.setValueByPath("EnumI18n/" + std::to_string(index), label);
            ++index;
        }
    }
};

FCITX_CONFIGURATION(
    VoimeConfig,
    fcitx::Option<Mode> mode{this, "Mode", _("Mode"), Mode::Ptt};
    fcitx::KeyListOption hotkey{this, "Hotkey", _("Hotkey"),
                                {fcitx::Key("Super+F2")},
                                fcitx::KeyListConstrain()};
    fcitx::Option<std::string, fcitx::NoConstrain<std::string>,
                  fcitx::DefaultMarshaller<std::string>,
                  ModelEnumAnnotation>
        model{this, "Model", _("Model"), ""};
);

const char *modeName(Mode mode);
std::string hotkeyString(const fcitx::KeyList &keys);
