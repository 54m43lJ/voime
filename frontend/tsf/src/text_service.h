// text_service.h — Voime TSF 文本服务（P5-01 框架 / P5-04 按键拦截 / P5-05 文本注入）
//
// 控制反转（PRD 2.2）：
//   ITfKeyEventSink 拦截 PTT -> IPC start/stop_session -> Agent 被动计算；
//   IPC commit_text -> 隐藏窗口转投 UI 线程 -> ITfEditSession 注入。
//   焦点文档经 ITfThreadMgrEventSink::OnSetFocus 跟踪（GetFocus 可能返回
//   线程占位 docmgr，见 text_service.cpp 踩坑注释）。
#pragma once
#include "ipc_client.h"
#include "tsf_defs.h"

#include <msctf.h>
#include <string>

class CVoimeTextService : public ITfTextInputProcessor,
                          public ITfKeyEventSink,
                          public ITfThreadMgrEventSink {
public:
    CVoimeTextService();
    virtual ~CVoimeTextService();

    // IUnknown
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override;
    STDMETHODIMP_(ULONG) AddRef() override;
    STDMETHODIMP_(ULONG) Release() override;

    // ITfTextInputProcessor
    STDMETHODIMP Activate(ITfThreadMgr* ptim, TfClientId tid) override;
    STDMETHODIMP Deactivate() override;

    // ITfKeyEventSink
    STDMETHODIMP OnSetFocus(BOOL fForeground) override;
    STDMETHODIMP OnTestKeyDown(ITfContext* pic, WPARAM wParam, LPARAM lParam,
                               BOOL* pfEaten) override;
    STDMETHODIMP OnTestKeyUp(ITfContext* pic, WPARAM wParam, LPARAM lParam,
                             BOOL* pfEaten) override;
    STDMETHODIMP OnKeyDown(ITfContext* pic, WPARAM wParam, LPARAM lParam,
                           BOOL* pfEaten) override;
    STDMETHODIMP OnKeyUp(ITfContext* pic, WPARAM wParam, LPARAM lParam,
                         BOOL* pfEaten) override;
    STDMETHODIMP OnPreservedKey(ITfContext* pic, REFGUID rguid, BOOL* pfEaten) override;

    // ITfThreadMgrEventSink
    STDMETHODIMP OnInitDocumentMgr(ITfDocumentMgr* pdim) override;
    STDMETHODIMP OnUninitDocumentMgr(ITfDocumentMgr* pdim) override;
    STDMETHODIMP OnSetFocus(ITfDocumentMgr* pdimFocus,
                            ITfDocumentMgr* pdimPrevFocus) override;
    STDMETHODIMP OnPushContext(ITfContext* pic) override;
    STDMETHODIMP OnPopContext(ITfContext* pic) override;

    // 隐藏窗口消息回调（UI 线程）：排队异步编辑会话执行注入
    void on_commit_msg(const std::string& payload_utf8, const std::string& request_id);
    // 编辑会话完成回调：向 Agent 应答注入结果
    void respond_commit(const std::string& request_id, HRESULT hr);

private:
    bool is_ptt(WPARAM wp) const { return ptt_vk_ && (UINT)wp == ptt_vk_; }
    bool mods_match() const; // 当前修饰键状态与配置精确一致
    HRESULT insert_text_async(const std::wstring& text, const std::string& request_id);
    static LRESULT CALLBACK wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp);

    LONG ref_ = 1;
    ITfThreadMgr* ptim_ = nullptr;
    TfClientId tid_ = 0;
    bool key_advised_ = false;
    DWORD tm_sink_cookie_ = TF_INVALID_COOKIE;
    ITfDocumentMgr* focus_dim_ = nullptr; // OnSetFocus 跟踪的真实焦点文档
    HWND hwnd_ = nullptr; // 隐藏消息窗口（与 Activate 同线程/套间）
    IpcClient ipc_;
    UINT ptt_mods_ = 0;
    UINT ptt_vk_ = 0;
    bool ptt_down_ = false;
    bool key_seen_logged_ = false; // 诊断：首个按键事件只记一次
};
