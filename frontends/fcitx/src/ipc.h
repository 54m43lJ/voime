#pragma once

#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <string>

#include <fcitx-utils/event.h>

#include "config.h"

namespace fcitx {
class Instance;
}

enum class TriggerSource { Hold, Toggle, Tui, Cli, Continuous };
enum class Phase { Idle, Recording, Finalizing };

struct IpcConfig {
    Mode mode = Mode::Ptt;
    std::string hotkey;
};

enum class ServerMessageType {
    Snapshot,
    SegmentStarted,
    State,
    Partial,
    Final,
    HistoryCleared,
    Status,
    Error,
    Bye,
    Unknown,
};

struct ServerMessage {
    ServerMessageType type = ServerMessageType::Unknown;
    std::optional<uint64_t> segmentId;
    std::string text;
    bool recording = false;
    bool finalizing = false;
    TriggerSource source = TriggerSource::Hold;
    std::optional<Phase> phase;
    std::optional<Mode> mode;
    std::optional<std::string> hotkey;
    std::string errorCode;
    std::string errorMessage;
};

std::string defaultSocketPath();
const char *triggerSourceName(TriggerSource source);
std::optional<TriggerSource> parseTriggerSource(const std::string &name);

class IpcClient {
public:
    explicit IpcClient(fcitx::Instance *instance);
    ~IpcClient();

    void setConfig(const IpcConfig &config);
    bool connect(const std::string &socket);
    void disconnect();
    void configure(Mode mode, const std::string &hotkey);
    void start(uint64_t segmentId, TriggerSource source);
    void stop(uint64_t segmentId, TriggerSource source);
    void toggle(uint64_t segmentId, TriggerSource source);
    bool connected() const { return fd_ >= 0; }
    void onReadable();

    std::function<void(const ServerMessage &)> onMessage;
    std::function<void()> onDisconnected;

private:
    void sendJson(const std::string &json);
    void flush();
    void readAvailable();
    void handleLine(const std::string &line);
    void updateEvents();
    void fail();

    fcitx::Instance *instance_;
    int fd_ = -1;
    std::unique_ptr<fcitx::EventSourceIO> ioEvent_;
    std::string socketPath_;
    std::string inBuffer_;
    std::string outBuffer_;
    IpcConfig config_;
    bool connectedOnce_ = false;
};
