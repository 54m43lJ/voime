use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    FindWindowW, GetCursorPos, GetWindowLongPtrW, LoadIconW, MessageBoxW, PostMessageW,
    PostQuitMessage, RegisterClassW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow,
    TrackPopupMenu, GWLP_USERDATA, HMENU, IDI_APPLICATION, MB_ICONERROR, MB_OK, MENU_ITEM_FLAGS,
    MF_CHECKED, MF_DISABLED, MF_POPUP, MF_SEPARATOR, MF_STRING, SW_SHOWNOACTIVATE, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::Frontend;

const WINDOW_CLASS: PCWSTR = w!("VoimeTrayWindow");
const ICON_ID: u32 = 1;
const CALLBACK_MESSAGE: u32 = WM_APP + 1;
const ACTIVATE_MESSAGE: u32 = WM_APP + 2;

const ID_SETTINGS: usize = 1000;
const ID_PTT: usize = 1001;
const ID_TOGGLE: usize = 1002;
const ID_CONTINUOUS: usize = 1003;
const ID_OVERLAY: usize = 1101;
const ID_START_AT_LOGIN: usize = 1102;
const ID_START_DAEMON: usize = 1201;
const ID_STOP_DAEMON: usize = 1202;
const ID_DOCTOR: usize = 1203;
const ID_QUIT: usize = 1204;
const ID_HOTKEY_BASE: usize = 2000;

