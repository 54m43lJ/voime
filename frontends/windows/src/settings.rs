use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use crossbeam_channel::Sender;
use voime_ipc::Mode;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetStockObject, GetSysColorBrush, UpdateWindow, COLOR_BTNFACE, DEFAULT_GUI_FONT,
};
use windows::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetMessageW, GetSystemMetrics, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW,
    IsDialogMessageW, MessageBoxW, PostQuitMessage, RegisterClassW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, BM_GETCHECK, BM_SETCHECK,
    BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_GROUPBOX, BS_PUSHBUTTON, CBS_DROPDOWNLIST,
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, ES_AUTOHSCROLL, GWLP_USERDATA, HMENU, MB_ICONERROR,
    MB_ICONINFORMATION, MB_OK, MSG, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, WM_CLOSE, WM_COMMAND,
    WM_DESTROY, WM_SETFONT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_OVERLAPPED, WS_SYSMENU,
    WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use crate::config::Config;
use crate::hotkey;
use crate::tray::FrontendStatus;

const ID_MODE: i32 = 1001;
const ID_HOTKEY: i32 = 1002;
const ID_OVERLAY: i32 = 1003;
const ID_START_AT_LOGIN: i32 = 1004;
const ID_SAVE: i32 = 1101;
const ID_CANCEL: i32 = 1102;
const ID_START_DAEMON: i32 = 1201;
const ID_STOP_DAEMON: i32 = 1202;
const ID_DOCTOR: i32 = 1203;

pub enum SettingsAction {
    Save(Config),
    StartDaemon,
    StopDaemon,
    RunDoctor,
}

pub fn open(
    config: Config,
    status: FrontendStatus,
    sender: Sender<SettingsAction>,
    open_flag: Arc<AtomicBool>,
) -> Result<()> {
    std::thread::Builder::new()
        .name("voime-settings".to_string())
        .spawn(move || {
            let _guard = OpenGuard(open_flag);
            if let Err(error) = run_window(config, status, sender) {
                show_error_text(HWND::default(), &format!("{error:#}"));
            }
        })
        .context("创建设置窗口线程失败")?;
    Ok(())
}

pub fn show_error(error: &anyhow::Error) {
    show_error_text(HWND::default(), &format!("{error:#}"));
}

