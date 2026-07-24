// overlay.h — Direct2D 悬浮窗（CPP-06，PRD 2.3 录音悬浮窗）
//
// 无边框 / 逐像素半透明 / 置顶 / 不抢焦点 / 点击穿透，屏幕底部居中。
// 渲染：32bpp DIB + ID2D1DCRenderTarget（预乘 alpha）-> UpdateLayeredWindow。
// 淡入淡出：定时器步进常量 alpha（不重绘内容）。
#pragma once
#include <string>
#include <windows.h>

class Overlay {
public:
    bool create(HINSTANCE inst);
    void destroy();

    void show(const std::wstring& status); // 淡入，显示状态行（如“正在听...”）
    void set_text(const std::wstring& text);
    void set_status(const std::wstring& status);
    void hide(); // 淡出后隐藏

    HWND hwnd() const { return hwnd_; }
    bool visible() const { return visible_; }

    static const int kWidth = 640;
    static const int kHeight = 96;

private:
    static LRESULT CALLBACK wnd_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp);
    void render();          // D2D 重绘到 DIB
    void apply(int alpha);  // UpdateLayeredWindow
    void fade_to(int target);

    HWND hwnd_ = nullptr;
    HINSTANCE inst_ = nullptr;
    bool visible_ = false;
    int alpha_ = 0;
    int fade_target_ = 0;

    std::wstring status_;
    std::wstring text_;

    // GDI/D2D 资源
    HDC mem_dc_ = nullptr;
    HBITMAP dib_ = nullptr;
    void* d2d_rt_ = nullptr;     // ID2D1DCRenderTarget*
    void* d2d_factory_ = nullptr; // ID2D1Factory*
    void* brush_bg_ = nullptr;   // ID2D1SolidColorBrush*
    void* brush_fg_ = nullptr;   // ID2D1SolidColorBrush*
    void* brush_dim_ = nullptr;  // ID2D1SolidColorBrush*
    void* dw_factory_ = nullptr; // IDWriteFactory*
    void* fmt_status_ = nullptr; // IDWriteTextFormat*（状态行）
    void* fmt_text_ = nullptr;   // IDWriteTextFormat*（内容行）
};
