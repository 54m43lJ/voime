#include "config.h"

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
