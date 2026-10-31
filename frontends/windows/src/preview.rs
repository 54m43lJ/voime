use std::ffi::c_void;

use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_SUCCESS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateFontW, CreatePen, CreateRoundRectRgn, CreateSolidBrush,
    DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetMonitorInfoW, GetStockObject,
    GetTextExtentPoint32W, MonitorFromWindow, ReleaseDC, RoundRect, SelectObject, SetBkMode,
    SetTextColor, SetWindowRgn, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_GUI_FONT, DEFAULT_PITCH, DT_CALCRECT, DT_LEFT, DT_NOPREFIX, DT_TOP, DT_WORDBREAK,
    FW_NORMAL, HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST, NULL_BRUSH, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, PS_SOLID, TRANSPARENT,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, RegisterClassW,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
    CS_DROPSHADOW, GWLP_USERDATA, HMENU, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SW_HIDE, SW_SHOWNOACTIVATE, WM_ERASEBKGND, WM_PAINT, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
};

use crate::target::Target;

const PADDING: i32 = 12;
const CORNER_RADIUS: i32 = 12;
const ACCENT_WIDTH: i32 = 3;
const ACCENT_GAP: i32 = 9;
const FONT_SIZE: i32 = 15;
const MAX_TEXT_WIDTH: i32 = 600;
const ALPHA: u8 = 240;

struct Palette {
    background: COLORREF,
    border: COLORREF,
    text: COLORREF,
    accent: COLORREF,
}

