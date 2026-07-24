// overlay.cpp — Direct2D 逐像素半透明悬浮窗实现
#include "overlay.h"
#include "util.h"

#include <d2d1.h>
#include <dwrite.h>
#include <vector>

static const wchar_t* kWndClass = L"VoimeOverlayWnd";
static const UINT_PTR kFadeTimer = 1;
static const UINT kFadeStepMs = 20;
static const int kFadeDelta = 32; // 每步 alpha 增量（约 160ms 完成）

LRESULT CALLBACK Overlay::wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    Overlay* self = (Overlay*)GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if (msg == WM_TIMER && self && wp == kFadeTimer) {
        if (self->alpha_ < self->fade_target_) {
            self->alpha_ = (self->alpha_ + kFadeDelta > self->fade_target_)
                               ? self->fade_target_ : self->alpha_ + kFadeDelta;
            self->apply(self->alpha_);
        } else if (self->alpha_ > self->fade_target_) {
            self->alpha_ = (self->alpha_ - kFadeDelta < self->fade_target_)
                               ? self->fade_target_ : self->alpha_ - kFadeDelta;
            self->apply(self->alpha_);
        }
        if (self->alpha_ == self->fade_target_) {
            KillTimer(hwnd, kFadeTimer);
            if (self->fade_target_ == 0) {
                ShowWindow(hwnd, SW_HIDE);
                self->visible_ = false;
            }
        }
        return 0;
    }
    if (msg == WM_DESTROY) {
        return 0;
    }
    return DefWindowProcW(hwnd, msg, wp, lp);
}

bool Overlay::create(HINSTANCE inst) {
    inst_ = inst;
    WNDCLASSEXW wc{ sizeof(wc) };
    wc.lpfnWndProc = &Overlay::wnd_proc;
    wc.hInstance = inst;
    wc.lpszClassName = kWndClass;
    RegisterClassExW(&wc);

    hwnd_ = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
        kWndClass, L"Voime Overlay", WS_POPUP, 0, 0, kWidth, kHeight,
        nullptr, nullptr, inst, nullptr);
    if (!hwnd_) return false;
    SetWindowLongPtrW(hwnd_, GWLP_USERDATA, (LONG_PTR)this);

    // 32bpp DIB（顶向下）
    BITMAPINFO bmi{};
    bmi.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    bmi.bmiHeader.biWidth = kWidth;
    bmi.bmiHeader.biHeight = -kHeight;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    HDC screen = GetDC(nullptr);
    mem_dc_ = CreateCompatibleDC(screen);
    void* bits = nullptr;
    dib_ = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &bits, nullptr, 0);
    ReleaseDC(nullptr, screen);
    if (!mem_dc_ || !dib_) return false;
    SelectObject(mem_dc_, dib_);

    // D2D 工厂 + DC 渲染目标
    if (FAILED(D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, __uuidof(ID2D1Factory),
                                 nullptr, (void**)&d2d_factory_)))
        return false;
    auto* factory = (ID2D1Factory*)d2d_factory_;
    D2D1_RENDER_TARGET_PROPERTIES props = D2D1::RenderTargetProperties(
        D2D1_RENDER_TARGET_TYPE_DEFAULT,
        D2D1::PixelFormat(DXGI_FORMAT_B8G8R8A8_UNORM, D2D1_ALPHA_MODE_PREMULTIPLIED));
    ID2D1DCRenderTarget* rt = nullptr;
    if (FAILED(factory->CreateDCRenderTarget(&props, &rt))) return false;
    d2d_rt_ = rt;
    RECT rc{ 0, 0, kWidth, kHeight };
    if (FAILED(rt->BindDC(mem_dc_, &rc))) return false;

    ID2D1SolidColorBrush *bg = nullptr, *fg = nullptr, *dim = nullptr;
    rt->CreateSolidColorBrush(D2D1::ColorF(0.0f, 0.0f, 0.0f, 0.62f), &bg);
    rt->CreateSolidColorBrush(D2D1::ColorF(1.0f, 1.0f, 1.0f, 1.0f), &fg);
    rt->CreateSolidColorBrush(D2D1::ColorF(0.55f, 0.85f, 0.55f, 1.0f), &dim);
    if (!bg || !fg || !dim) return false;
    brush_bg_ = bg; brush_fg_ = fg; brush_dim_ = dim;

    // DirectWrite
    if (FAILED(DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED, __uuidof(IDWriteFactory),
                                   (IUnknown**)&dw_factory_)))
        return false;
    auto* dw = (IDWriteFactory*)dw_factory_;
    IDWriteTextFormat *fs = nullptr, *ft = nullptr;
    if (FAILED(dw->CreateTextFormat(L"Microsoft YaHei UI", nullptr, DWRITE_FONT_WEIGHT_NORMAL,
                                    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 13.0f,
                                    L"zh-cn", &fs)))
        return false;
    if (FAILED(dw->CreateTextFormat(L"Microsoft YaHei UI", nullptr, DWRITE_FONT_WEIGHT_NORMAL,
                                    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 20.0f,
                                    L"zh-cn", &ft)))
        return false;
    fs->SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
    ft->SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
    fmt_status_ = fs; fmt_text_ = ft;
    return true;
}

