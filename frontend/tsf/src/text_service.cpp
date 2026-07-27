// text_service.cpp — Voime TSF 文本服务实现
#include "text_service.h"
#include "ptt.h"
#include "util.h" // frontend/src（utf8_to_wide）

#include <new>

// commit_text 投递体（IPC reader 线程 -> UI 线程）
struct CommitMsg {
    std::string payload;
    std::string request_id;
};

// ---- P5-05：异步编辑会话，在光标处插入文本 ----
//
// 三条实测踩坑结论（qa06 调试记录）：
//  1) TF_ES_SYNC 在本线程拥有目标文档时永久挂起（TSF 等待自身消息循环，自死锁）。
//  2) 异步会话对象所有权：TSF 排队后不一定 AddRef，请求方提前 Release 会导致
//     会话永不派发（悬垂指针）。采用“成功移交 + DoEditSession 末尾自释”策略。
//  3) DoEditSession 执行期间 TSF 持有串行锁，回调内禁止再调 GetFocus/GetTop
//     （二次进入自死锁）。目标 ITfContext 必须在请求前取好传入。
//  4) ITfInsertAtSelection::InsertTextAtSelection 在本环境（Win10 22H2, 标准
//     EDIT 控件）会话内永久阻塞（原因未明，疑与系统 ACP 包装层重入有关）；
//     改用 IME 标准路径：GetSelection -> StartComposition -> SetText ->
//     EndComposition，实测可用。
// 空 CompositionSink（StartComposition 传 NULL 槽在 Win10 22H2 实测返回
// E_INVALIDARG，IME 惯例总是携带 sink）
class CNullCompositionSink : public ITfCompositionSink {
public:
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) {
        if (!ppv) return E_POINTER;
        if (IsEqualIID(riid, IID_IUnknown) || IsEqualIID(riid, IID_ITfCompositionSink)) {
            *ppv = (ITfCompositionSink*)this;
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }
    STDMETHODIMP_(ULONG) AddRef() { return (ULONG)InterlockedIncrement(&ref_); }
    STDMETHODIMP_(ULONG) Release() { return (ULONG)InterlockedDecrement(&ref_); }
    STDMETHODIMP OnCompositionTerminated(TfEditCookie, ITfComposition*) { return S_OK; }

private:
    LONG ref_ = 1;
};

class CInsertTextEditSession : public ITfEditSession {
public:
    // ctx 由调用方持有引用传入（会话接管之，析构释放）
    CInsertTextEditSession(ITfContext* ctx, CVoimeTextService* svc,
                           const std::wstring& text, const std::string& req)
        : ctx_(ctx), svc_(svc), text_(text), req_(req) {}

    ~CInsertTextEditSession() {
        if (ctx_) ctx_->Release();
    }

    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) {
        if (!ppv) return E_POINTER;
        if (IsEqualIID(riid, IID_IUnknown) || IsEqualIID(riid, IID_ITfEditSession)) {
            *ppv = (ITfEditSession*)this;
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }
    STDMETHODIMP_(ULONG) AddRef() { return (ULONG)InterlockedIncrement(&ref_); }
    STDMETHODIMP_(ULONG) Release() {
        ULONG r = (ULONG)InterlockedDecrement(&ref_);
        if (!r) delete this;
        return r;
    }

    STDMETHODIMP DoEditSession(TfEditCookie ec) {
        tsf_log("DoEditSession enter ec=%lu", ec);
        // 诊断：确认注入目标窗口（ GetActiveView/GetWnd 为接口直取，不进 TSF 锁）
        ITfContextView* view = nullptr;
        if (SUCCEEDED(ctx_->GetActiveView(&view)) && view) {
            HWND hw = nullptr;
            view->GetWnd(&hw);
            DWORD pid = 0;
            if (hw) GetWindowThreadProcessId(hw, &pid);
            tsf_log("inject target hwnd=%p pid=%lu", hw, pid);
            view->Release();
        }
        HRESULT hr = E_FAIL;
        TF_SELECTION sel{};
        ULONG fetched = 0;
        HRESULT hr_sel = ctx_->GetSelection(ec, TF_DEFAULT_SELECTION, 1, &sel, &fetched);
        if (SUCCEEDED(hr_sel) && fetched == 1) {
            // 读写会话内直接改写选区 range（IME 提交惯例；不用组合）
            hr = sel.range->SetText(ec, 0, text_.c_str(), (LONG)text_.size());
            // 诊断：回读文档文本，确认写入是否真实生效
            if (SUCCEEDED(hr)) {
                wchar_t rb[64]{};
                ULONG got = 0;
                if (SUCCEEDED(sel.range->GetText(ec, 0, rb, 63, &got)))
                    tsf_log("doc readback len=%lu text=%s", got,
                            wide_to_utf8(std::wstring(rb, got)).c_str());
            }
            sel.range->Release();
        }
        tsf_log("compose sel=0x%08lx/%lu direct_set hr=0x%08lx", hr_sel, fetched, hr);
        svc_->respond_commit(req_, hr); // 应答 Agent（PRD OnCommitText 契约）
        tsf_log("commit_text inject hr=0x%08lx len=%zu", hr, text_.size());
        // 自释放所有权（兼容 TSF AddRef 与否两种实现，见 insert_text_async）
        Release();
        return S_OK; // 会话本身已执行，注入结果经 IPC 回报
    }

private:
    LONG ref_ = 1;
    ITfContext* ctx_;
    CVoimeTextService* svc_;
    std::wstring text_;
    std::string req_;
};

