#pragma once

#include <string>

namespace fcitx {
class InputContext;
}

class Preview {
public:
    void show(fcitx::InputContext *ic, const std::string &partial);
    void showEmpty(fcitx::InputContext *ic);
    void clear(fcitx::InputContext *ic);
};
