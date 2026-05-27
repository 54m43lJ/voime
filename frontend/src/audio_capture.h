#pragma once

#include <windows.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <functional>
#include <thread>
#include <atomic>

class AudioCapture {
public:
    using DataCallback = std::function<void(const uint8_t* data, size_t size)>;

    AudioCapture();
    ~AudioCapture();

    bool Start(DataCallback callback, int sampleRate = 16000, float chunkDurationMs = 100.0f);
    void Stop();
    bool IsRecording() const { return m_recording.load(); }

private:
    void CaptureLoop();

    IMMDeviceEnumerator* m_enumerator = nullptr;
    IMMDevice* m_device = nullptr;
    IAudioClient* m_audioClient = nullptr;
    IAudioCaptureClient* m_captureClient = nullptr;

    std::thread m_thread;
    std::atomic<bool> m_recording{false};
    std::atomic<bool> m_stopRequested{false};
    DataCallback m_callback;
    int m_sampleRate = 16000;
    int m_chunkSize = 0;
};