// ---- CVoimeTextService ----

CVoimeTextService::CVoimeTextService() { InterlockedIncrement(&g_dll_locks); }
CVoimeTextService::~CVoimeTextService() { InterlockedDecrement(&g_dll_locks); }

STDMETHODIMP CVoimeTextService::QueryInterface(REFIID riid, void** ppv) {
    if (!ppv) return E_POINTER;
    if (IsEqualIID(riid, IID_IUnknown) || IsEqualIID(riid, IID_ITfTextInputProcessor))
        *ppv = (ITfTextInputProcessor*)this;
    else if (IsEqualIID(riid, IID_ITfKeyEventSink))
        *ppv = (ITfKeyEventSink*)this;
    else if (IsEqualIID(riid, IID_ITfThreadMgrEventSink))
        *ppv = (ITfThreadMgrEventSink*)this;
    else {
        *ppv = nullptr;
        return E_NOINTERFACE;
    }
    AddRef();
    return S_OK;
}

STDMETHODIMP_(ULONG) CVoimeTextService::AddRef() {
    return (ULONG)InterlockedIncrement(&ref_);
}

STDMETHODIMP_(ULONG) CVoimeTextService::Release() {
    ULONG r = (ULONG)InterlockedDecrement(&ref_);
    if (!r) delete this;
    return r;
}

STDMETHODIMP CVoimeTextService::Activate(ITfThreadMgr* ptim, TfClientId tid) {
    if (!ptim) return E_INVALIDARG;
    ptim_ = ptim;
    ptim_->AddRef();
    tid_ = tid;

    // P5-04：按键事件槽（前台线程按键全量回调）
    ITfKeystrokeMgr* ksm = nullptr;
    if (SUCCEEDED(ptim_->QueryInterface(IID_ITfKeystrokeMgr, (void**)&ksm))) {
        key_advised_ = SUCCEEDED(ksm->AdviseKeyEventSink(tid_, (ITfKeyEventSink*)this, TRUE));
        ksm->Release();
    }

    // 焦点文档跟踪（GetFocus 可能返回线程占位 docmgr，OnSetFocus 才是真实焦点）
    ITfSource* src = nullptr;
    if (SUCCEEDED(ptim_->QueryInterface(IID_ITfSource, (void**)&src))) {
        src->AdviseSink(IID_ITfThreadMgrEventSink, (ITfThreadMgrEventSink*)this,
                        &tm_sink_cookie_);
        src->Release();
    }

    load_ptt(ptt_mods_, ptt_vk_); // 与 Agent 同配置（config.toml hotkey）

    // 隐藏消息窗口（commit_text 注入在本线程/套间执行）
    WNDCLASSEXW wc{ sizeof(wc) };
    wc.lpfnWndProc = &CVoimeTextService::wnd_proc;
    wc.hInstance = g_hmod;
    wc.lpszClassName = L"VoimeTsfMsg";
    RegisterClassExW(&wc); // 已注册返回失败，忽略
    hwnd_ = CreateWindowExW(0, L"VoimeTsfMsg", L"", 0, 0, 0, 0, 0, HWND_MESSAGE, nullptr,
                            g_hmod, nullptr);
    if (hwnd_) SetWindowLongPtrW(hwnd_, GWLP_USERDATA, (LONG_PTR)this);

    // P5-02：IPC 客户端（自动重连）
    IpcClient::Events ev;
    ev.on_commit_text = [this](const std::string& payload, const std::string& req) {
        if (hwnd_) {
            auto* m = new CommitMsg{ payload, req };
            if (!PostMessageW(hwnd_, WM_VOIME_COMMIT, 0, (LPARAM)m)) delete m;
        }
    };
    ev.on_state = [](bool c) { tsf_log("IPC %s", c ? "connected" : "disconnected"); };
    ev.on_log = [](const std::string& s) { tsf_log("%s", s.c_str()); };
    ipc_.start(ev);

    tsf_log("Activate tid=%lu key_advised=%d ptt vk=0x%x mods=0x%x hwnd=%p", tid_,
            key_advised_, ptt_vk_, ptt_mods_, hwnd_);
    return S_OK;
}

