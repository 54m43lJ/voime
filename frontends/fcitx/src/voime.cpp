#include "voime.h"

#include <cstdlib>
#include <ctime>
#include <filesystem>

#include <fcitx-config/iniparser.h>
#include <fcitx-utils/log.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addonmanager.h>
#include <fcitx/event.h>
#include <fcitx/inputcontext.h>

namespace {

constexpr char CONFIG_FILE[] = "conf/voime.conf";
constexpr char LEGACY_CONFIG_FILE[] = "conf/voime";

std::filesystem::path userConfigPath(const char *relative) {
    std::filesystem::path base;
    if (const char *xdg = std::getenv("XDG_CONFIG_HOME"); xdg && *xdg) {
        base = xdg;
    } else if (const char *home = std::getenv("HOME"); home && *home) {
        base = std::filesystem::path(home) / ".config";
    } else {
        return {};
    }
    return base / "fcitx5" / relative;
}

void migrateLegacyConfig() {
    const auto current = userConfigPath(CONFIG_FILE);
    const auto legacy = userConfigPath(LEGACY_CONFIG_FILE);
    if (current.empty() || legacy.empty()) {
        return;
    }
    std::error_code error;
    if (std::filesystem::exists(current, error) ||
        !std::filesystem::exists(legacy, error)) {
        return;
    }
    VoimeConfig config;
    fcitx::readAsIni(config, LEGACY_CONFIG_FILE);
    fcitx::safeSaveAsIni(config, CONFIG_FILE);
    std::filesystem::remove(legacy, error);
    FCITX_INFO() << "voime: migrated legacy config " << legacy << " -> "
                 << current;
}

} // namespace

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
            if (target_.get() != event.inputContext()) {
                return;
            }
            if (toggleActive_) {
                // The input panel is hidden by Fcitx when the target loses
                // focus. Keeping Toggle recording would leave the daemon
                // listening without any visible session.
                stopToggle();
            } else {
                preview_.clear(target_.get());
            }
        });

    reconnectTimer_ = instance_->eventLoop().addTimeEvent(
        CLOCK_MONOTONIC, fcitx::now(CLOCK_MONOTONIC) + 2000000, 0,
        [this](fcitx::EventSourceTime *time, uint64_t) {
            // Re-arm explicitly: returning true alone is not a portable
            // periodic timer across Fcitx event loop backends.
            time->setNextInterval(2000000);
            time->setOneShot();
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
    fcitx::safeSaveAsIni(config_, CONFIG_FILE);
    const auto current = currentConfig();
    client_.configure(current.mode, current.hotkey);
}

void VoimeAddon::reloadConfig() {
    migrateLegacyConfig();
    fcitx::readAsIni(config_, CONFIG_FILE);
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
    if (!toggleActive_ && !starting_ && !recording_ && !finalizing_) {
        auto *inputContext = event.inputContext();
        if (!inputContext) {
            return;
        }
        bindTarget(inputContext);
        toggleActive_ = true;
        starting_ = true;
        refreshTogglePreview();
        client_.start(segmentId_.value_or(0), TriggerSource::Toggle);
    } else if (toggleActive_) {
        stopToggle();
    }
}

void VoimeAddon::stopToggle() {
    if (!toggleActive_) {
        return;
    }
    toggleActive_ = false;
    starting_ = false;
    finalizing_ = recording_;
    recording_ = false;
    clearPreview();
    // 0 lets the daemon stop whatever Toggle segment is current, even if
    // Final and the next SegmentStarted crossed on the wire.
    client_.stop(0, TriggerSource::Toggle);
    if (!finalizing_) {
        target_.ref.unwatch();
        segmentId_.reset();
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
        } else if (toggleActive_ &&
                   message.source == TriggerSource::Toggle &&
                   !target_.focused()) {
            // A toggle session may continue after a segment ended; follow the
            // current focus if the original input context disappeared. A
            // focus-out ends the session, so only live sessions may rebind.
            auto *inputContext = instance_->mostRecentInputContext();
            if (inputContext && inputContext->hasFocus()) {
                target_.ref = inputContext->watch();
                target_.segment_id = message.segmentId.value_or(target_.segment_id);
            } else {
                target_.ref.unwatch();
            }
        }
        if (toggleActive_) {
            refreshTogglePreview();
        }
        break;
    case ServerMessageType::State:
        recording_ = message.recording;
        finalizing_ = message.finalizing;
        if (recording_) {
            starting_ = false;
        }
        if (!recording_) {
            if (finalizing_) {
                clearPreview();
            } else if (toggleActive_) {
                refreshTogglePreview();
            } else {
                clearPreview();
            }
        }
        if (!recording_ && !finalizing_) {
            starting_ = false;
            if (!toggleActive_) {
                target_.ref.unwatch();
                segmentId_.reset();
            }
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
        starting_ = false;
        recording_ = false;
        finalizing_ = false;
        if (toggleActive_) {
            refreshTogglePreview();
        } else {
            target_.ref.unwatch();
            segmentId_.reset();
        }
        break;
    }
    case ServerMessageType::Error:
        FCITX_WARN() << "voime: daemon error " << message.errorCode << ": "
                     << message.errorMessage;
        if (message.errorCode == "role_conflict" ||
            message.errorCode == "version_mismatch" ||
            message.errorCode == "invalid_message") {
            // The daemon rejected the handshake/socket; retry from scratch.
            client_.disconnect();
        } else if (message.errorCode == "inference" && toggleActive_) {
            starting_ = false;
            recording_ = false;
            finalizing_ = false;
            refreshTogglePreview();
        } else if (!recording_) {
            toggleActive_ = false;
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
    toggleActive_ = false;
    FCITX_INFO() << "voime: daemon disconnected; hotkeys are passed through";
}

void VoimeAddon::clearPreview() {
    if (auto *inputContext = target_.get()) {
        preview_.clear(inputContext);
    }
}

void VoimeAddon::refreshTogglePreview() {
    if (!toggleActive_) {
        return;
    }
    if (target_.focused()) {
        preview_.showEmpty(target_.get());
    } else {
        clearPreview();
    }
}

class VoimeAddonFactory final : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override {
        return new VoimeAddon(manager->instance());
    }
};

FCITX_ADDON_FACTORY_V2(voime, VoimeAddonFactory)
