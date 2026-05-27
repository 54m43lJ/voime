#include "ime_service.h"
#include <stdio.h>
#include <string>

// Simple JSON value extractors (avoid external dependencies)

static std::string JsonGetString(const std::string& json, const std::string& key) {
    std::string search = "\"" + key + "\":\"";
    size_t pos = json.find(search);
    if (pos == std::string::npos) return "";
    pos += search.size();
    size_t end = json.find('"', pos);
    if (end == std::string::npos) return "";
    return json.substr(pos, end - pos);
}

static std::string JsonGetType(const std::string& json) {
    return JsonGetString(json, "type");
}

static bool JsonGetBool(const std::string& json, const std::string& key) {
    std::string search = "\"" + key + "\":";
    size_t pos = json.find(search);
    if (pos == std::string::npos) return false;
    pos += search.size();
    while (pos < json.size() && (json[pos] == ' ' || json[pos] == '\t')) pos++;
    if (pos >= json.size()) return false;
    return json[pos] == 't';
}

// ============================================================
// Edit session helpers
// ============================================================

struct EditSessionData {
    std::string text;   // UTF-8 text
    bool commit = false;
    VoimeTextService* svc = nullptr;
};

// Edit session for setting composition text
class CompositionEditSession : public ITfEditSession {
public:
    CompositionEditSession(const std::string& t) : text(t) {}

    STDMETHODIMP QueryInterface(REFIID, void**) override { return E_NOINTERFACE; }
    STDMETHODIMP_(ULONG) AddRef() override { return 1; }
    STDMETHODIMP_(ULONG) Release() override { return 1; }

    STDMETHODIMP DoEditSession(TfEditCookie ec) override {
        if (!svc) return E_FAIL;

        if (commit) {
            // End existing composition to commit
            if (svc->m_composition) {
                svc->m_composition->Release();
                svc->m_composition = nullptr;
            }

            if (!text.empty()) {
                // Insert final text at selection
                int wideLen = MultiByteToWideChar(CP_UTF8, 0, text.c_str(), -1, nullptr, 0);
                std::wstring wtext(wideLen, L'\0');
                MultiByteToWideChar(CP_UTF8, 0, text.c_str(), -1, &wtext[0], wideLen);
                wtext.resize(wideLen - 1);

                ITfInsertAtSelection* pInsert = nullptr;
                if (SUCCEEDED(svc->m_currentContext->QueryInterface(
                        IID_ITfInsertAtSelection, (void**)&pInsert)) && pInsert) {
                    ITfRange* pRange = nullptr;
                    pInsert->InsertTextAtSelection(ec, TF_IAS_NOQUERY,
                        wtext.c_str(), (LONG)wtext.size(), &pRange);
                    if (pRange) pRange->Release();
                    pInsert->Release();
                }
            }
        } else {
            // Update composition with partial text
            int wideLen = MultiByteToWideChar(CP_UTF8, 0, text.c_str(), -1, nullptr, 0);
            std::wstring wtext(wideLen, L'\0');
            MultiByteToWideChar(CP_UTF8, 0, text.c_str(), -1, &wtext[0], wideLen);
            wtext.resize(wideLen - 1);

            if (svc->m_composition) {
                // Update existing composition range text
                ITfRange* pCompRange = nullptr;
                if (SUCCEEDED(svc->m_composition->GetRange(&pCompRange)) && pCompRange) {
                    pCompRange->SetText(ec, 0, wtext.c_str(), (LONG)wtext.size());
                    pCompRange->Release();
                }
            } else {
                // Start new composition
                ITfContextComposition* pCtxComp = nullptr;
                if (SUCCEEDED(svc->m_currentContext->QueryInterface(
                        IID_ITfContextComposition, (void**)&pCtxComp)) && pCtxComp) {
                    ITfRange* pRange = nullptr;
                    // Get selection range
                    svc->m_currentContext->GetSelection(ec, TF_DEFAULT_SELECTION, 1, &pRange, nullptr);
                    if (pRange) {
                        pCtxComp->StartComposition(ec, pRange, svc, &svc->m_composition);
                        // Set text on the composition range
                        if (svc->m_composition) {
                            ITfRange* pCompRange = nullptr;
                            svc->m_composition->GetRange(&pCompRange);
                            if (pCompRange) {
                                pCompRange->SetText(ec, 0, wtext.c_str(), (LONG)wtext.size());
                                pCompRange->Release();
                            }
                        }
                        pRange->Release();
                    }
                    pCtxComp->Release();
                }
            }
        }
        return S_OK;
    }