STDMETHODIMP CVoimeTextService::Deactivate() {
    tsf_log("Deactivate tid=%lu", tid_);
    ipc_.stop();
    if (hwnd_) {
        DestroyWindow(hwnd_);
        hwnd_ = nullptr;
    }
    if (key_advised_ && ptim_) {
        ITfKeystrokeMgr* ksm = nullptr;
        if (SUCCEEDED(ptim_->QueryInterface(IID_ITfKeystrokeMgr, (void**)&ksm))) {
            ksm->UnadviseKeyEventSink(tid_);
            ksm->Release();
        }
        key_advised_ = false;
    }
    if (tm_sink_cookie_ != TF_INVALID_COOKIE && ptim_) {
        ITfSource* src = nullptr;
        if (SUCCEEDED(ptim_->QueryInterface(IID_ITfSource, (void**)&src))) {
            src->UnadviseSink(tm_sink_cookie_);
            src->Release();
        }
        tm_sink_cookie_ = TF_INVALID_COOKIE;
    }
    if (focus_dim_) {
        focus_dim_->Release();
        focus_dim_ = nullptr;
    }
    if (ptim_) {
        ptim_->Release();
        ptim_ = nullptr;
    }
    ptt_down_ = false;
    return S_OK;
}

STDMETHODIMP CVoimeTextService::OnSetFocus(BOOL) { return S_OK; }

STDMETHODIMP CVoimeTextService::OnInitDocumentMgr(ITfDocumentMgr*) { return S_OK; }
STDMETHODIMP CVoimeTextService::OnUninitDocumentMgr(ITfDocumentMgr*) { return S_OK; }
STDMETHODIMP CVoimeTextService::OnPushContext(ITfContext*) { return S_OK; }
STDMETHODIMP CVoimeTextService::OnPopContext(ITfContext*) { return S_OK; }

STDMETHODIMP CVoimeTextService::OnSetFocus(ITfDocumentMgr* focus,
                                           ITfDocumentMgr*) {
    // 记录真实焦点文档（注入目标）；诊断窗口归属
    if (focus) focus->AddRef();
    if (focus_dim_) focus_dim_->Release();
    focus_dim_ = focus;
    if (focus) {
        ITfContext* ctx = nullptr;
        ITfContextView* view = nullptr;
        if (SUCCEEDED(focus->GetTop(&ctx)) && ctx &&
            SUCCEEDED(ctx->GetActiveView(&view)) && view) {
            HWND hw = nullptr;
            view->GetWnd(&hw);
            tsf_log("OnSetFocus dim hwnd=%p", hw);
        }
        if (view) view->Release();
        if (ctx) ctx->Release();
    } else {
        tsf_log("OnSetFocus dim=null");
    }
    return S_OK;
}

bool CVoimeTextService::mods_match() const {
    UINT cur = 0;
    if (GetKeyState(VK_CONTROL) & 0x8000) cur |= MOD_CONTROL;
    if (GetKeyState(VK_MENU) & 0x8000) cur |= MOD_ALT;
    if (GetKeyState(VK_SHIFT) & 0x8000) cur |= MOD_SHIFT;
    if ((GetKeyState(VK_LWIN) | GetKeyState(VK_RWIN)) & 0x8000) cur |= MOD_WIN;
    return cur == ptt_mods_;
}

STDMETHODIMP CVoimeTextService::OnTestKeyDown(ITfContext*, WPARAM wParam, LPARAM,
                                              BOOL* pfEaten) {
    if (!key_seen_logged_) {
        key_seen_logged_ = true;
        tsf_log("first key event vk=0x%x", (UINT)wParam);
    }
    // 仅在 IPC 已连接时吃掉 PTT（Agent 不在时把组合键还给应用，如编辑器快捷键）
    *pfEaten = (is_ptt(wParam) && mods_match() && ipc_.connected()) ? TRUE : FALSE;
    return S_OK;
}