pub fn show_text(title: &str, text: &str) {
    let title = to_wide(title);
    let text = to_wide(text);
    unsafe {
        MessageBoxW(
            HWND::default(),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

struct OpenGuard(Arc<AtomicBool>);

impl Drop for OpenGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

struct SettingsWindow {
    sender: Sender<SettingsAction>,
    mode: HWND,
    hotkey: HWND,
    overlay: HWND,
    start_at_login: HWND,
    status: HWND,
}

fn run_window(
    config: Config,
    status: FrontendStatus,
    sender: Sender<SettingsAction>,
) -> Result<()> {
    register_class()?;
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 440,
        bottom: 330,
    };
    unsafe {
        AdjustWindowRect(&mut rect, style, false).context("计算设置窗口大小失败")?;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            w!("VoimeSettings"),
            w!("Voime 设置"),
            style,
            (screen_width - width) / 2,
            (screen_height - height) / 2,
            width,
            height,
            HWND::default(),
            HMENU::default(),
            HINSTANCE::default(),
            None,
        )
    }
    .context("创建设置窗口失败")?;

    let mut state = SettingsWindow {
        sender,
        mode: HWND::default(),
        hotkey: HWND::default(),
        overlay: HWND::default(),
        start_at_login: HWND::default(),
        status: HWND::default(),
    };
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut state as *mut _ as isize);
    }
    if let Err(error) = create_controls(hwnd, &mut state, &config, status) {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return Err(error);
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = UpdateWindow(hwnd);
    }

    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, HWND::default(), 0, 0) }.as_bool() {
        if unsafe { IsDialogMessageW(hwnd, &message) }.as_bool() {
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

fn create_controls(
    hwnd: HWND,
    state: &mut SettingsWindow,
    config: &Config,
    status: FrontendStatus,
) -> Result<()> {
    let status_text = match status {
        FrontendStatus::Connected => "daemon：已连接",
        FrontendStatus::Disconnected => "daemon：未连接（可点击“启动 daemon”）",
        FrontendStatus::Error => "daemon：连接错误（请点击“诊断”）",
    };
    state.status = control(hwnd, w!("STATIC"), status_text, 0, 20, 16, 400, 20, 0)?;

    state.mode = control(
        hwnd,
        w!("COMBOBOX"),
        "",
        CBS_DROPDOWNLIST | (WS_TABSTOP.0 as i32) | (WS_VSCROLL.0 as i32),
        130,
        52,
        290,
        200,
        ID_MODE,
    )?;
    for item in [
        "PTT（按住说话）",
        "Toggle（按一下切换）",
        "Continuous（VAD 自动分段）",
    ] {
        let item = to_wide(item);
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                state.mode,
                CB_ADDSTRING,
                WPARAM(0),
                LPARAM(item.as_ptr() as isize),
            );
        }
    }
    let mode_index = match config.mode {
        Mode::Ptt => 0,
        Mode::Toggle => 1,
        Mode::Continuous => 2,
    };
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            state.mode,
            CB_SETCURSEL,
            WPARAM(mode_index),
            LPARAM(0),
        );
    }

    label(hwnd, "模式", 20, 54)?;
    label(hwnd, "热键", 20, 94)?;
    state.hotkey = control(
        hwnd,
        w!("EDIT"),
        &config.hotkey.to_string(),
        ES_AUTOHSCROLL | (WS_TABSTOP.0 as i32) | (WS_BORDER.0 as i32),
        130,
        90,
        290,
        26,
        ID_HOTKEY,
    )?;
    label(hwnd, "示例：Ctrl+Alt+Space；留空禁用", 130, 120)?;

    state.overlay = checkbox(
        hwnd,
        "显示临时浮层",
        config.overlay_enabled,
        130,
        150,
        ID_OVERLAY,
    )?;
    state.start_at_login = checkbox(
        hwnd,
        "登录时启动",
        config.start_at_login,
        130,
        178,
        ID_START_AT_LOGIN,
    )?;

    control(
        hwnd,
        w!("BUTTON"),
        "daemon",
        BS_GROUPBOX,
        16,
        208,
        408,
        74,
        0,
    )?;
    button(hwnd, "启动 daemon", 32, 238, 100, ID_START_DAEMON, false)?;
    button(hwnd, "停止 daemon", 142, 238, 100, ID_STOP_DAEMON, false)?;
    button(hwnd, "诊断", 252, 238, 80, ID_DOCTOR, false)?;
    button(hwnd, "保存", 238, 292, 84, ID_SAVE, true)?;
    button(hwnd, "取消", 332, 292, 84, ID_CANCEL, false)?;

    for control in [
        state.mode,
        state.hotkey,
        state.overlay,
        state.start_at_login,
        state.status,
    ] {
        set_font(control);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn control(
    parent: HWND,
    class: PCWSTR,
    text: &str,
    extra_style: i32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: i32,
) -> Result<HWND> {
    let text = to_wide(text);
    let style = WS_CHILD
        | WS_VISIBLE
        | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(extra_style as u32);
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            class,
            PCWSTR(text.as_ptr()),
            style,
            x,
            y,
            width,
            height,
            parent,
            HMENU(id as usize as *mut c_void),
            HINSTANCE::default(),
            None,
        )
    }
    .context("创建设置控件失败")?;
    set_font(hwnd);
    Ok(hwnd)
}

fn label(parent: HWND, text: &str, x: i32, y: i32) -> Result<()> {
    control(parent, w!("STATIC"), text, 0, x, y, 100, 22, 0)?;
    Ok(())
}

fn checkbox(parent: HWND, text: &str, checked: bool, x: i32, y: i32, id: i32) -> Result<HWND> {
    let hwnd = control(
        parent,
        w!("BUTTON"),
        text,
        BS_AUTOCHECKBOX | (WS_TABSTOP.0 as i32),
        x,
        y,
        290,
        24,
        id,
    )?;
    let state = if checked { BST_CHECKED } else { BST_UNCHECKED };
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            hwnd,
            BM_SETCHECK,
            WPARAM(state.0 as usize),
            LPARAM(0),
        );
    }
    Ok(hwnd)
}

