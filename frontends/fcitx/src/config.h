#pragma once

#include <string>

#include <fcitx-config/configuration.h>
#include <fcitx-config/enum.h>
#include <fcitx-config/option.h>
#include <fcitx-utils/i18n.h>
#include <fcitx-utils/key.h>

enum class Mode { Ptt, Toggle, Continuous };
enum class Backend { Cpu, Gpu };

FCITX_CONFIG_ENUM_NAME(Mode, "ptt", "toggle", "continuous")
FCITX_CONFIG_ENUM_NAME(Backend, "cpu", "gpu")

FCITX_CONFIGURATION(
    VoimeConfig,
    fcitx::Option<Mode> mode{this, "Mode", _("Mode"), Mode::Ptt};
    fcitx::Option<Backend> backend{this, "Backend", _("Backend"),
                                   Backend::Cpu};
    fcitx::KeyListOption hotkey{this, "Hotkey", _("Hotkey"),
                                {fcitx::Key("Super+F2")},
                                fcitx::KeyListConstrain()};
);

const char *modeName(Mode mode);
const char *backendName(Backend backend);
std::string hotkeyString(const fcitx::KeyList &keys);
