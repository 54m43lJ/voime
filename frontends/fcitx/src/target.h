#pragma once

#include <cstdint>

#include <fcitx/inputcontext.h>
#include <fcitx-utils/trackableobject.h>

struct TargetRef {
    fcitx::TrackableObjectReference<fcitx::InputContext> ref;
    uint64_t segment_id = 0;

    bool focused() const {
        auto *ic = ref.get();
        return ic != nullptr && ic->hasFocus();
    }

    fcitx::InputContext *get() const { return ref.get(); }
};
