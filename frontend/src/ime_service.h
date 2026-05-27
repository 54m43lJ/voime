#pragma once

#include "globals.h"
#include "audio_capture.h"
#include "ws_client.h"

#include <string>
#include <atomic>

class CompositionEditSession;

class VoimeTextService : public ITfTextInputProcessorEx,
                         public ITfKeyEventSink,
                         public ITfDisplayAttributeProvider,
                         public ITfThreadMgrEventSink,
                         public ITfCompositionSink {
    friend class CompositionEditSession;

public:
    VoimeTextService();
    virtual ~VoimeTextService() = default;

    // IUnknown
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override;
    STDMETHODIMP_(ULONG) AddRef() override;
    STDMETHODIMP_(ULONG) Release() override;

    // ITfTextInputProcessorEx
    STDMETHODIMP ActivateEx(ITfThreadMgr* pThreadMgr, TfClientId tfClientId, DWORD dwFlags) override;
    STDMETHODIMP Deactivate() override;

    // ITfKeyEventSink
    STDMETHODIMP OnSetFocus(BOOL fForeground) override { return S_OK; }
    STDMETHODIMP OnTestKeyDown(ITfContext* pContext, WPARAM wParam, LPARAM lParam, BOOL* pEaten) override;
    STDMETHODIMP OnKeyDown(ITfContext* pContext, WPARAM wParam, LPARAM lParam, BOOL* pEaten) override;
    STDMETHODIMP OnTestKeyUp(ITfContext* pContext, WPARAM wParam, LPARAM lParam, BOOL* pEaten) override;
    STDMETHODIMP OnKeyUp(ITfContext* pContext, WPARAM wParam, LPARAM lParam, BOOL* pEaten) override;
    STDMETHODIMP OnPreservedKey(ITfContext* pContext, REFGUID rguid, BOOL* pEaten) override { return S_OK; }

    // ITfDisplayAttributeProvider
    STDMETHODIMP EnumDisplayAttributeInfo(IEnumTfDisplayAttributeInfo** ppEnum) override;
    STDMETHODIMP GetDisplayAttributeInfo(REFGUID guidInfo, ITfDisplayAttributeInfo** ppInfo, CLSID* pclsidOwner) override;

    // ITfThreadMgrEventSink
    STDMETHODIMP OnInitDocumentMgr(ITfDocumentMgr* pDocMgr) override { return S_OK; }
    STDMETHODIMP OnUninitDocumentMgr(ITfDocumentMgr* pDocMgr) override { return S_OK; }
    STDMETHODIMP OnSetFocus(ITfDocumentMgr* pDocMgrFocused, ITfDocumentMgr* pDocMgrPrevFocus) override { return S_OK; }
    STDMETHODIMP OnPushContext(ITfContext* pContext) override { return S_OK; }
    STDMETHODIMP OnPopContext(ITfContext* pContext) override { return S_OK; }

    // ITfCompositionSink
    STDMETHODIMP OnCompositionTerminated(TfEditCookie ecWrite, ITfComposition* pComposition) override { return S_OK; }

private:
    void StartRecording();
    void StopRecording();
    void CommitText(const std::string& text);
    void UpdateComposition(const std::string& text);
    void EndComposition(bool commit);

    void OnAudioData(const uint8_t* data, size_t size);
    void OnWsMessage(const std::string& msg);
    void OnWsStatus(const std::string& status);

    LONG m_refCount = 1;

    ITfThreadMgr* m_threadMgr = nullptr;
    TfClientId m_clientId = 0;
    ITfContext* m_currentContext = nullptr;
    ITfComposition* m_composition = nullptr;
    DWORD m_dwThreadMgrCookie = 0;

    // Display attribute info GUID
    GUID m_displayAttrGuid = GUID_VoimeDisplayAttribute;

    AudioCapture m_audioCapture;
    WebSocketClient m_wsClient;

    std::atomic<bool> m_isRecording{false};
    std::string m_accumulatedText;
    std::string m_finalText;
    std::atomic<bool> m_hasFinal{false};

    // Server URL
    std::wstring m_serverUrl = L"ws://127.0.0.1:9876/asr";

    // Trigger key
    static constexpr int TRIGGER_KEY = VK_F2;
};
