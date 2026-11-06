#![cfg_attr(not(test), windows_subsystem = "windows")]

mod commit;
mod config;
mod control;
mod diagnostics;
mod hotkey;
mod icon;
mod ipc;
mod paths;
mod preview;
mod settings;
mod target;
mod tray;

use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use anyhow::Result;
use voime_ipc::{
    ClientMessage, ErrorCode, FrontendState, Mode, Phase, Role, ServerMessage, TriggerSource,
    PROTOCOL_VERSION,
};
use voime_log::vlog;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HWND};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::{CreateMutexW, Sleep, CREATE_NO_WINDOW};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, FindWindowW, PeekMessageW, SetForegroundWindow, TranslateMessage, MSG,
    PM_REMOVE, WM_HOTKEY, WM_QUIT,
};

use crate::commit::commit;
use crate::config::Config;
use crate::hotkey::HotkeyHandle;
use crate::ipc::IpcClient;
use crate::preview::Overlay;
use crate::target::Target;
use crate::tray::{FrontendStatus, Tray};

const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
const DAEMON_RETRY_INTERVAL: Duration = Duration::from_secs(10);

fn main() -> windows::core::Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--settings") {
        let _ = voime_log::init("voime-settings");
        vlog!("设置进程启动");
        let focus_hotkey = args.iter().any(|arg| arg == "--focus-hotkey");
        let status = args
            .iter()
            .find_map(|arg| arg.strip_prefix("--status="))
            .map(status_from_arg)
            .unwrap_or(FrontendStatus::Disconnected);
        let config = match config::load() {
            Ok(config) => config,
            Err(error) => {
                settings::show_error(&error);
                return Ok(());
            }
        };
        if let Err(error) = settings::run_process(config, status, focus_hotkey) {
            settings::show_error(&error);
        }
        return Ok(());
    }
    let _ = voime_log::init("voime-tray");
    if !ensure_single_instance() {
        vlog!("已有托盘实例，激活既有窗口后退出");
        tray::activate_existing();
        return Ok(());
    }
    let config = config::load().map_err(to_windows)?;
    vlog!(
        "托盘启动 mode={:?} hotkey={}",
        config.mode.as_str(),
        config.hotkey
    );
    run(Frontend::new(config)).map_err(to_windows)
}

fn status_from_arg(value: &str) -> FrontendStatus {
    match value {
        "connected" => FrontendStatus::Connected,
        "error" => FrontendStatus::Error,
        _ => FrontendStatus::Disconnected,
    }
}

fn status_arg(status: FrontendStatus) -> &'static str {
    match status {
        FrontendStatus::Connected => "connected",
        FrontendStatus::Disconnected => "disconnected",
        FrontendStatus::Error => "error",
    }
}

fn run(mut frontend: Frontend) -> Result<()> {
    let _com = ComGuard::init();
    let mut tray = Tray::create(&mut frontend)?;
    frontend.attach_window(tray.hwnd());
    if let Err(error) = config::set_start_at_login(frontend.config.start_at_login) {
        vlog!("设置登录启动失败：{error:#}");
    }
    frontend.overlay = Overlay::create().ok();
    if let Some(overlay) = frontend.overlay.as_mut() {
        overlay.set_enabled(frontend.config.overlay_enabled);
    }
    frontend.start_daemon();
    'main: loop {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if message.message == WM_QUIT {
                break 'main;
            }
            if message.message == WM_HOTKEY {
                frontend.on_hotkey();
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        tray.poll_events(&mut frontend);
        frontend.tick();
        tray.update_status(frontend.status);
        unsafe {
            Sleep(10);
        }
    }
    frontend.shutdown();
    vlog!("托盘退出");
    Ok(())
}

fn ensure_single_instance() -> bool {
    let mutex = unsafe { CreateMutexW(None, false, w!("Local\\voime-tray")) };
    match mutex {
        Ok(handle) => {
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                false
            } else {
                let _ = Box::leak(Box::new(handle));
                true
            }
        }
        Err(_) => false,
    }
}

fn to_windows(error: anyhow::Error) -> windows::core::Error {
    windows::core::Error::new(windows::Win32::Foundation::E_FAIL, format!("{error:#}"))
}

struct ComGuard(bool);