void Overlay::destroy() {
    if (hwnd_) DestroyWindow(hwnd_);
    hwnd_ = nullptr;
    auto rel = [](void*& p) { if (p) { ((IUnknown*)p)->Release(); p = nullptr; } };
    rel(fmt_text_); rel(fmt_status_); rel(dw_factory_);
    rel(brush_dim_); rel(brush_fg_); rel(brush_bg_);
    rel(d2d_rt_); rel(d2d_factory_);
    if (dib_) { DeleteObject(dib_); dib_ = nullptr; }
    if (mem_dc_) { DeleteDC(mem_dc_); mem_dc_ = nullptr; }
}

// 超长截头（保留尾部，模拟滚动）
static std::wstring truncate_head(IDWriteFactory* dw, IDWriteTextFormat* fmt,
                                  const std::wstring& s, float max_w) {
    if (s.empty()) return s;
    IDWriteTextLayout* layout = nullptr;
    if (FAILED(dw->CreateTextLayout(s.data(), (UINT32)s.size(), fmt, max_w, 100.0f, &layout)))
        return s;
    DWRITE_TEXT_METRICS m{};
    layout->GetMetrics(&m);
    layout->Release();
    if (m.width <= max_w) return s;
    // 二分最小保留长度
    size_t lo = 0, hi = s.size();
    while (lo < hi) {
        size_t mid = (lo + hi) / 2;
        std::wstring t = L"…" + s.substr(s.size() - mid);
        if (FAILED(dw->CreateTextLayout(t.data(), (UINT32)t.size(), fmt, max_w, 100.0f, &layout)))
            break;
        layout->GetMetrics(&m);
        layout->Release();
        if (m.width <= max_w) lo = mid + 1; else hi = mid;
    }
    size_t keep = lo > 0 ? lo - 1 : 0;
    return L"…" + s.substr(s.size() - keep);
}

void Overlay::render() {
    auto* rt = (ID2D1DCRenderTarget*)d2d_rt_;
    auto* bg = (ID2D1SolidColorBrush*)brush_bg_;
    auto* fg = (ID2D1SolidColorBrush*)brush_fg_;
    auto* dim = (ID2D1SolidColorBrush*)brush_dim_;
    auto* dw = (IDWriteFactory*)dw_factory_;
    auto* fs = (IDWriteTextFormat*)fmt_status_;
    auto* ft = (IDWriteTextFormat*)fmt_text_;
    if (!rt || !bg || !fg || !dim || !dw || !fs || !ft) return;

    rt->BeginDraw();
    rt->Clear(D2D1::ColorF(0, 0, 0, 0)); // 全透明
    D2D1_ROUNDED_RECT rr = D2D1::RoundedRect(D2D1::RectF(0.5f, 0.5f, kWidth - 0.5f, kHeight - 0.5f),
                                             14.0f, 14.0f);
    rt->FillRoundedRectangle(&rr, bg);

    D2D1_RECT_F rc_status = D2D1::RectF(18.0f, 10.0f, kWidth - 18.0f, 32.0f);
    rt->DrawTextW(status_.c_str(), (UINT32)status_.size(), fs, &rc_status, dim);

    std::wstring shown = truncate_head(dw, ft, text_, kWidth - 36.0f);
    D2D1_RECT_F rc_text = D2D1::RectF(18.0f, 38.0f, kWidth - 18.0f, kHeight - 12.0f);
    rt->DrawTextW(shown.c_str(), (UINT32)shown.size(), ft, &rc_text, fg);
    rt->EndDraw();
}

void Overlay::apply(int alpha) {
    if (!mem_dc_) return;
    // 底部居中（工作区）
    RECT work{};
    SystemParametersInfoW(SPI_GETWORKAREA, 0, &work, 0);
    int x = (work.left + work.right - kWidth) / 2;
    int y = work.bottom - kHeight - 48;
    POINT dst{ x, y }, src{ 0, 0 };
    SIZE sz{ kWidth, kHeight };
    BLENDFUNCTION bf{ AC_SRC_OVER, 0, (BYTE)alpha, AC_SRC_ALPHA };
    UpdateLayeredWindow(hwnd_, nullptr, &dst, &sz, mem_dc_, &src, 0, &bf, ULW_ALPHA);
}

void Overlay::fade_to(int target) {
    fade_target_ = target;
    SetTimer(hwnd_, kFadeTimer, kFadeStepMs, nullptr);
}

void Overlay::show(const std::wstring& status) {
    status_ = status;
    render();
    if (!visible_) {
        visible_ = true;
        alpha_ = 0;
        apply(0);
        ShowWindow(hwnd_, SW_SHOWNOACTIVATE);
    }
    fade_to(255);
}

void Overlay::set_text(const std::wstring& text) {
    text_ = text;
    if (visible_) {
        render();
        apply(alpha_);
    }
}

void Overlay::set_status(const std::wstring& status) {
    status_ = status;
    if (visible_) {
        render();
        apply(alpha_);
    }
}

void Overlay::hide() {
    if (!visible_) return;
    fade_to(0);
}
