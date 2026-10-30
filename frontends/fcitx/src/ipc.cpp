#include "ipc.h"

#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <nlohmann/json.hpp>

#include <fcitx/instance.h>

#include "config.h"

namespace {

constexpr size_t MAX_LINE = 1024 * 1024;

std::string jsonString(const nlohmann::json &body, const char *key) {
    auto it = body.find(key);
    if (it == body.end() || !it->is_string()) {
        return {};
    }
    return it->get<std::string>();
}

std::optional<uint64_t> jsonUint64(const nlohmann::json &body, const char *key) {
    auto it = body.find(key);
    if (it == body.end() || it->is_null() || !it->is_number_unsigned()) {
        return std::nullopt;
    }
    return it->get<uint64_t>();
}

std::optional<Mode> parseMode(const std::string &name) {
    if (name == "ptt") {
        return Mode::Ptt;
    }
    if (name == "toggle") {
        return Mode::Toggle;
    }
    if (name == "continuous") {
        return Mode::Continuous;
    }
    return std::nullopt;
}

} // namespace

std::string defaultSocketPath() {
    const char *runtime = std::getenv("XDG_RUNTIME_DIR");
    if (runtime && *runtime) {
        return std::string(runtime) + "/voime/voimed.sock";
    }
    return "/tmp/voime-" + std::to_string(getuid()) + "/voimed.sock";
}

const char *triggerSourceName(TriggerSource source) {
    switch (source) {
    case TriggerSource::Hold:
        return "hold";
    case TriggerSource::Toggle:
        return "toggle";
    case TriggerSource::Tui:
        return "tui";
    case TriggerSource::Cli:
        return "cli";
    case TriggerSource::Continuous:
        return "continuous";
    }
    return "hold";
}

std::optional<TriggerSource> parseTriggerSource(const std::string &name) {
    if (name == "hold") {
        return TriggerSource::Hold;
    }
    if (name == "toggle") {
        return TriggerSource::Toggle;
    }
    if (name == "tui") {
        return TriggerSource::Tui;
    }
    if (name == "cli") {
        return TriggerSource::Cli;
    }
    if (name == "continuous") {
        return TriggerSource::Continuous;
    }
    return std::nullopt;
}

IpcClient::IpcClient(fcitx::Instance *instance) : instance_(instance) {}

IpcClient::~IpcClient() {
    if (ioEvent_) {
        ioEvent_->setEnabled(false);
    }
    if (fd_ >= 0) {
        ::close(fd_);
        fd_ = -1;
    }
}

void IpcClient::setConfig(const IpcConfig &config) { config_ = config; }

bool IpcClient::connect(const std::string &socket) {
    if (fd_ >= 0) {
        return true;
    }
    socketPath_ = socket;
    int fd = ::socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        return false;
    }
    sockaddr_un addr{};
    addr.sun_family = AF_UNIX;
    if (socket.size() >= sizeof(addr.sun_path)) {
        ::close(fd);
        return false;
    }
    std::memcpy(addr.sun_path, socket.c_str(), socket.size() + 1);
    if (::connect(fd, reinterpret_cast<sockaddr *>(&addr), sizeof(addr)) < 0) {
        ::close(fd);
        return false;
    }
    int flags = ::fcntl(fd, F_GETFL, 0);
    if (flags >= 0 && ::fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0) {
        ::close(fd);
        return false;
    }
    fd_ = fd;
    inBuffer_.clear();
    outBuffer_.clear();
    ioEvent_ = instance_->eventLoop().addIOEvent(
        fd_, fcitx::IOEventFlag::In,
        [this](fcitx::EventSourceIO *, int, fcitx::IOEventFlags) {
            onReadable();
            return true;
        });

    nlohmann::json config = {
        {"mode", modeName(config_.mode)},
        {"hotkey", config_.hotkey},
    };
    nlohmann::json hello = {
        {"Hello",
         {{"role", "fcitx"},
          {"version", 1},
          {"config", config}}},
    };
    sendJson(hello.dump());
    connectedOnce_ = true;
    return true;
}

void IpcClient::disconnect() {
    bool notify = fd_ >= 0;
    if (fd_ >= 0) {
        ::close(fd_);
        fd_ = -1;
    }
    if (ioEvent_) {
        ioEvent_->setEnabled(false);
    }
    inBuffer_.clear();
    outBuffer_.clear();
    if (notify && onDisconnected) {
        onDisconnected();
    }
}

void IpcClient::configure(Mode mode, const std::string &hotkey) {
    config_ = IpcConfig{mode, hotkey};
    if (fd_ < 0) {
        return;
    }
    nlohmann::json message = {
        {"Configure", {{"mode", modeName(mode)}, {"hotkey", hotkey}}},
    };
    sendJson(message.dump());
}

void IpcClient::start(uint64_t segmentId, TriggerSource source) {
    if (fd_ < 0) {
        return;
    }
    nlohmann::json message = {
        {"Start",
         {{"segment_id", segmentId},
          {"source", triggerSourceName(source)}}},
    };
    sendJson(message.dump());
}

void IpcClient::stop(uint64_t segmentId, TriggerSource source) {
    if (fd_ < 0) {
        return;
    }
    nlohmann::json message = {
        {"Stop",
         {{"segment_id", segmentId},
          {"source", triggerSourceName(source)}}},
    };
    sendJson(message.dump());
}

