#include "audio_capture.h"
#include <stdio.h>
#include <functiondiscoverykeys_devpkey.h>

AudioCapture::AudioCapture() = default;

AudioCapture::~AudioCapture() {
    Stop();
}

bool AudioCapture::Start(DataCallback callback, int sampleRate, float chunkDurationMs) {
    if (m_recording.load()) return false;

    m_callback = std::move(callback);
    m_sampleRate = sampleRate;
    m_chunkSize = static_cast<int>(sampleRate * chunkDurationMs / 1000.0f);
    m_stopRequested.store(false);

    HRESULT hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    if (FAILED(hr) && hr != RPC_E_CHANGED_MODE) {
        return false;
    }

    // Get default audio capture device
    hr = CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL,
        __uuidof(IMMDeviceEnumerator), (void**)&m_enumerator);
    if (FAILED(hr)) return false;

    hr = m_enumerator->GetDefaultAudioEndpoint(eCapture, eConsole, &m_device);
    if (FAILED(hr)) return false;

    hr = m_device->Activate(__uuidof(IAudioClient), CLSCTX_ALL,
        nullptr, (void**)&m_audioClient);
    if (FAILED(hr)) return false;

    WAVEFORMATEX wfx = {};
    wfx.wFormatTag = WAVE_FORMAT_PCM;
    wfx.nChannels = 1;
    wfx.nSamplesPerSec = m_sampleRate;
    wfx.wBitsPerSample = 16;
    wfx.nBlockAlign = wfx.nChannels * wfx.wBitsPerSample / 8;
    wfx.nAvgBytesPerSec = wfx.nSamplesPerSec * wfx.nBlockAlign;
    wfx.cbSize = 0;

    REFERENCE_TIME bufferDuration = (REFERENCE_TIME)(chunkDurationMs * 10000.0 * 2); // 2 chunks buffer
    hr = m_audioClient->Initialize(AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, bufferDuration, 0, &wfx, nullptr);
    if (FAILED(hr)) return false;

    hr = m_audioClient->GetService(__uuidof(IAudioCaptureClient),
        (void**)&m_captureClient);
    if (FAILED(hr)) return false;

    m_recording.store(true);
    hr = m_audioClient->Start();
    if (FAILED(hr)) {
        m_recording.store(false);
        return false;
    }

    m_thread = std::thread(&AudioCapture::CaptureLoop, this);
    return true;
}

void AudioCapture::Stop() {
    m_stopRequested.store(true);
    if (m_thread.joinable()) {
        m_thread.join();
    }

    if (m_audioClient) {
        m_audioClient->Stop();
        m_audioClient->Release();
        m_audioClient = nullptr;
    }
    if (m_captureClient) {
        m_captureClient->Release();
        m_captureClient = nullptr;
    }
    if (m_device) {
        m_device->Release();
        m_device = nullptr;
    }
    if (m_enumerator) {
        m_enumerator->Release();
        m_enumerator = nullptr;
    }
    m_recording.store(false);
}

void AudioCapture::CaptureLoop() {
    while (!m_stopRequested.load()) {
        UINT32 packetLength = 0;
        HRESULT hr = m_captureClient->GetNextPacketSize(&packetLength);
        if (FAILED(hr)) break;

        if (packetLength == 0) {
            Sleep(10);
            continue;
        }

        while (packetLength > 0) {
            BYTE* data = nullptr;
            UINT32 framesAvailable = 0;
            DWORD flags = 0;

            hr = m_captureClient->GetBuffer(&data, &framesAvailable, &flags, nullptr, nullptr);
            if (FAILED(hr)) break;

            int bytes = framesAvailable * 2; // 16-bit mono = 2 bytes/frame
            if (!(flags & AUDCLNT_BUFFERFLAGS_SILENT) && m_callback) {
                m_callback(data, bytes);
            }

            hr = m_captureClient->ReleaseBuffer(framesAvailable);
            if (FAILED(hr)) break;

            packetLength = 0;
            hr = m_captureClient->GetNextPacketSize(&packetLength);
            if (FAILED(hr)) break;
        }
    }
}