const HOTKEYS: &[(&str, &str)] = &[
    ("Ctrl+Alt+Space", "Ctrl+Alt+Space"),
    ("Ctrl+Shift+Space", "Ctrl+Shift+Space"),
    ("Alt+Space", "Alt+Space"),
    ("Ctrl+Alt+F2", "Ctrl+Alt+F2"),
    ("", "禁用"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrontendStatus {
    Connected,
    Disconnected,
    Error,
}

#[derive(Clone, Copy, Debug)]
enum TrayEvent {
    ShowMenu,
    ShowSettings,
}

pub struct Tray {
    hwnd: HWND,
    queue: Arc<Mutex<VecDeque<TrayEvent>>>,
    icon: HICON,
    icon_added: bool,
    status: FrontendStatus,
}

impl Tray {
    pub fn create(frontend: &mut Frontend) -> Result<Self> {
        register_class()?;
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                WINDOW_CLASS,
                w!("Voime"),
                WS_POPUP,
                -32000,
                -32000,
                1,
                1,
                HWND::default(),
                HMENU::default(),
                HINSTANCE::default(),
                None,
            )
        }
        .context("创建托盘消息窗口失败")?;
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Arc::as_ptr(&queue) as isize);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        let icon = unsafe { LoadIconW(HINSTANCE::default(), IDI_APPLICATION) }
            .context("加载托盘图标失败")?;
        let mut tray = Self {
            hwnd,
            queue,
            icon,
            icon_added: false,
            status: FrontendStatus::Disconnected,
        };
        tray.add_icon("Voime — daemon 未连接")?;
        let _ = frontend;
        Ok(tray)
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn update_status(&mut self, status: FrontendStatus) {
        if self.status == status && self.icon_added {
            return;
        }
        self.status = status;
        let tip = match status {
            FrontendStatus::Connected => "Voime — 已连接",
            FrontendStatus::Disconnected => "Voime — daemon 未连接",
            FrontendStatus::Error => "Voime — 错误",
        };
        let mut data = self.base_data();
        data.uFlags = NIF_TIP;
        copy_tip(&mut data.szTip, tip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    pub fn poll_events(&mut self, frontend: &mut Frontend) {
        loop {
            let event = self.queue.lock().expect("tray queue poisoned").pop_front();
            match event {
                Some(TrayEvent::ShowMenu) => self.show_menu(frontend),
                Some(TrayEvent::ShowSettings) => frontend.open_settings(),
                None => break,
            }
        }
    }

    fn add_icon(&mut self, tip: &str) -> Result<()> {
        let mut data = self.base_data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = CALLBACK_MESSAGE;
        data.hIcon = self.icon;
        copy_tip(&mut data.szTip, tip);
        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
        if !added.as_bool() {
            return Err(anyhow::anyhow!("添加托盘图标失败"));
        }
        self.icon_added = true;
        Ok(())
    }

    fn base_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ICON_ID,
            ..Default::default()
        }
    }

    fn show_menu(&mut self, frontend: &mut Frontend) {
        let command = unsafe { self.track_menu(frontend) };
        let Some(command) = command else {
            return;
        };
        if (ID_HOTKEY_BASE..ID_HOTKEY_BASE + HOTKEYS.len()).contains(&command) {
            let spec = HOTKEYS[command - ID_HOTKEY_BASE].0;
            if let Err(error) = frontend.set_hotkey(spec) {
                Self::show_error(&error);
            }
            return;
        }
        let result = match command {
            ID_PTT => frontend.set_mode(voime_ipc::Mode::Ptt),
            ID_TOGGLE => frontend.set_mode(voime_ipc::Mode::Toggle),
            ID_CONTINUOUS => frontend.set_mode(voime_ipc::Mode::Continuous),
            ID_OVERLAY => {
                let enabled = !frontend.config.overlay_enabled;
                frontend.set_overlay_enabled(enabled)
            }
            ID_START_AT_LOGIN => {
                let enabled = !frontend.config.start_at_login;
                frontend.set_start_at_login(enabled)
            }
            ID_SETTINGS => {
                frontend.open_settings();
                Ok(())
            }
            ID_START_DAEMON => {
                frontend.start_daemon();
                Ok(())
            }
            ID_STOP_DAEMON => {
                frontend.stop_daemon();
                Ok(())
            }
            ID_DOCTOR => {
                frontend.run_doctor();
                Ok(())
            }
            ID_QUIT => {
                unsafe {
                    PostQuitMessage(0);
                }
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            Self::show_error(&error);
        }
    }

    unsafe fn track_menu(&self, frontend: &Frontend) -> Option<usize> {
        let menu = CreatePopupMenu().ok()?;
        let status = match self.status {
            FrontendStatus::Connected => "状态：已连接",
            FrontendStatus::Disconnected => "状态：daemon 未连接",
            FrontendStatus::Error => "状态：错误",
        };
        let status_text = to_wide(status);
        append(
            menu,
            MF_STRING | MF_DISABLED,
            0,
            PCWSTR(status_text.as_ptr()),
        );
        append(menu, MF_SEPARATOR, 0, PCWSTR::null());
        append(menu, MF_STRING, ID_SETTINGS, w!("设置..."));
        append(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let modes = CreatePopupMenu().ok()?;
        append_checked(
            modes,
            MF_STRING,
            ID_PTT as u32,
            w!("PTT"),
            frontend.config.mode == voime_ipc::Mode::Ptt,
        );
        append_checked(
            modes,
            MF_STRING,
            ID_TOGGLE as u32,
            w!("Toggle"),
            frontend.config.mode == voime_ipc::Mode::Toggle,
        );
        append_checked(
            modes,
            MF_STRING,
            ID_CONTINUOUS as u32,
            w!("Continuous"),
            frontend.config.mode == voime_ipc::Mode::Continuous,
        );
        append(menu, MF_POPUP, modes.0 as usize, w!("模式"));

        let hotkeys = CreatePopupMenu().ok()?;
        let current = frontend.config.hotkey.to_string();
        for (index, (spec, label)) in HOTKEYS.iter().enumerate() {
            let checked = if spec.is_empty() {
                current.is_empty()
            } else {
                current.eq_ignore_ascii_case(spec)
            };
            let label = to_wide(label);
            append_checked(
                hotkeys,
                MF_STRING,
                (ID_HOTKEY_BASE + index) as u32,
                PCWSTR(label.as_ptr()),
                checked,
            );
        }
        append(menu, MF_POPUP, hotkeys.0 as usize, w!("热键"));

        append_checked(
            menu,
            MF_STRING,
            ID_OVERLAY as u32,
            w!("显示浮层"),
            frontend.config.overlay_enabled,
        );
        append_checked(
            menu,
            MF_STRING,
            ID_START_AT_LOGIN as u32,
            w!("登录时启动"),
            frontend.config.start_at_login,
        );
        append(menu, MF_SEPARATOR, 0, PCWSTR::null());
        append(menu, MF_STRING, ID_START_DAEMON, w!("启动 daemon"));
        append(menu, MF_STRING, ID_STOP_DAEMON, w!("停止 daemon"));
        append(menu, MF_STRING, ID_DOCTOR, w!("诊断..."));
        append(menu, MF_SEPARATOR, 0, PCWSTR::null());
        append(menu, MF_STRING, ID_QUIT, w!("退出"));

        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(self.hwnd);
        let selected = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            self.hwnd,
            None,
        );
        let _ = PostMessageW(self.hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        let command = selected.0 as usize;
        if command == 0 {
            None
        } else {
            Some(command)
        }
    }

    fn show_error(error: &anyhow::Error) {
        let text = to_wide(&format!("{error:#}"));
        unsafe {
            MessageBoxW(
                HWND::default(),
                PCWSTR(text.as_ptr()),
                w!("Voime"),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if self.icon_added {
            let data = self.base_data();
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
        }
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
    if message == CALLBACK_MESSAGE {
        let mouse = (lparam.0 as u32) & 0xffff;
        if mouse == WM_RBUTTONUP || mouse == WM_CONTEXTMENU {
            push_event(hwnd, TrayEvent::ShowMenu);
        } else if mouse == WM_LBUTTONUP {
            push_event(hwnd, TrayEvent::ShowSettings);
        }
        return LRESULT(0);
    }
    if message == ACTIVATE_MESSAGE {
        push_event(hwnd, TrayEvent::ShowMenu);
        return LRESULT(0);
    }
    if message == WM_CLOSE {
        unsafe {
            let _ = DestroyWindow(hwnd);
            PostQuitMessage(0);
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn push_event(hwnd: HWND, event: TrayEvent) {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if pointer == 0 {
        return;
    }
    let queue = unsafe { &*(pointer as *const Mutex<VecDeque<TrayEvent>>) };
    if let Ok(mut queue) = queue.lock() {
        queue.push_back(event);
    }
}

fn register_class() -> Result<()> {
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: HINSTANCE::default(),
        lpszClassName: WINDOW_CLASS,
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let error = unsafe { GetLastError() };
        if error != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS {
            return Err(anyhow::anyhow!("注册托盘窗口类失败：{error:?}"));
        }
    }
    Ok(())
}

fn append(menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: PCWSTR) {
    unsafe {
        let _ = AppendMenuW(menu, flags, id, text);
    }
}

fn append_checked(menu: HMENU, flags: MENU_ITEM_FLAGS, id: u32, text: PCWSTR, checked: bool) {
    let flags = if checked { flags | MF_CHECKED } else { flags };
    unsafe {
        let _ = AppendMenuW(menu, flags, id as usize, text);
    }
}

fn copy_tip(target: &mut [u16; 128], text: &str) {
    for slot in target.iter_mut() {
        *slot = 0;
    }
    for (slot, unit) in target.iter_mut().zip(text.encode_utf16()).take(127) {
        *slot = unit;
    }
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn activate_existing() {
    let Ok(hwnd) = (unsafe { FindWindowW(WINDOW_CLASS, PCWSTR::null()) }) else {
        return;
    };
    unsafe {
        let _ = PostMessageW(hwnd, ACTIVATE_MESSAGE, WPARAM(0), LPARAM(0));
    }
}