    std::string text;
    bool commit = false;
    VoimeTextService* svc = nullptr;
};

// ============================================================
// VoimeTextService implementation
// ============================================================

VoimeTextService::VoimeTextService() = default;

STDMETHODIMP VoimeTextService::QueryInterface(REFIID riid, void** ppv) {
    if (ppv == nullptr) return E_POINTER;
    *ppv = nullptr;

    if (IsEqualIID(riid, IID_IUnknown) || IsEqualIID(riid, IID_ITfTextInputProcessorEx))
        *ppv = static_cast<ITfTextInputProcessorEx*>(this);
    else if (IsEqualIID(riid, IID_ITfKeyEventSink))
        *ppv = static_cast<ITfKeyEventSink*>(this);
    else if (IsEqualIID(riid, IID_ITfDisplayAttributeProvider))
        *ppv = static_cast<ITfDisplayAttributeProvider*>(this);
    else if (IsEqualIID(riid, IID_ITfThreadMgrEventSink))
        *ppv = static_cast<ITfThreadMgrEventSink*>(this);
    else if (IsEqualIID(riid, IID_ITfCompositionSink))
        *ppv = static_cast<ITfCompositionSink*>(this);
    else
        return E_NOINTERFACE;

    AddRef();
    return S_OK;
}

STDMETHODIMP_(ULONG) VoimeTextService::AddRef() {
    return InterlockedIncrement(&m_refCount);
}

STDMETHODIMP_(ULONG) VoimeTextService::Release() {
    LONG c = InterlockedDecrement(&m_refCount);
    if (c == 0) delete this;
    return c;
}

// ============================================================
// ITfTextInputProcessorEx
// ============================================================

STDMETHODIMP VoimeTextService::ActivateEx(ITfThreadMgr* pThreadMgr,
                                           TfClientId tfClientId, DWORD /*dwFlags*/) {
    m_threadMgr = pThreadMgr;
    m_clientId = tfClientId;

    ITfKeystrokeMgr* pKeyMgr = nullptr;
    if (SUCCEEDED(m_threadMgr->QueryInterface(IID_ITfKeystrokeMgr, (void**)&pKeyMgr))) {
        pKeyMgr->AdviseKeyEventSink(m_clientId, this, TRUE);
        pKeyMgr->Release();
    }

    ITfSource* pSource = nullptr;
    if (SUCCEEDED(m_threadMgr->QueryInterface(IID_ITfSource, (void**)&pSource))) {
        pSource->AdviseSink(IID_ITfThreadMgrEventSink,
            static_cast<ITfThreadMgrEventSink*>(this), &m_dwThreadMgrCookie);
        pSource->Release();
    }

    m_wsClient.Connect(m_serverUrl,
        std::bind(&VoimeTextService::OnWsMessage, this, std::placeholders::_1),
        std::bind(&VoimeTextService::OnWsStatus, this, std::placeholders::_1));

    m_wsClient.SendText("{\"type\":\"config\",\"language\":\"zh\",\"model\":\"base\"}");

    OutputDebugStringW(L"Voime: Activated\n");
    return S_OK;
}

STDMETHODIMP VoimeTextService::Deactivate() {
    StopRecording();
    m_wsClient.Disconnect();

    ITfKeystrokeMgr* pKeyMgr = nullptr;
    if (m_threadMgr && SUCCEEDED(m_threadMgr->QueryInterface(IID_ITfKeystrokeMgr, (void**)&pKeyMgr))) {
        pKeyMgr->UnadviseKeyEventSink(m_clientId);
        pKeyMgr->Release();
    }

    if (m_threadMgr) {
        ITfSource* pSource = nullptr;
        if (SUCCEEDED(m_threadMgr->QueryInterface(IID_ITfSource, (void**)&pSource))) {
            pSource->UnadviseSink(m_dwThreadMgrCookie);
            pSource->Release();
        }
    }

    VOIME_SAFE_RELEASE(m_currentContext);
    VOIME_SAFE_RELEASE(m_threadMgr);
    OutputDebugStringW(L"Voime: Deactivated\n");
    return S_OK;
}

// ============================================================
// ITfKeyEventSink
// ============================================================

STDMETHODIMP VoimeTextService::OnTestKeyDown(ITfContext* pContext,
                                              WPARAM wParam, LPARAM, BOOL* pEaten) {
    if (wParam == TRIGGER_KEY) {
        *pEaten = TRUE;
        if (pContext != m_currentContext) {
            VOIME_SAFE_RELEASE(m_currentContext);
            m_currentContext = pContext;
            if (m_currentContext) m_currentContext->AddRef();
        }
        return S_OK;
    }
    *pEaten = FALSE;
    return S_OK;
}

