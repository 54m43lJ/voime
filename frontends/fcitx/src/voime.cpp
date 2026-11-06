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
            if (pendingTarget_) {
                // The target that lost focus belongs to the old segment.
                // Keep the pending target for the session that will start
                // after its Final.
                preview_.clear(target_.get());
                return;
            }
            if (holdActive_) {
                stopPtt();
            } else if (toggleActive_) {
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
    const Mode previous = config_.mode.value();
    config_.load(config, true);
    fcitx::safeSaveAsIni(config_, CONFIG_FILE);
    if (config_.mode.value() != previous) {
        // The old mode's hotkey state must not survive a configuration
        // change. Keep the target only while an old segment is still being
        // finalized so its Final can be committed.
        holdActive_ = false;
        toggleActive_ = false;
        starting_ = false;
        toggleCode_ = 0;
        holdCode_ = 0;
        if (!recording_ && !finalizing_) {
            target_.ref.unwatch();
            segmentId_.reset();
            clearPreview();
        }
    }
    const auto current = currentConfig();
    client_.setConfig(current);
    if (client_.connected()) {
        client_.configure(current.mode, current.hotkey);
    }
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
            stopPtt();
            event.filterAndAccept();
        }
        return;
    }
    if (!matches) {
        return;
    }
    event.filterAndAccept();
    if (holdActive_ || starting_ || recording_) {
        return;
    }
    auto *inputContext = event.inputContext();
    if (!inputContext) {
        return;
    }
    const bool startsDuringFinalize = finalizing_;
    if (startsDuringFinalize) {
        bindPendingTarget(inputContext);
    } else {
        bindTarget(inputContext);
    }
    holdCode_ = static_cast<uint32_t>(event.rawKey().code());
    holdActive_ = true;
    starting_ = true;
    client_.start(startsDuringFinalize ? pendingSegmentId_.value_or(0)
                                      : segmentId_.value_or(0),
                  TriggerSource::Hold);
}

void VoimeAddon::stopPtt() {
    if (!holdActive_) {
        return;
    }
    holdActive_ = false;
    starting_ = false;
    if (recording_) {
        finalizing_ = true;
    }
    recording_ = false;
    client_.stop(segmentId_.value_or(0), TriggerSource::Hold);
    if (!finalizing_) {
        clearPreview();
        target_.ref.unwatch();
        segmentId_.reset();
    }
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
    if (toggleActive_) {
        // A second press closes the session even while the previous segment
        // is still finalizing.
        stopToggle();
    } else if (!starting_ && !recording_) {
        auto *inputContext = event.inputContext();
        if (!inputContext) {
            return;
        }
        const bool startsDuringFinalize = finalizing_;
        if (startsDuringFinalize) {
            bindPendingTarget(inputContext);
        } else {
            bindTarget(inputContext);
        }
        toggleActive_ = true;
        starting_ = true;
        refreshTogglePreview();
        client_.start(startsDuringFinalize ? pendingSegmentId_.value_or(0)
                                          : segmentId_.value_or(0),
                      TriggerSource::Toggle);
    }
}

void VoimeAddon::stopToggle() {
    if (!toggleActive_) {
        return;
    }
    toggleActive_ = false;
    starting_ = false;
    if (recording_) {
        finalizing_ = true;
    }
    recording_ = false;
    // Finalizing keeps the last partial on screen until Final or a drop
    // result arrives. Only a session with no in-flight segment clears now.
    if (!finalizing_) {
        clearPreview();
        target_.ref.unwatch();
        segmentId_.reset();
    }
    // 0 lets the daemon stop whatever Toggle segment is current, even if
    // Final and the next SegmentStarted crossed on the wire.
    client_.stop(0, TriggerSource::Toggle);
}

void VoimeAddon::bindTarget(fcitx::InputContext *inputContext) {
    target_.ref = inputContext->watch();
    target_.segment_id = nextLocalId_;
    segmentId_ = nextLocalId_++;
}

void VoimeAddon::bindPendingTarget(fcitx::InputContext *inputContext) {
    TargetRef target;
    target.ref = inputContext->watch();
    target.segment_id = nextLocalId_;
    pendingSegmentId_ = nextLocalId_++;
    pendingTarget_ = target;
}