fn button(
    parent: HWND,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    id: i32,
    default: bool,
) -> Result<HWND> {
    let style = if default {
        BS_DEFPUSHBUTTON
    } else {
        BS_PUSHBUTTON
    };
    control(
        parent,
        w!("BUTTON"),
        text,
        style | (WS_TABSTOP.0 as i32),
        x,
        y,
        width,
        28,
        id,
    )
}

fn set_font(hwnd: HWND) {
    let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            hwnd,
            WM_SETFONT,
            WPARAM(font.0 as usize),
            LPARAM(1),
        );
    }
}

impl SettingsWindow {
    fn on_command(&self, hwnd: HWND, id: i32) -> Result<()> {
        match id {
            ID_SAVE => {
                let config = self.read_config()?;
                self.sender
                    .send(SettingsAction::Save(config))
                    .context("提交设置失败")?;
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            ID_CANCEL => unsafe {
                let _ = DestroyWindow(hwnd);
            },
            ID_START_DAEMON => {
                let _ = self.sender.send(SettingsAction::StartDaemon);
                set_text(self.status, "daemon：正在启动…");
            }
            ID_STOP_DAEMON => {
                let _ = self.sender.send(SettingsAction::StopDaemon);
                set_text(self.status, "daemon：已请求停止");
            }
            ID_DOCTOR => {
                let _ = self.sender.send(SettingsAction::RunDoctor);
            }
            _ => {}
        }
        Ok(())
    }

    fn read_config(&self) -> Result<Config> {
        let selection = unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.mode,
                CB_GETCURSEL,
                WPARAM(0),
                LPARAM(0),
            )
            .0
        };
        let mode = match selection {
            0 => Mode::Ptt,
            1 => Mode::Toggle,
            2 => Mode::Continuous,
            _ => bail!("请选择模式"),
        };
        let hotkey = hotkey::parse(&get_text(self.hotkey))?;
        let overlay = checked(self.overlay);
        let start_at_login = checked(self.start_at_login);
        Ok(Config {
            mode,
            hotkey,
            start_at_login,
            overlay_enabled: overlay,
        })
    }
}

fn checked(hwnd: HWND) -> bool {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            hwnd,
            BM_GETCHECK,
            WPARAM(0),
            LPARAM(0),
        )
        .0 as u32
            == BST_CHECKED.0
    }
}

fn get_text(hwnd: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    let mut buffer = vec![0u16; (length.max(0) as usize) + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
}

fn set_text(hwnd: HWND, text: &str) {
    let text = to_wide(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as i32;
            let notification = ((wparam.0 >> 16) & 0xffff) as u32;
            if notification == BN_CLICKED {
                let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
                if pointer != 0 {
                    let state = unsafe { &*(pointer as *const SettingsWindow) };
                    if let Err(error) = state.on_command(hwnd, id) {
                        show_error_text(hwnd, &format!("{error:#}"));
                    }
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn register_class() -> Result<()> {
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: HINSTANCE::default(),
        hbrBackground: unsafe { GetSysColorBrush(COLOR_BTNFACE) },
        lpszClassName: w!("VoimeSettings"),
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let error = unsafe { windows::Win32::Foundation::GetLastError() };
        if error != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS {
            return Err(anyhow::anyhow!("注册设置窗口类失败：{error:?}"));
        }
    }
    Ok(())
}

fn show_error_text(owner: HWND, text: &str) {
    let text = to_wide(text);
    unsafe {
        MessageBoxW(
            owner,
            PCWSTR(text.as_ptr()),
            w!("Voime"),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

    #[test]
    fn settings_window_opens_and_closes() {
        let (sender, _receiver) = crossbeam_channel::unbounded();
        let flag = Arc::new(AtomicBool::new(true));
        open(
            Config::default(),
            FrontendStatus::Disconnected,
            sender,
            Arc::clone(&flag),
        )
        .unwrap();
        let mut hwnd = HWND::default();
        for _ in 0..200 {
            if let Ok(found) = unsafe { FindWindowW(w!("VoimeSettings"), PCWSTR::null()) } {
                hwnd = found;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!hwnd.0.is_null());
        unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        for _ in 0..200 {
            if !flag.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!flag.load(Ordering::SeqCst));
    }
}
