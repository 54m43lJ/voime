#include <cassert>
#include <cmath>
#include <iostream>

#include <fcitx-utils/key.h>

#include "config.h"
#include "ipc.h"
#include "target.h"

int main() {
    assert(std::string(modeName(Mode::Ptt)) == "ptt");
    assert(std::string(modeName(Mode::Toggle)) == "toggle");
    assert(std::string(modeName(Mode::Continuous)) == "continuous");

    fcitx::KeyList keys{fcitx::Key("Super+F2")};
    const auto text = hotkeyString(keys);
    assert(text.find("F2") != std::string::npos);

    VoimeConfig config;
    fcitx::RawConfig raw;
    config.save(raw);
    const auto *mode = raw.valueByPath("Mode");
    assert(mode && *mode == "ptt");
    config.load(raw, true);
    assert(config.mode.value() == Mode::Ptt);

    assert(std::string(triggerSourceName(TriggerSource::Hold)) == "hold");
    assert(parseTriggerSource("continuous") == TriggerSource::Continuous);
    assert(!parseTriggerSource("nope").has_value());

    TargetRef target;
    assert(!target.focused());
    assert(target.get() == nullptr);

    std::cout << "voime addon tests passed\n";
    return 0;
}