STDMETHODIMP VoimeTextService::OnKeyDown(ITfContext* pContext,
                                          WPARAM wParam, LPARAM, BOOL* pEaten) {
    if (wParam == TRIGGER_KEY) {
        *pEaten = TRUE;
        if (pContext != m_currentContext) {
            VOIME_SAFE_RELEASE(m_currentContext);
            m_currentContext = pContext;
            if (m_currentContext) m_currentContext->AddRef();
        }

        if (m_isRecording.load())
            StopRecording();
        else
            StartRecording();
        return S_OK;
    }
    *pEaten = FALSE;
    return S_OK;
}

STDMETHODIMP VoimeTextService::OnTestKeyUp(ITfContext*, WPARAM, LPARAM, BOOL* pEaten) {
    *pEaten = FALSE; return S_OK;
}

STDMETHODIMP VoimeTextService::OnKeyUp(ITfContext*, WPARAM, LPARAM, BOOL* pEaten) {
    *pEaten = FALSE; return S_OK;
}

// ============================================================
// ITfDisplayAttributeProvider
// ============================================================

STDMETHODIMP VoimeTextService::EnumDisplayAttributeInfo(IEnumTfDisplayAttributeInfo** ppEnum) {
    *ppEnum = nullptr;
    return E_NOTIMPL;
}

STDMETHODIMP VoimeTextService::GetDisplayAttributeInfo(REFGUID, ITfDisplayAttributeInfo** ppInfo, CLSID*) {
    *ppInfo = nullptr;
    return E_NOTIMPL;
}

// ============================================================
// Recording control
// ============================================================

void VoimeTextService::StartRecording() {
    if (m_isRecording.load()) return;

    m_isRecording.store(true);
    m_accumulatedText.clear();
    m_finalText.clear();
    m_hasFinal.store(false);

    m_wsClient.SendText("{\"type\":\"speech_start\"}");

    m_audioCapture.Start(
        std::bind(&VoimeTextService::OnAudioData, this,
                  std::placeholders::_1, std::placeholders::_2));

    OutputDebugStringW(L"Voime: Recording started\n");
}

void VoimeTextService::StopRecording() {
    if (!m_isRecording.load()) return;

    m_audioCapture.Stop();
    m_wsClient.SendText("{\"type\":\"speech_end\"}");

    m_isRecording.store(false);
    OutputDebugStringW(L"Voime: Recording stopped\n");
}

void VoimeTextService::OnAudioData(const uint8_t* data, size_t size) {
    if (m_wsClient.IsConnected())
        m_wsClient.SendBinary(data, size);
}

// ============================================================
// WebSocket message handling
// ============================================================

void VoimeTextService::OnWsMessage(const std::string& msg) {
    std::string type = JsonGetType(msg);

    if (type == "status") return;

    if (type == "partial") {
        std::string text = JsonGetString(msg, "text");
        if (!text.empty())
            UpdateComposition(text);

    } else if (type == "final") {
        m_finalText = JsonGetString(msg, "text");
        m_hasFinal.store(true);

    } else if (type == "polished") {
        m_finalText = JsonGetString(msg, "text");
        m_hasFinal.store(true);
    }

    if (m_hasFinal.load() && !m_finalText.empty()) {
        CommitText(m_finalText);
        m_finalText.clear();
        m_hasFinal.store(false);
    }
}

void VoimeTextService::OnWsStatus(const std::string& status) {
    OutputDebugStringA(("Voime WS: " + status + "\n").c_str());
}

// ============================================================
// Composition helpers
// ============================================================

void VoimeTextService::UpdateComposition(const std::string& text) {
    if (!m_currentContext) return;

    CompositionEditSession* session = new CompositionEditSession(text);
    session->svc = this;
    session->commit = false;

    HRESULT hr = m_currentContext->RequestEditSession(m_clientId,
        session, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE, &hr);
    session->Release();
}

void VoimeTextService::CommitText(const std::string& text) {
    if (!m_currentContext || text.empty()) return;

    CompositionEditSession* session = new CompositionEditSession(text);
    session->svc = this;
    session->commit = true;

    HRESULT hr = m_currentContext->RequestEditSession(m_clientId,
        session, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE, &hr);
    session->Release();
}

void VoimeTextService::EndComposition(bool) {
    if (m_composition) {
        m_composition->Release();
        m_composition = nullptr;
    }
}
