#pragma once

#include <cstdint>
#include <memory>
#include <optional>
#include <string>

#include <fcitx-utils/handlertable.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/instance.h>

#include "config.h"
#include "ipc.h"
#include "preview.h"
#include "target.h"

class VoimeAddon final : public fcitx::InputMethodEngineV2 {
public:
    explicit VoimeAddon(fcitx::Instance *instance);

    void keyEvent(const fcitx::InputMethodEntry &entry,
                  fcitx::KeyEvent &keyEvent) override;
    void activate(const fcitx::InputMethodEntry &entry,
                  fcitx::InputContextEvent &event) override;
    void deactivate(const fcitx::InputMethodEntry &entry,
                    fcitx::InputContextEvent &event) override;
    const fcitx::Configuration *getConfig() const override;
    void setConfig(const fcitx::RawConfig &config) override;
    void reloadConfig() override;

private:
    void handleKey(fcitx::KeyEvent &event);
    void handlePtt(fcitx::KeyEvent &event, bool matches);
    void handleToggle(fcitx::KeyEvent &event, bool matches);
    void onIpcMessage(const ServerMessage &message);
    void onDaemonDisconnected();
    void connectDaemon();
    void bindTarget(fcitx::InputContext *inputContext);
    void clearPreview();
    void refreshTogglePreview();
    IpcConfig currentConfig() const;

    fcitx::Instance *instance_;
    VoimeConfig config_;
    IpcClient client_;
    Preview preview_;
    TargetRef target_;
    std::unique_ptr<fcitx::HandlerTableEntry<fcitx::EventHandler>> keyWatcher_;
    std::unique_ptr<fcitx::HandlerTableEntry<fcitx::EventHandler>> focusWatcher_;
    std::unique_ptr<fcitx::EventSourceTime> reconnectTimer_;
    std::optional<uint64_t> segmentId_;
    bool holdActive_ = false;
    bool starting_ = false;
    bool recording_ = false;
    bool finalizing_ = false;
    bool toggleActive_ = false;
    uint32_t holdCode_ = 0;
    uint32_t toggleCode_ = 0;
    uint64_t nextLocalId_ = 1;
    std::string socketPath_;
};