STDMETHODIMP CVoimeTextService::OnKeyDown(ITfContext*, WPARAM wParam, LPARAM lParam,
                                          BOOL* pfEaten) {
    *pfEaten = FALSE;
    if (is_ptt(wParam) && mods_match() && ipc_.connected()) {
        *pfEaten = TRUE;
        // lParam bit30=上次按键状态：抑制系统自动重复；ptt_down_ 状态机双保险
        if (!ptt_down_ && !(lParam & 0x40000000)) {
            ptt_down_ = true;
            ipc_.send("start_session");
            tsf_log("PTT down -> start_session");
        }
    }
    return S_OK;
}

STDMETHODIMP CVoimeTextService::OnTestKeyUp(ITfContext*, WPARAM wParam, LPARAM,
                                            BOOL* pfEaten) {
    *pfEaten = (is_ptt(wParam) && ptt_down_) ? TRUE : FALSE;
    return S_OK;
}

STDMETHODIMP CVoimeTextService::OnKeyUp(ITfContext*, WPARAM wParam, LPARAM,
                                        BOOL* pfEaten) {
    *pfEaten = FALSE;
    if (is_ptt(wParam) && ptt_down_) {
        ptt_down_ = false;
        *pfEaten = TRUE;
        ipc_.send("stop_session");
        tsf_log("PTT up -> stop_session");
    }
    return S_OK;
}

STDMETHODIMP CVoimeTextService::OnPreservedKey(ITfContext*, REFGUID, BOOL* pfEaten) {
    *pfEaten = FALSE;
    return S_OK;
}

LRESULT CALLBACK CVoimeTextService::wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    if (msg == WM_VOIME_COMMIT) {
        auto* self = (CVoimeTextService*)GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        auto* m = (CommitMsg*)lp;
        if (self && m) self->on_commit_msg(m->payload, m->request_id);
        delete m;
        return 0;
    }
    return DefWindowProcW(hwnd, msg, wp, lp);
}

void CVoimeTextService::on_commit_msg(const std::string& payload_utf8,
                                      const std::string& request_id) {
    tsf_log("on_commit_msg enter req=%s len=%zu", request_id.c_str(),
            payload_utf8.size());
    HRESULT hr = insert_text_async(utf8_to_wide(payload_utf8), request_id);
    if (FAILED(hr)) { // 排队失败：立即应答错误（排队成功则由 DoEditSession 应答）
        ipc_.respond(request_id, false, (int)hr);
        tsf_log("commit_text 排队失败 hr=0x%08lx", hr);
    }
}

void CVoimeTextService::respond_commit(const std::string& request_id, HRESULT hr) {
    ipc_.respond(request_id, SUCCEEDED(hr), (int)hr);
}

HRESULT CVoimeTextService::insert_text_async(const std::wstring& text,
                                             const std::string& request_id) {
    if (!ptim_ || text.empty()) return E_FAIL;
    // 优先 OnSetFocus 跟踪的真实焦点文档；未跟踪到则 GetFocus 兜底
    ITfDocumentMgr* dm = focus_dim_;
    if (dm) dm->AddRef();
    if (!dm) {
        HRESULT hr = ptim_->GetFocus(&dm);
        if (FAILED(hr) || !dm) return E_FAIL;
    }
    ITfContext* ctx = nullptr;
    HRESULT hr = dm->GetTop(&ctx);
    dm->Release();
    if (FAILED(hr) || !ctx) return E_FAIL;

    // 目标上下文在请求前取好并 AddRef 传入（DoEditSession 回调内禁止再进
    // TSF 串行化调用，见类注释 3）
    ctx->AddRef();
    auto* sess = new (std::nothrow) CInsertTextEditSession(ctx, this, text, request_id);
    if (!sess) {
        ctx->Release();
        ctx->Release();
        return E_OUTOFMEMORY;
    }
    HRESULT hr_sess = E_FAIL;
    hr = ctx->RequestEditSession(tid_, sess, TF_ES_ASYNC | TF_ES_READWRITE, &hr_sess);
    if (FAILED(hr))
        sess->Release(); // 排队失败：回收；成功则所有权移交，DoEditSession 末尾自释
    ctx->Release();
    tsf_log("RequestEditSession(ASYNC) hr=0x%08lx sess=0x%08lx", hr, hr_sess);
    return SUCCEEDED(hr) ? S_OK : hr;
}
