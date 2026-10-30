#include "voime.h"

#include <ctime>

#include <fcitx-config/iniparser.h>
#include <fcitx-utils/log.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addonmanager.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>

VoimeAddon::VoimeAddon(fcitx::Instance *instance)
    : instance_(instance), client_(instance) {
    reloadConfig();
    socketPath_ = defaultSocketPath();

    client_.onMessage = [this](const ServerMessage &message) {
        onIpcMessage(message);
    };
    client_.onDisconnected = [this]() { onDaemonDisconnected(); };

    keyWatcher_ = instance_->watchEvent<fcitx::EventType::InputContextKeyEvent>(
        fcitx::EventWatcherPhase::PreInputMethod,
        [this](fcitx::KeyEvent &event) { handleKey(event); });
    focusWatcher_ = instance_->watchEvent<fcitx::EventType::InputContextFocusOut>(
        fcitx::EventWatcherPhase::PreInputMethod,
        [this](fcitx::FocusOutEvent &event) {
            if (target_.get() == event.inputContext()) {
                preview_.clear(target_.get());
            }
        });

    reconnectTimer_ = instance_->eventLoop().addTimeEvent(
        CLOCK_MONOTONIC, fcitx::now(CLOCK_MONOTONIC) + 2000000, 0,
        [this](fcitx::EventSourceTime *, uint64_t) {
            if (!client_.connected()) {
                connectDaemon();
            }
            return true;
        });
    connectDaemon();
}

void VoimeAddon::keyEvent(const fcitx::InputMethodEntry &entry,
                          fcitx::KeyEvent &keyEvent) {
    FCITX_UNUSED(entry);
    FCITX_UNUSED(keyEvent);
}

void VoimeAddon::activate(const fcitx::InputMethodEntry &entry,
                          fcitx::InputContextEvent &event) {
    FCITX_UNUSED(entry);
    FCITX_UNUSED(event);
}

void VoimeAddon::deactivate(const fcitx::InputMethodEntry &entry,
                            fcitx::InputContextEvent &event) {
    FCITX_UNUSED(entry);
    if (target_.get() == event.inputContext()) {
        preview_.clear(target_.get());
    }
}

const fcitx::Configuration *VoimeAddon::getConfig() const { return &config_; }

void VoimeAddon::setConfig(const fcitx::RawConfig &config) {
    config_.load(config, true);
    fcitx::safeSaveAsIni(config_, "conf/voime");
    const auto current = currentConfig();
    client_.configure(current.mode, current.hotkey);
}

void VoimeAddon::reloadConfig() {
    fcitx::readAsIni(config_, "conf/voime");
    const auto current = currentConfig();
    client_.setConfig(current);
    if (client_.connected()) {
        client_.configure(current.mode, current.hotkey);
    }
}

IpcConfig VoimeAddon::currentConfig() const {
    return IpcConfig{
        config_.mode.value(),
        hotkeyString(config_.hotkey.value()),
    };
}

void VoimeAddon::connectDaemon() {
    if (client_.connected()) {
        return;
    }
    client_.setConfig(currentConfig());
    if (!client_.connect(socketPath_)) {
        FCITX_INFO() << "voime: waiting for voimed at " << socketPath_;
    }
}

void VoimeAddon::handleKey(fcitx::KeyEvent &event) {
    if (config_.mode.value() == Mode::Continuous) {
        return;
    }
    if (!client_.connected()) {
        return;
    }
    const bool matches = event.checkKeyList(
        config_.hotkey.value(),
        fcitx::KeyEventMatchingMode::MatchAllNormalizedKeys);
    if (config_.mode.value() == Mode::Ptt) {
        handlePtt(event, matches);
    } else {
        handleToggle(event, matches);
    }
}

void VoimeAddon::handlePtt(fcitx::KeyEvent &event, bool matches) {
    if (event.isRelease()) {
        if (holdActive_ && event.rawKey().code() == holdCode_) {
            holdActive_ = false;
            finalizing_ = recording_;
            recording_ = false;
            starting_ = false;
            client_.stop(segmentId_.value_or(0), TriggerSource::Hold);
            event.filterAndAccept();
        }
        return;
    }
    if (!matches) {
        return;
    }
    event.filterAndAccept();
    if (holdActive_ || starting_ || recording_ || finalizing_) {
        return;
    }
    auto *inputContext = event.inputContext();
    if (!inputContext) {
        return;
    }
    bindTarget(inputContext);
    holdCode_ = static_cast<uint32_t>(event.rawKey().code());
    holdActive_ = true;
    starting_ = true;
    client_.start(segmentId_.value_or(0), TriggerSource::Hold);
}