void IpcClient::toggle(uint64_t segmentId, TriggerSource source) {
    if (fd_ < 0) {
        return;
    }
    nlohmann::json message = {
        {"Toggle",
         {{"segment_id", segmentId},
          {"source", triggerSourceName(source)}}},
    };
    sendJson(message.dump());
}

void IpcClient::sendJson(const std::string &json) {
    if (fd_ < 0) {
        return;
    }
    outBuffer_ += json;
    outBuffer_ += '\n';
    flush();
}

void IpcClient::flush() {
    if (fd_ < 0) {
        return;
    }
    while (!outBuffer_.empty()) {
        ssize_t written = ::send(fd_, outBuffer_.data(), outBuffer_.size(),
                                 MSG_NOSIGNAL);
        if (written > 0) {
            outBuffer_.erase(0, static_cast<size_t>(written));
            continue;
        }
        if (written < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            break;
        }
        fail();
        return;
    }
    updateEvents();
}

void IpcClient::readAvailable() {
    if (fd_ < 0) {
        return;
    }
    char buffer[4096];
    for (;;) {
        ssize_t read = ::recv(fd_, buffer, sizeof(buffer), 0);
        if (read > 0) {
            inBuffer_.append(buffer, static_cast<size_t>(read));
            if (inBuffer_.size() > MAX_LINE && inBuffer_.find('\n') == std::string::npos) {
                fail();
                return;
            }
            size_t newline = 0;
            while ((newline = inBuffer_.find('\n')) != std::string::npos) {
                std::string line = inBuffer_.substr(0, newline);
                inBuffer_.erase(0, newline + 1);
                if (line.size() > MAX_LINE) {
                    fail();
                    return;
                }
                handleLine(line);
                if (fd_ < 0) {
                    return;
                }
            }
            continue;
        }
        if (read == 0) {
            fail();
            return;
        }
        if (errno == EAGAIN || errno == EWOULDBLOCK) {
            break;
        }
        fail();
        return;
    }
}

void IpcClient::handleLine(const std::string &line) {
    nlohmann::json document;
    try {
        document = nlohmann::json::parse(line);
    } catch (...) {
        return;
    }
    if (!document.is_object() || document.empty()) {
        return;
    }
    auto it = document.begin();
    const std::string tag = it.key();
    const nlohmann::json &body = it.value();
    ServerMessage message;

    if (tag == "Snapshot") {
        message.type = ServerMessageType::Snapshot;
        auto mode = parseMode(jsonString(body, "mode"));
        message.mode = mode;
        auto hotkey = jsonString(body, "hotkey");
        if (!hotkey.empty()) {
            message.hotkey = hotkey;
        }
        auto partial = jsonString(body, "partial");
        if (!partial.empty()) {
            message.text = partial;
        }
    } else if (tag == "SegmentStarted") {
        message.type = ServerMessageType::SegmentStarted;
        message.segmentId = jsonUint64(body, "segment_id");
        if (auto source = parseTriggerSource(jsonString(body, "source"))) {
            message.source = *source;
        }
    } else if (tag == "State") {
        message.type = ServerMessageType::State;
        message.recording = body.value("recording", false);
        message.finalizing = body.value("finalizing", false);
        message.segmentId = jsonUint64(body, "segment_id");
    } else if (tag == "Partial") {
        message.type = ServerMessageType::Partial;
        message.segmentId = jsonUint64(body, "segment_id");
        message.text = jsonString(body, "text");
    } else if (tag == "Final") {
        message.type = ServerMessageType::Final;
        message.segmentId = jsonUint64(body, "segment_id");
        message.text = jsonString(body, "text");
    } else if (tag == "HistoryCleared") {
        message.type = ServerMessageType::HistoryCleared;
    } else if (tag == "Status") {
        message.type = ServerMessageType::Status;
        if (body.contains("status") && body["status"].is_object()) {
            const auto &status = body["status"];
            auto mode = parseMode(jsonString(status, "mode"));
            message.mode = mode;
            auto hotkey = jsonString(status, "hotkey");
            if (!hotkey.empty()) {
                message.hotkey = hotkey;
            }
            message.recording = status.value("recording", false);
            message.finalizing = status.value("finalizing", false);
            message.segmentId = jsonUint64(status, "segment_id");
        }
    } else if (tag == "Error") {
        message.type = ServerMessageType::Error;
        message.errorCode = jsonString(body, "code");
        message.errorMessage = jsonString(body, "message");
    } else if (tag == "Bye") {
        message.type = ServerMessageType::Bye;
    } else {
        message.type = ServerMessageType::Unknown;
    }

    if (onMessage) {
        onMessage(message);
    }
}

void IpcClient::updateEvents() {
    if (!ioEvent_) {
        return;
    }
    fcitx::IOEventFlags flags = fcitx::IOEventFlag::In;
    if (!outBuffer_.empty()) {
        flags |= fcitx::IOEventFlag::Out;
    }
    ioEvent_->setEvents(flags);
}

void IpcClient::onReadable() {
    if (fd_ < 0) {
        return;
    }
    flush();
    if (fd_ < 0) {
        return;
    }
    readAvailable();
}

void IpcClient::fail() {
    if (fd_ >= 0) {
        ::close(fd_);
        fd_ = -1;
    }
    if (ioEvent_) {
        ioEvent_->setEnabled(false);
    }
    inBuffer_.clear();
    outBuffer_.clear();
    if (onDisconnected) {
        onDisconnected();
    }
}