void VoimeAddon::onIpcMessage(const ServerMessage &message) {
    switch (message.type) {
    case ServerMessageType::SegmentStarted: {
        if (!message.segmentId) {
            break;
        }
        if (pendingTarget_ && !finalizing_) {
            // A Start sent while the previous segment was finalizing must not
            // replace the old target until the old Final has been committed.
            target_ = *pendingTarget_;
            pendingTarget_.reset();
            pendingSegmentId_.reset();
            segmentId_ = message.segmentId;
            target_.segment_id = *message.segmentId;
            if ((holdActive_ || toggleActive_) && !target_.focused()) {
                if (holdActive_) {
                    stopPtt();
                } else {
                    stopToggle();
                }
                return;
            }
            if (toggleActive_) {
                refreshTogglePreview();
            }
            break;
        }
        segmentId_ = message.segmentId;
        target_.segment_id = *message.segmentId;
        if (message.source == TriggerSource::Continuous) {
            auto *inputContext = instance_->mostRecentInputContext();
            if (inputContext && inputContext->hasFocus()) {
                target_.ref = inputContext->watch();
                target_.segment_id = *message.segmentId;
            } else {
                target_.ref.unwatch();
            }
        } else if (message.source == TriggerSource::Hold ||
                   message.source == TriggerSource::Toggle) {
            if ((holdActive_ || toggleActive_) && !target_.focused()) {
                // The original target is gone; a PTT / Toggle session must
                // end instead of committing into a different window.
                if (holdActive_) {
                    stopPtt();
                } else {
                    stopToggle();
                }
                return;
            }
        }
        if (toggleActive_) {
            refreshTogglePreview();
        }
        break;
    }
    case ServerMessageType::State:
        recording_ = message.recording;
        finalizing_ = message.finalizing;
        if (recording_) {
            starting_ = false;
        }
        if (!recording_ && !finalizing_) {
            starting_ = false;
            if (toggleActive_) {
                refreshTogglePreview();
            } else if (holdActive_) {
                // PTT keeps the target for the next soft-split segment but
                // clears the old partial.
                clearPreview();
            } else {
                clearPreview();
                target_.ref.unwatch();
                segmentId_.reset();
            }
        }
        break;
    case ServerMessageType::Partial:
        if (finalizing_) {
            break;
        }
        if (!message.segmentId || !segmentId_ ||
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
        if (!message.segmentId || !segmentId_ ||
            *message.segmentId != *segmentId_) {
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
            // Toggle keeps an empty window until the session is closed.
            refreshTogglePreview();
        } else if (holdActive_) {
            // PTT is still held; the next SegmentStarted will reuse the
            // session target.
            clearPreview();
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
        } else if (message.errorCode == "busy") {
            // A Start sent during Finalizing was rejected; drop its pending
            // target so it cannot leak into a later session.
            pendingTarget_.reset();
            pendingSegmentId_.reset();
            starting_ = false;
        } else if (message.errorCode == "segment_dropped") {
            if (!message.segmentId || !segmentId_ ||
                *message.segmentId != *segmentId_) {
                break;
            }
            starting_ = false;
            recording_ = false;
            finalizing_ = false;
            if (toggleActive_) {
                refreshTogglePreview();
            } else {
                clearPreview();
                if (!holdActive_) {
                    target_.ref.unwatch();
                    segmentId_.reset();
                }
            }
        }
        // Other codes are diagnostic only; they must not tear down an active
        // session.
        break;
    case ServerMessageType::Bye:
        client_.disconnect();
        break;
    case ServerMessageType::Snapshot:
        if (message.phase) {
            recording_ = *message.phase == Phase::Recording;
            finalizing_ = *message.phase == Phase::Finalizing;
            if (!recording_ && !finalizing_) {
                starting_ = false;
                if (toggleActive_) {
                    refreshTogglePreview();
                } else if (holdActive_) {
                    clearPreview();
                } else {
                    clearPreview();
                    target_.ref.unwatch();
                    segmentId_.reset();
                }
            }
        }
        break;
    case ServerMessageType::Status:
    case ServerMessageType::HistoryCleared:
    case ServerMessageType::Unknown:
        break;
    }
}

void VoimeAddon::onDaemonDisconnected() {
    clearPreview();
    target_.ref.unwatch();
    segmentId_.reset();
    pendingTarget_.reset();
    pendingSegmentId_.reset();
    holdActive_ = false;
    starting_ = false;
    toggleCode_ = 0;
    holdCode_ = 0;
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