pub struct Overlay {
    hwnd: HWND,
    enabled: bool,
    text: String,
    font: HFONT,
    font_dpi: u32,
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
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), ALPHA, LWA_ALPHA);
        }
        let mut overlay = Self {
            hwnd,
            enabled: true,
            text: String::new(),
            font: HFONT::default(),
            font_dpi: 0,
        };
        overlay.ensure_font(96);
        Ok(overlay)
    }

    pub fn show(&mut self, text: &str, target: &Target) {
        if !self.enabled || text.is_empty() {
            return;
        }
        self.show_text(text, target);
    }

    pub fn show_empty(&mut self, target: &Target) {
        if !self.enabled {
            return;
        }
        self.show_text("", target);
    }

    fn show_text(&mut self, text: &str, target: &Target) {
        self.text.clear();
        self.text.push_str(text);
        let dpi = dpi(target.window);
        self.ensure_font(dpi);
        let text_units: Vec<u16> = text.encode_utf16().collect();
        let (width, height) = measure(self.hwnd, &text_units, dpi, self.font);
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
        let radius = scale(CORNER_RADIUS, dpi) * 2;
        let region = unsafe { CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius) };
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(wide.as_ptr()));
            if !region.is_invalid() && SetWindowRgn(self.hwnd, region, true) == 0 {
                let _ = DeleteObject(region);
            }
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

    fn ensure_font(&mut self, dpi: u32) {
        if self.font_dpi == dpi && !self.font.is_invalid() {
            return;
        }
        if !self.font.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.font);
            }
        }
        let height = -((FONT_SIZE * dpi as i32 + 48) / 96);
        let font = unsafe {
            CreateFontW(
                height,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                CLEARTYPE_QUALITY.0 as u32,
                DEFAULT_PITCH.0 as u32,
                w!("Microsoft YaHei UI"),
            )
        };
        self.font = font;
        self.font_dpi = dpi;
        unsafe {
            SetWindowLongPtrW(
                self.hwnd,
                GWLP_USERDATA,
                if font.is_invalid() {
                    0
                } else {
                    font.0 as isize
                },
            );
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            if !self.font.is_invalid() {
                let _ = DeleteObject(self.font);
            }
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
            let dpi = dpi(hwnd);
            let palette = palette();
            let padding = scale(PADDING, dpi);
            let bar_width = scale(ACCENT_WIDTH, dpi);
            let gap = scale(ACCENT_GAP, dpi);
            let radius = scale(CORNER_RADIUS, dpi) * 2;

            unsafe {
                let background = CreateSolidBrush(palette.background);
                FillRect(hdc, &rect, background);
                let _ = DeleteObject(background);

                let border = CreatePen(PS_SOLID, scale(1, dpi), palette.border);
                let old_pen = SelectObject(hdc, border);
                let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
                let _ = RoundRect(hdc, 0, 0, rect.right, rect.bottom, radius, radius);
                let _ = SelectObject(hdc, old_brush);
                let _ = SelectObject(hdc, old_pen);
                let _ = DeleteObject(border);

                let accent = CreateSolidBrush(palette.accent);
                let bar = RECT {
                    left: padding,
                    top: padding,
                    right: padding + bar_width,
                    bottom: rect.bottom - padding,
                };
                FillRect(hdc, &bar, accent);
                let _ = DeleteObject(accent);

                let font = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                let old_font = if font != 0 {
                    SelectObject(hdc, HFONT(font as *mut c_void))
                } else {
                    SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT))
                };
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, palette.text);
                let length = GetWindowTextLengthW(hwnd);
                let mut buffer = vec![0u16; (length.max(0) as usize) + 1];
                let copied = GetWindowTextW(hwnd, &mut buffer);
                buffer.truncate(copied.max(0) as usize);
                let mut text_rect = RECT {
                    left: rect.left + padding + bar_width + gap,
                    top: rect.top + padding,
                    right: rect.right - padding,
                    bottom: rect.bottom - padding,
                };
                DrawTextW(
                    hdc,
                    &mut buffer,
                    &mut text_rect,
                    DT_LEFT | DT_TOP | DT_WORDBREAK | DT_NOPREFIX,
                );
                let _ = SelectObject(hdc, old_font);
            }
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn register_class() -> Result<()> {
    let class = WNDCLASSW {
        style: CS_DROPSHADOW,
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

fn measure(hwnd: HWND, text: &[u16], dpi: u32, font: HFONT) -> (i32, i32) {
    let padding = scale(PADDING, dpi);
    let bar_width = scale(ACCENT_WIDTH, dpi);
    let gap = scale(ACCENT_GAP, dpi);
    let max_text = scale(MAX_TEXT_WIDTH, dpi);
    let hdc = unsafe { GetDC(hwnd) };
    let mut width = 0;
    let mut height = 0;
    if !hdc.0.is_null() {
        unsafe {
            let old_font = if font.is_invalid() {
                SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT))
            } else {
                SelectObject(hdc, font)
            };
            let mut size = Default::default();
            if GetTextExtentPoint32W(hdc, text, &mut size).as_bool() {
                width = size.cx;
                height = size.cy;
            }
            let _ = SelectObject(hdc, old_font);
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
                let old_font = if font.is_invalid() {
                    SelectObject(hdc, GetStockObject(DEFAULT_GUI_FONT))
                } else {
                    SelectObject(hdc, font)
                };
                DrawTextW(
                    hdc,
                    &mut buffer,
                    &mut rect,
                    DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
                );
                let _ = SelectObject(hdc, old_font);
                ReleaseDC(hwnd, hdc);
            }
            height = rect.bottom - rect.top;
        }
    }
    (
        text_width.max(scale(120, dpi)) + 2 * padding + bar_width + gap,
        height.max(scale(20, dpi)) + 2 * padding,
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

fn scale(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

fn palette() -> Palette {
    if system_dark_mode() {
        Palette {
            background: rgb(32, 33, 36),
            border: rgb(60, 64, 67),
            text: rgb(232, 234, 237),
            accent: rgb(122, 155, 255),
        }
    } else {
        Palette {
            background: rgb(250, 250, 252),
            border: rgb(218, 220, 224),
            text: rgb(32, 33, 36),
            accent: rgb(82, 125, 255),
        }
    }
}

fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(red as u32 | (green as u32) << 8 | (blue as u32) << 16)
}

fn system_dark_mode() -> bool {
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut c_void),
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && value == 0
}
