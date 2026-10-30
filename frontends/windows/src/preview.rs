use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, DrawTextW, EndPaint, FillRect, GetDC, GetMonitorInfoW,
    GetStockObject, GetSysColor, GetSysColorBrush, GetTextExtentPoint32W, MonitorFromWindow,
    ReleaseDC, SelectObject, SetBkMode, SetTextColor, COLOR_INFOBK, COLOR_INFOTEXT,
    DEFAULT_GUI_FONT, DT_CALCRECT, DT_LEFT, DT_NOPREFIX, DT_TOP, DT_WORDBREAK, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, RegisterClassW, SetLayeredWindowAttributes, SetWindowPos,
    SetWindowTextW, ShowWindow, HMENU, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SW_HIDE, SW_SHOWNOACTIVATE, WM_ERASEBKGND, WM_PAINT, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
};

use crate::target::Target;

const PADDING: i32 = 10;

pub struct Overlay {
    hwnd: HWND,
    enabled: bool,
    text: String,
}

impl Overlay {
    pub fn create() -> Result<Self> {
        register_class()?;
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST,
                w!("VoimeOverlay"),
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                HWND::default(),
                HMENU::default(),
                HINSTANCE::default(),
                None,
            )
        }
        .context("创建浮层窗口失败")?;
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 235, LWA_ALPHA);
        }
        Ok(Self {
            hwnd,
            enabled: true,
            text: String::new(),
        })
    }

    pub fn show(&mut self, text: &str, target: &Target) {
        if !self.enabled || text.is_empty() {
            return;
        }
        self.text.clear();
        self.text.push_str(text);
        let text_units: Vec<u16> = text.encode_utf16().collect();
        let (width, height) = measure(self.hwnd, &text_units, dpi(target.window));
        let mut wide = text_units;
        wide.push(0);
        let (mut x, mut y) = anchor(target);
        let monitor = unsafe { MonitorFromWindow(target.window, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            let work = info.rcWork;
            if x + width > work.right {
                x = (work.right - width).max(work.left);
            }
            if y + height > work.bottom {
                y = (work.bottom - height).max(work.top);
            }
            x = x.max(work.left);
            y = y.max(work.top);
        }
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(wide.as_ptr()));
            let _ = SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub fn hide(&mut self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.hide();
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let _ = FillRect(hdc, &rect, GetSysColorBrush(COLOR_INFOBK));
            let length = GetWindowTextLengthW(hwnd);
            let mut buffer = vec![0u16; (length.max(0) as usize) + 1];
            let copied = GetWindowTextW(hwnd, &mut buffer);
            buffer.truncate(copied.max(0) as usize);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(GetSysColor(COLOR_INFOTEXT)));
            SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));
            rect.left += PADDING;
            rect.top += PADDING;
            rect.right -= PADDING;
            rect.bottom -= PADDING;
            DrawTextW(
                hdc,
                &mut buffer,
                &mut rect,
                DT_LEFT | DT_TOP | DT_WORDBREAK | DT_NOPREFIX,
            );
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn register_class() -> Result<()> {
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: HINSTANCE::default(),
        lpszClassName: w!("VoimeOverlay"),
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let error = unsafe { GetLastError() };
        if error != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS {
            return Err(anyhow::anyhow!("注册浮层窗口类失败：{error:?}"));
        }
    }
    Ok(())
}

fn measure(hwnd: HWND, text: &[u16], dpi: u32) -> (i32, i32) {
    let scale = |value: i32| (value * dpi as i32 + 48) / 96;
    let max_text = scale(560);
    let hdc = unsafe { GetDC(hwnd) };
    let mut width = 0;
    let mut height = 0;
    if !hdc.0.is_null() {
        unsafe {
            SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));
            let mut size = Default::default();
            if GetTextExtentPoint32W(hdc, text, &mut size).as_bool() {
                width = size.cx;
                height = size.cy;
            }
            ReleaseDC(hwnd, hdc);
        }
    }
    let text_width = width.min(max_text);
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: text_width,
        bottom: 0,
    };
    let mut buffer = text.to_vec();
    if !buffer.is_empty() {
        let hdc = unsafe { GetDC(hwnd) };
        if !hdc.0.is_null() {
            unsafe {
                SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT));
                DrawTextW(
                    hdc,
                    &mut buffer,
                    &mut rect,
                    DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
                );
                ReleaseDC(hwnd, hdc);
            }
            height = rect.bottom - rect.top;
        }
    }
    (
        text_width.max(scale(120)) + 2 * scale(PADDING),
        height.max(scale(20)) + 2 * scale(PADDING),
    )
}

fn anchor(target: &Target) -> (i32, i32) {
    let thread = unsafe { GetWindowThreadProcessId(target.window, None) };
    if thread != 0 {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetGUIThreadInfo(thread, &mut info) }.is_ok()
            && !info.hwndCaret.0.is_null()
            && info.rcCaret.right > info.rcCaret.left
        {
            let mut point = POINT {
                x: info.rcCaret.left,
                y: info.rcCaret.bottom,
            };
            if unsafe { ClientToScreen(info.hwndCaret, &mut point) }.as_bool() {
                return (point.x, point.y + 4);
            }
        }
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(target.window, &mut rect) }.is_ok() {
        return (rect.left, rect.bottom + 4);
    }
    (0, 0)
}

fn dpi(hwnd: HWND) -> u32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        96
    } else {
        dpi
    }
}