void VoimeAddon::handleToggle(fcitx::KeyEvent &event, bool matches) {
    if (event.isRelease()) {
        if (toggleCode_ != 0 &&
            event.rawKey().code() == static_cast<int>(toggleCode_)) {
            toggleCode_ = 0;
            event.filterAndAccept();
        }
        return;
    }
    if (!matches) {
        return;
    }
    const auto code = static_cast<uint32_t>(event.rawKey().code());
    if (toggleCode_ == code) {
        event.filterAndAccept();
        return;
    }
    event.filterAndAccept();
    toggleCode_ = code;
    if (!starting_ && !recording_ && !finalizing_) {
        auto *inputContext = event.inputContext();
        if (!inputContext) {
            return;
        }
        bindTarget(inputContext);
        starting_ = true;
        client_.start(segmentId_.value_or(0), TriggerSource::Toggle);
    } else if (recording_) {
        recording_ = false;
        finalizing_ = true;
        client_.stop(segmentId_.value_or(0), TriggerSource::Toggle);
    }
}

void VoimeAddon::bindTarget(fcitx::InputContext *inputContext) {
    target_.ref = inputContext->watch();
    target_.segment_id = nextLocalId_;
    segmentId_ = nextLocalId_++;
}

void VoimeAddon::onIpcMessage(const ServerMessage &message) {
    switch (message.type) {
    case ServerMessageType::SegmentStarted:
        if (message.segmentId) {
            segmentId_ = message.segmentId;
            target_.segment_id = *message.segmentId;
        }
        if (message.source != TriggerSource::Hold &&
            message.source != TriggerSource::Toggle) {
            auto *inputContext = instance_->mostRecentInputContext();
            if (inputContext && inputContext->hasFocus()) {
                target_.ref = inputContext->watch();
                target_.segment_id = message.segmentId.value_or(target_.segment_id);
            } else {
                target_.ref.unwatch();
            }
        }
        break;
    case ServerMessageType::State:
        recording_ = message.recording;
        finalizing_ = message.finalizing;
        if (recording_) {
            starting_ = false;
        }
        if (!recording_) {
            clearPreview();
        }
        if (!recording_ && !finalizing_) {
            starting_ = false;
            target_.ref.unwatch();
            segmentId_.reset();
        }
        break;
    case ServerMessageType::Partial:
        if (message.segmentId && segmentId_ &&
            *message.segmentId != *segmentId_) {
            break;
        }
        if (target_.focused()) {
            preview_.show(target_.get(), message.text);
        } else {
            clearPreview();
        }
        break;
    case ServerMessageType::Final: {
        if (message.segmentId && message.segmentId != target_.segment_id) {
            break;
        }
        auto *inputContext = target_.get();
        preview_.clear(inputContext);
        if (inputContext && inputContext->hasFocus() && !message.text.empty()) {
            inputContext->commitString(message.text);
        }
        target_.ref.unwatch();
        segmentId_.reset();
        starting_ = false;
        recording_ = false;
        finalizing_ = false;
        break;
    }
    case ServerMessageType::Error:
        FCITX_WARN() << "voime: daemon error " << message.errorCode << ": "
                     << message.errorMessage;
        if (!recording_) {
            starting_ = false;
            holdActive_ = false;
            toggleCode_ = 0;
            finalizing_ = false;
            target_.ref.unwatch();
            segmentId_.reset();
            clearPreview();
        }
        break;
    case ServerMessageType::Bye:
        client_.disconnect();
        break;
    case ServerMessageType::Snapshot:
    case ServerMessageType::HistoryCleared:
    case ServerMessageType::Status:
    case ServerMessageType::Unknown:
        break;
    }
}

void VoimeAddon::onDaemonDisconnected() {
    clearPreview();
    target_.ref.unwatch();
    segmentId_.reset();
    holdActive_ = false;
    starting_ = false;
    toggleCode_ = 0;
    recording_ = false;
    finalizing_ = false;
    FCITX_INFO() << "voime: daemon disconnected; hotkeys are passed through";
}

void VoimeAddon::clearPreview() {
    if (auto *inputContext = target_.get()) {
        preview_.clear(inputContext);
    }
}

class VoimeAddonFactory final : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new VoimeAddon(manager->instance());
    }
};

FCITX_ADDON_FACTORY_V2(voime, VoimeAddonFactory)