impl ComGuard {
    fn init() -> Self {
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self(result.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

pub struct Frontend {
    pub config: Config,
    pub status: FrontendStatus,
    hwnd: Option<HWND>,
    ipc: Option<IpcClient>,
    hotkey: Option<HotkeyHandle>,
    overlay: Option<Overlay>,
    target: Option<Target>,
    pending_target: Option<Target>,
    current_segment: Option<u64>,
    recording: bool,
    finalizing: bool,
    ptt_active: bool,
    hotkey_down: bool,
    hotkey_suspended: bool,
    toggle_active: bool,
    next_connect: Instant,
    last_focus_check: Instant,
    daemon: Option<control::Daemon>,
    daemon_started: Option<Instant>,
    settings_child: Option<Child>,
}

impl Frontend {
    fn new(config: Config) -> Self {
        Self {
            config,
            status: FrontendStatus::Disconnected,
            hwnd: None,
            ipc: None,
            hotkey: None,
            overlay: None,
            target: None,
            pending_target: None,
            current_segment: None,
            recording: false,
            finalizing: false,
            ptt_active: false,
            hotkey_down: false,
            hotkey_suspended: false,
            toggle_active: false,
            next_connect: Instant::now(),
            last_focus_check: Instant::now(),
            daemon: None,
            daemon_started: None,
            settings_child: None,
        }
    }

    fn attach_window(&mut self, hwnd: HWND) {
        self.hwnd = Some(hwnd);
    }

    fn hwnd(&self) -> Option<HWND> {
        self.hwnd
    }

    fn tick(&mut self) {
        self.poll_settings();
        self.poll_daemon();
        let now = Instant::now();
        if self.ipc.is_none() && now >= self.next_connect {
            if let Err(error) = self.try_connect() {
                vlog!("连接 daemon 失败：{error:#}");
                self.next_connect = now + RECONNECT_INTERVAL;
                let start_daemon = self.daemon.is_none()
                    && self
                        .daemon_started
                        .map(|started| now.duration_since(started) >= DAEMON_RETRY_INTERVAL)
                        .unwrap_or(true);
                if start_daemon {
                    self.start_daemon();
                }
            }
        }

        loop {
            let event = self.ipc.as_mut().and_then(|ipc| ipc.try_event());
            match event {
                Some(Ok(message)) => self.on_message(message),
                Some(Err(error)) => {
                    eprintln!("voime-tray: {error:#}");
                    self.on_disconnect();
                    break;
                }
                None => break,
            }
        }

        let hotkey_down = !self.config.hotkey.is_empty() && self.config.hotkey.is_down();
        if self.ptt_active
            && self.config.mode == Mode::Ptt
            && !self.config.hotkey.is_empty()
            && !hotkey_down
        {
            vlog!("PTT 松开，结束语音段");
            self.stop_segment(TriggerSource::Hold);
        }
        self.hotkey_down = hotkey_down;

        if self.target.is_some()
            && now.duration_since(self.last_focus_check) >= Duration::from_millis(50)
        {
            self.last_focus_check = now;
            if let Some(target) = self.target.as_ref() {
                if !target::focused(target) {
                    self.on_target_lost();
                }
            }
        }
    }

    fn poll_daemon(&mut self) {
        let Some(daemon) = self.daemon.as_mut() else {
            return;
        };
        if daemon.has_exited() {
            vlog!("daemon 子进程已退出");
            self.daemon = None;
            self.on_disconnect();
        }
    }

    fn try_connect(&mut self) -> Result<()> {
        let endpoint = voime_transport::default_endpoint();
        let mut client = IpcClient::connect(&endpoint)?;
        client.send(&ClientMessage::Hello {
            role: Role::Frontend,
            version: PROTOCOL_VERSION,
            config: Some(self.config.to_ipc_config()),
        })?;
        let snapshot = client.snapshot()?;
        self.ipc = Some(client);
        self.status = FrontendStatus::Connected;
        self.next_connect = Instant::now();
        vlog!("已连接 daemon");
        if let Err(error) = self.sync_hotkey() {
            vlog!("注册热键失败：{error:#}");
            self.status = FrontendStatus::Error;
        }
        self.on_message(snapshot);
        Ok(())
    }

    fn on_message(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Snapshot {
                phase,
                addon,
                partial,
                ..
            } => {
                self.apply_phase(phase);
                if addon == FrontendState::Disconnected {
                    self.status = FrontendStatus::Disconnected;
                }
                if phase == Phase::Idle && !self.ptt_active && !self.toggle_active {
                    self.target = None;
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay.hide();
                    }
                } else if phase == Phase::Finalizing {
                    // Keep the last partial / target while an old segment is
                    // being finalized.
                } else if let (Some(text), Some(target)) =
                    (partial.as_deref(), self.target.as_ref())
                {
                    if target::focused(target) {
                        if let Some(overlay) = self.overlay.as_mut() {
                            overlay.show(text, target);
                        }
                    }
                }
            }
            ServerMessage::SegmentStarted { segment_id, source } => {
                self.on_segment_started(segment_id, source);
            }
            ServerMessage::State {
                recording,
                finalizing,
                segment_id,
            } => {
                self.recording = recording;
                self.finalizing = finalizing;
                if let Some(segment_id) = segment_id {
                    self.current_segment = Some(segment_id);
                    if let Some(target) = self.target.as_mut() {
                        if target.segment_id == 0 {
                            target.segment_id = segment_id;
                        }
                    }
                }
                if !recording && !finalizing && !self.ptt_active && !self.toggle_active {
                    self.current_segment = None;
                    self.target = None;
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay.hide();
                    }
                }
            }
            ServerMessage::Partial { segment_id, text } => {
                if text.is_empty() || self.finalizing {
                    return;
                }
                if self
                    .target
                    .as_ref()
                    .is_some_and(|target| target.segment_id == segment_id)
                {
                    vlog!("Partial seg={segment_id} chars={}", text.len());
                    if let (Some(overlay), Some(target)) =
                        (self.overlay.as_mut(), self.target.as_ref())
                    {
                        overlay.show(&text, target);
                    }
                }
            }
            ServerMessage::Final { segment_id, text } => {
                let Some(target) = self.target.clone() else {
                    vlog!("Final seg={segment_id} 无目标，忽略");
                    return;
                };
                if target.segment_id != segment_id {
                    vlog!(
                        "Final seg={segment_id} 与目标段 {:?} 不匹配",
                        target.segment_id
                    );
                    if self.config.mode == Mode::Toggle && !self.toggle_active {
                        self.target = None;
                        if let Some(overlay) = self.overlay.as_mut() {
                            overlay.hide();
                        }
                    }
                    return;
                }
                let keep_toggle_preview = self.config.mode == Mode::Toggle && self.toggle_active;
                let keep_ptt_target = self.config.mode == Mode::Ptt && self.ptt_active;
                self.current_segment = None;
                self.recording = false;
                self.finalizing = false;
                if !target::focused(&target) {
                    vlog!("目标已失效，未提交 segment {segment_id}");
                    self.on_target_lost();
                    return;
                }
                if keep_toggle_preview {
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay.show_empty(&target);
                    }
                    self.target = Some(target.clone());
                } else if keep_ptt_target {
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay.hide();
                    }
                    self.target = Some(target.clone());
                } else {
                    self.target = None;
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay.hide();
                    }
                }
                if !text.is_empty() {
                    vlog!("提交文本 seg={segment_id} chars={}", text.len());
                    if let Err(error) = commit(&target, &text) {
                        vlog!("提交失败（segment {segment_id}）：{error:#}");
                    }
                }
            }
            ServerMessage::Error {
                code,
                segment_id: error_segment,
                message,
            } => match code {
                ErrorCode::VersionMismatch
                | ErrorCode::RoleConflict
                | ErrorCode::InvalidMessage
                | ErrorCode::DaemonUnavailable => {
                    vlog!("daemon 拒绝连接（{code:?}）：{message}");
                    self.on_disconnect();
                }
                ErrorCode::SegmentDropped => {
                    vlog!("daemon 丢弃语音段（{error_segment:?}）：{message}");
                    if error_segment.is_none()
                        || error_segment == self.current_segment
                        || error_segment == self.target.as_ref().map(|target| target.segment_id)
                    {
                        self.recording = false;
                        self.finalizing = false;
                        self.current_segment = None;
                        if self.config.mode == Mode::Toggle && self.toggle_active {
                            if let Some(target) = self.target.as_ref() {
                                if target::focused(target) {
                                    if let Some(overlay) = self.overlay.as_mut() {
                                        overlay.show_empty(target);
                                    }
                                }
                            }
                        } else if self.config.mode == Mode::Ptt && self.ptt_active {
                            if let Some(overlay) = self.overlay.as_mut() {
                                overlay.hide();
                            }
                        } else {
                            self.target = None;
                            if let Some(overlay) = self.overlay.as_mut() {
                                overlay.hide();
                            }
                        }
                    }
                }
                ErrorCode::Busy => {
                    if self.pending_target.take().is_some() {
                        self.recording = false;
                        self.ptt_active = false;
                        self.toggle_active = false;
                    }
                }
                _ => vlog!("daemon 错误（{code:?}）：{message}"),
            },
            ServerMessage::Bye {} => self.on_disconnect(),
            ServerMessage::HistoryCleared {}
            | ServerMessage::Status { .. }
            | ServerMessage::Diagnostics { .. } => {}
        }
    }

    fn on_segment_started(&mut self, segment_id: u64, source: TriggerSource) {
        if self.pending_target.is_some() && self.current_segment.is_none() {
            let mut target = self.pending_target.take().expect("pending target checked");
            target.segment_id = segment_id;
            self.target = Some(target);
            self.current_segment = Some(segment_id);
            if let Some(target) = self.target.as_ref() {
                if !target::focused(target) {
                    self.on_target_lost();
                    return;
                }
            }
            if self.config.mode == Mode::Toggle && self.toggle_active {
                if let (Some(overlay), Some(target)) = (self.overlay.as_mut(), self.target.as_ref())
                {
                    overlay.show_empty(target);
                }
            }
            return;
        }
        let ptt_continuation =
            self.config.mode == Mode::Ptt && self.ptt_active && source == TriggerSource::Hold;
        let toggle_continuation = self.config.mode == Mode::Toggle
            && self.toggle_active
            && source == TriggerSource::Toggle;
        match source {
            TriggerSource::Continuous => {
                self.target = target::capture().ok().map(|mut target| {
                    target.segment_id = segment_id;
                    target
                });
            }
            TriggerSource::Hold => {
                if !ptt_continuation {
                    return;
                }
                if self.target.is_none() {
                    self.on_target_lost();
                    return;
                }
            }
            TriggerSource::Toggle => {
                if !toggle_continuation {
                    return;
                }
                if self.target.is_none() {
                    self.on_target_lost();
                    return;
                }
            }
            TriggerSource::Tui | TriggerSource::Cli => return,
        }
        if let Some(target) = self.target.as_mut() {
            target.segment_id = segment_id;
        }
        self.current_segment = Some(segment_id);
        if let Some(target) = self.target.as_ref() {
            if !target::focused(target) {
                self.on_target_lost();
                return;
            }
        }
        if toggle_continuation {
            if let (Some(overlay), Some(target)) = (self.overlay.as_mut(), self.target.as_ref()) {
                overlay.show_empty(target);
            }
        }
    }

    fn apply_phase(&mut self, phase: Phase) {
        self.recording = phase == Phase::Recording;
        self.finalizing = phase == Phase::Finalizing;
    }

    fn on_hotkey(&mut self) {
        if self.ipc.is_none() || self.hotkey_down {
            return;
        }
        self.hotkey_down = true;
        vlog!(
            "热键触发 mode={:?} recording={} toggle_active={}",
            self.config.mode.as_str(),
            self.recording,
            self.toggle_active
        );
        match self.config.mode {
            Mode::Continuous => {}
            Mode::Ptt => {
                if self.recording {
                    return;
                }
                self.start_segment(TriggerSource::Hold);
            }
            Mode::Toggle => {
                if self.toggle_active {
                    self.toggle_active = false;
                    self.stop_segment(TriggerSource::Toggle);
                } else {
                    self.start_segment(TriggerSource::Toggle);
                    self.toggle_active = self.recording;
                }
            }
        }
    }

    fn start_segment(&mut self, source: TriggerSource) {
        let Ok(mut target) = target::capture() else {
            vlog!("没有可用的前台输入目标，放弃启动");
            return;
        };
        target.segment_id = 0;
        if !self.send(&ClientMessage::Start {
            segment_id: 0,
            source,
        }) {
            return;
        }
        let starts_during_finalize = self.finalizing;
        if starts_during_finalize {
            // Keep the old target until its Final has been committed; the new
            // target becomes active on SegmentStarted.
            self.pending_target = Some(target);
        } else {
            self.target = Some(target);
            self.current_segment = None;
        }
        self.recording = true;
        self.finalizing = false;
        self.ptt_active = source == TriggerSource::Hold;
        vlog!("请求开始语音段 source={source:?}");
        if source == TriggerSource::Toggle && !starts_during_finalize {
            if let (Some(overlay), Some(target)) = (self.overlay.as_mut(), self.target.as_ref()) {
                overlay.show_empty(target);
            }
        }
    }

    fn stop_segment(&mut self, source: TriggerSource) {
        let segment_id = self.current_segment.unwrap_or(0);
        let active = self.recording || self.current_segment.is_some();
        vlog!("请求结束语音段 source={source:?} seg={segment_id} active={active}");
        if self.send(&ClientMessage::Stop { segment_id, source }) {
            self.recording = false;
            self.finalizing = active;
            if source == TriggerSource::Hold {
                self.ptt_active = false;
            }
            if !active {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.hide();
                }
            }
            // While finalizing, keep the last partial and target so the
            // Final can still be committed.
        }
    }

    fn send(&mut self, message: &ClientMessage) -> bool {
        if let Some(ipc) = self.ipc.as_mut() {
            if ipc.send(message).is_ok() {
                return true;
            }
        }
        self.on_disconnect();
        false
    }

    fn on_target_lost(&mut self) {
        vlog!(
            "目标失效 mode={:?} toggle_active={} recording={}",
            self.config.mode.as_str(),
            self.toggle_active,
            self.recording
        );
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.hide();
        }
        if self.pending_target.is_some() {
            // The lost target belongs to the old segment; keep the pending
            // target for the session that will start after its Final.
            self.target = None;
            self.current_segment = None;
            return;
        }
        let ptt = self.config.mode == Mode::Ptt && (self.ptt_active || self.recording);
        let toggle = self.config.mode == Mode::Toggle && (self.toggle_active || self.recording);
        if ptt || toggle {
            let active = self.recording || self.current_segment.is_some();
            let source = if ptt {
                self.ptt_active = false;
                TriggerSource::Hold
            } else {
                self.toggle_active = false;
                TriggerSource::Toggle
            };
            let segment_id = self.current_segment.unwrap_or(0);
            if let Some(ipc) = self.ipc.as_mut() {
                let _ = ipc.send(&ClientMessage::Stop { segment_id, source });
            }
            self.recording = false;
            self.finalizing = active;
            self.current_segment = None;
        }
        self.target = None;
    }

    fn on_disconnect(&mut self) {
        vlog!("与 daemon 断开");
        self.ipc.take();
        if let Some(handle) = self.hotkey.take() {
            hotkey::unregister(handle);
        }
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.hide();
        }
        self.target = None;
        self.pending_target = None;
        self.current_segment = None;
        self.recording = false;
        self.finalizing = false;
        self.ptt_active = false;
        self.hotkey_down = false;
        self.toggle_active = false;
        self.status = FrontendStatus::Disconnected;
        self.next_connect = Instant::now() + RECONNECT_INTERVAL;
    }

    fn leave_toggle_preview(&mut self) {
        if self.toggle_active && self.config.mode != Mode::Toggle {
            self.toggle_active = false;
            if !self.recording && !self.finalizing {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.hide();
                }
                self.target = None;
            }
        }
    }

    fn clear_mode_state(&mut self) {
        self.ptt_active = false;
        self.toggle_active = false;
        self.hotkey_down = false;
        if !self.recording && !self.finalizing {
            self.current_segment = None;
            self.target = None;
            if let Some(overlay) = self.overlay.as_mut() {
                overlay.hide();
            }
        }
    }

    fn sync_hotkey(&mut self) -> Result<()> {
        if let Some(handle) = self.hotkey.take() {
            hotkey::unregister(handle);
        }
        self.hotkey_down = false;
        if self.ipc.is_some()
            && !self.hotkey_suspended
            && self.config.mode != Mode::Continuous
            && !self.config.hotkey.is_empty()
        {
            let hotkey = self.config.hotkey;
            self.hotkey = Some(hotkey::register(&hotkey, self)?);
        }
        if self.ipc.is_some() {
            self.status = FrontendStatus::Connected;
        }
        Ok(())
    }

    pub fn set_mode(&mut self, mode: Mode) -> Result<()> {
        let previous = self.config.mode;
        self.config.mode = mode;
        if let Err(error) = self.sync_hotkey() {
            self.config.mode = previous;
            let _ = self.sync_hotkey();
            return Err(error);
        }
        if previous != mode {
            self.clear_mode_state();
        }
        self.leave_toggle_preview();
        self.persist()?;
        self.configure()?;
        Ok(())
    }

    pub fn set_overlay_enabled(&mut self, enabled: bool) -> Result<()> {
        self.config.overlay_enabled = enabled;
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.set_enabled(enabled);
        }
        self.persist()
    }

    pub fn set_start_at_login(&mut self, enabled: bool) -> Result<()> {
        config::set_start_at_login(enabled)?;
        self.config.start_at_login = enabled;
        self.persist()
    }

    pub fn open_settings(&mut self, focus_hotkey: bool) {
        self.poll_settings();
        if self.settings_running() {
            vlog!("设置进程已在运行，激活其窗口");
            if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR::null(), w!("Voime 设置")) } {
                unsafe {
                    let _ = SetForegroundWindow(hwnd);
                }
            }
            return;
        }
        if let Some(handle) = self.hotkey.take() {
            hotkey::unregister(handle);
        }
        self.hotkey_down = false;
        self.hotkey_suspended = true;
        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                vlog!("读取设置进程路径失败：{error:#}");
                self.hotkey_suspended = false;
                let _ = self.sync_hotkey();
                settings::show_error(&error.into());
                return;
            }
        };
        let mut command = Command::new(executable);
        command
            .arg("--settings")
            .arg(format!("--status={}", status_arg(self.status)))
            .creation_flags(CREATE_NO_WINDOW.0);
        if focus_hotkey {
            command.arg("--focus-hotkey");
        }
        match command.spawn() {
            Ok(child) => {
                vlog!("设置进程已启动");
                self.settings_child = Some(child);
            }
            Err(error) => {
                vlog!("启动设置进程失败：{error:#}");
                self.hotkey_suspended = false;
                let _ = self.sync_hotkey();
                settings::show_error(&error.into());
            }
        }
    }

    fn settings_running(&mut self) -> bool {
        let Some(child) = self.settings_child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) => {
                self.settings_child = None;
                false
            }
            Err(error) => {
                vlog!("检查设置进程失败：{error}");
                true
            }
        }
    }

    fn poll_settings(&mut self) {
        if self.settings_child.is_none() {
            return;
        }
        if self.settings_running() {
            return;
        }
        vlog!("设置进程退出，重载配置");
        self.hotkey_suspended = false;
        match config::load() {
            Ok(config) => {
                if let Err(error) = self.apply_settings(config) {
                    settings::show_error(&error);
                }
            }
            Err(error) => {
                settings::show_error(&error);
                let _ = self.sync_hotkey();
            }
        }
    }

    fn apply_settings(&mut self, config: Config) -> Result<()> {
        let previous = self.config.clone();
        self.config.mode = config.mode;
        self.config.hotkey = config.hotkey;
        if let Err(error) = self.sync_hotkey() {
            self.config.mode = previous.mode;
            self.config.hotkey = previous.hotkey;
            let _ = self.sync_hotkey();
            return Err(error);
        }
        if previous.mode != config.mode {
            self.clear_mode_state();
        }
        self.leave_toggle_preview();
        self.config.overlay_enabled = config.overlay_enabled;
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.set_enabled(config.overlay_enabled);
        }
        if config.start_at_login != previous.start_at_login {
            if let Err(error) = config::set_start_at_login(config.start_at_login) {
                self.config.start_at_login = previous.start_at_login;
                return Err(error);
            }
        }
        self.config.start_at_login = config.start_at_login;
        self.persist()?;
        self.configure()?;
        Ok(())
    }

    pub fn run_doctor(&mut self) {
        settings::show_text("Voime 诊断", &diagnostics::doctor_text());
    }

    pub fn start_daemon(&mut self) {
        if self.daemon.is_some() || self.ipc.is_some() {
            return;
        }
        self.daemon_started = Some(Instant::now());
        vlog!("启动 daemon 子进程");
        control::stop_existing();
        match control::Daemon::start() {
            Ok(daemon) => self.daemon = Some(daemon),
            Err(error) => vlog!("启动 daemon 失败：{error:#}"),
        }
    }

    pub fn shutdown(&mut self) {
        vlog!("托盘关闭流程开始");
        self.on_disconnect();
        if let Some(mut daemon) = self.daemon.take() {
            daemon.stop();
        } else {
            let _ = control::quit_running();
        }
    }

    fn persist(&self) -> Result<()> {
        self.config.save()
    }

    fn configure(&mut self) -> Result<()> {
        let message = ClientMessage::Configure {
            mode: self.config.mode,
            hotkey: self.config.hotkey.to_string(),
        };
        if self.ipc.is_some() && !self.send(&message) {
            anyhow::bail!("daemon 连接已断开");
        }
        Ok(())
    }
}
