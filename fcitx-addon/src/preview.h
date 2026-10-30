#pragma once

#include <string>

namespace fcitx {
class InputContext;
}

class Preview {
public:
    void show(fcitx::InputContext *ic, const std::string &partial);
    void clear(fcitx::InputContext *ic);
};
