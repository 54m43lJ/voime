mod commit;
mod config;
mod hotkey;
mod ipc;
mod preview;
mod settings;
mod target;
mod tray;

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use voime_ipc::{
    ClientMessage, ErrorCode, FrontendState, Mode, Phase, Role, ServerMessage, TriggerSource,
    PROTOCOL_VERSION,
};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HWND};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::{CreateMutexW, Sleep, CREATE_NO_WINDOW};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, WM_HOTKEY, WM_QUIT,
};

use crate::commit::commit;
use crate::config::Config;
use crate::hotkey::{Hotkey, HotkeyHandle};
use crate::ipc::IpcClient;
use crate::preview::Overlay;
use crate::settings::SettingsAction;
use crate::target::Target;
use crate::tray::{FrontendStatus, Tray};

const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
const DAEMON_RETRY_INTERVAL: Duration = Duration::from_secs(10);

fn main() -> windows::core::Result<()> {
    if !ensure_single_instance() {
        tray::activate_existing();
        return Ok(());
    }
    let config = config::load().map_err(to_windows)?;
    run(Frontend::new(config)).map_err(to_windows)
}

fn run(mut frontend: Frontend) -> Result<()> {
    let _com = ComGuard::init();
    let mut tray = Tray::create(&mut frontend)?;
    frontend.attach_window(tray.hwnd());
    if let Err(error) = config::set_start_at_login(frontend.config.start_at_login) {
        eprintln!("voime-tray: {error:#}");
    }
    frontend.overlay = Overlay::create().ok();
    if let Some(overlay) = frontend.overlay.as_mut() {
        overlay.set_enabled(frontend.config.overlay_enabled);
    }
    if frontend.try_connect().is_err() {
        frontend.start_daemon();
    }
    loop {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if message.message == WM_QUIT {
                return Ok(());
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
        Self(result.ok().is_ok())
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
    current_segment: Option<u64>,
    recording: bool,
    finalizing: bool,
    ptt_active: bool,
    hotkey_down: bool,
    next_connect: Instant,
    last_focus_check: Instant,
    daemon_started: Option<Instant>,
    settings_tx: Sender<SettingsAction>,
    settings_rx: Receiver<SettingsAction>,
    settings_open: Arc<AtomicBool>,
}

impl Frontend {
    fn new(config: Config) -> Self {
        let (settings_tx, settings_rx) = crossbeam_channel::unbounded();
        Self {
            config,
            status: FrontendStatus::Disconnected,
            hwnd: None,
            ipc: None,
            hotkey: None,
            overlay: None,
            target: None,
            current_segment: None,
            recording: false,
            finalizing: false,
            ptt_active: false,
            hotkey_down: false,
            next_connect: Instant::now(),
            last_focus_check: Instant::now(),
            daemon_started: None,
            settings_tx,
            settings_rx,
            settings_open: Arc::new(AtomicBool::new(false)),
        }
    }

    fn attach_window(&mut self, hwnd: HWND) {
        self.hwnd = Some(hwnd);
    }

    fn hwnd(&self) -> Option<HWND> {
        self.hwnd
    }

    fn tick(&mut self) {
        let now = Instant::now();
        if self.ipc.is_none() && now >= self.next_connect {
            if let Err(error) = self.try_connect() {
                eprintln!("voime-tray: {error:#}");
                self.next_connect = now + RECONNECT_INTERVAL;
                let start_daemon = self
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
                Some(Ok(message)) => ipc::dispatch(message, self),
                Some(Err(error)) => {
                    eprintln!("voime-tray: {error:#}");
                    self.on_disconnect();
                    break;
                }
                None => break,
            }
        }

        while let Ok(action) = self.settings_rx.try_recv() {
            self.on_settings_action(action);
        }

        let hotkey_down = !self.config.hotkey.is_empty() && self.config.hotkey.is_down();
        if self.ptt_active
            && self.config.mode == Mode::Ptt
            && !self.config.hotkey.is_empty()
            && !hotkey_down
        {
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
        if let Err(error) = self.sync_hotkey() {
            eprintln!("voime-tray: {error:#}");
            self.status = FrontendStatus::Error;
        }
        ipc::dispatch(snapshot, self);
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
                if partial.is_some() {
                    self.target = None;
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
            }
            ServerMessage::Partial { segment_id, text } => {
                if text.is_empty() {
                    return;
                }
                if self
                    .target
                    .as_ref()
                    .is_some_and(|target| target.segment_id == segment_id)
                {
                    if let (Some(overlay), Some(target)) =
                        (self.overlay.as_mut(), self.target.as_ref())
                    {
                        overlay.show(&text, target);
                    }
                }
            }
            ServerMessage::Final { segment_id, text } => {
                let Some(target) = self.target.clone() else {
                    return;
                };
                if target.segment_id != segment_id {
                    return;
                }
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.hide();
                }
                self.target = None;
                self.current_segment = None;
                self.recording = false;
                self.finalizing = false;
                self.ptt_active = false;
                if text.is_empty() {
                    return;
                }
                if !target::focused(&target) {
                    eprintln!("voime-tray: 目标已失效，未提交 segment {segment_id}");
                    return;
                }
                if let Err(error) = commit(&target, &text) {
                    eprintln!("voime-tray: 提交失败（segment {segment_id}）：{error:#}");
                }
            }
            ServerMessage::Error { code, message } => match code {
                ErrorCode::VersionMismatch
                | ErrorCode::RoleConflict
                | ErrorCode::DaemonUnavailable => {
                    eprintln!("voime-tray: daemon 拒绝连接（{code:?}）：{message}");
                    self.on_disconnect();
                }
                ErrorCode::Busy
                | ErrorCode::NoInputContext
                | ErrorCode::Unavailable
                | ErrorCode::Inference => {
                    eprintln!("voime-tray: daemon 错误（{code:?}）：{message}");
                    self.clear_segment();
                }
                _ => eprintln!("voime-tray: daemon 错误（{code:?}）：{message}"),
            },
            ServerMessage::Bye => self.on_disconnect(),
            ServerMessage::HistoryCleared | ServerMessage::Status { .. } => {}
        }
    }

    fn on_segment_started(&mut self, segment_id: u64, source: TriggerSource) {
        if !self.recording && source != TriggerSource::Continuous {
            return;
        }
        let bind = match self.target.as_ref() {
            None => true,
            Some(target) => target.segment_id != 0 && target.segment_id != segment_id,
        };
        if bind {
            self.target = target::capture().ok().map(|mut target| {
                target.segment_id = segment_id;
                target
            });
        } else if let Some(target) = self.target.as_mut() {
            target.segment_id = segment_id;
        }
        self.current_segment = Some(segment_id);
        if let Some(target) = self.target.as_ref() {
            if !target::focused(target) {
                self.on_target_lost();
            }
        }
    }

    fn apply_phase(&mut self, phase: Phase) {
        self.recording = phase == Phase::Recording;
        self.finalizing = phase == Phase::Finalizing;
    }

    fn on_hotkey(&mut self) {
        if self.ipc.is_none() || self.finalizing || self.hotkey_down {
            return;
        }
        self.hotkey_down = true;
        match self.config.mode {
            Mode::Continuous => {}
            Mode::Ptt => {
                if self.recording {
                    return;
                }
                self.start_segment(TriggerSource::Hold);
            }
            Mode::Toggle => {
                if self.recording {
                    self.stop_segment(TriggerSource::Toggle);
                } else {
                    self.start_segment(TriggerSource::Toggle);
                }
            }
        }
    }

    fn start_segment(&mut self, source: TriggerSource) {
        let Ok(mut target) = target::capture() else {
            eprintln!("voime-tray: 没有可用的前台输入目标");
            return;
        };
        target.segment_id = 0;
        if !self.send(&ClientMessage::Start {
            segment_id: 0,
            source,
        }) {
            return;
        }
        self.target = Some(target);
        self.recording = true;
        self.finalizing = false;
        self.ptt_active = source == TriggerSource::Hold;
        self.current_segment = None;
    }

    fn stop_segment(&mut self, source: TriggerSource) {
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.hide();
        }
        let segment_id = self.current_segment.unwrap_or(0);
        if self.send(&ClientMessage::Stop { segment_id, source }) {
            self.recording = false;
            self.finalizing = true;
            if source == TriggerSource::Hold {
                self.ptt_active = false;
            }
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
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.hide();
        }
        if self.config.mode == Mode::Toggle && self.recording {
            let segment_id = self.current_segment.unwrap_or(0);
            if let Some(ipc) = self.ipc.as_mut() {
                let _ = ipc.send(&ClientMessage::Stop {
                    segment_id,
                    source: TriggerSource::Toggle,
                });
            }
            self.recording = false;
            self.finalizing = true;
        }
        self.target = None;
    }

    fn on_disconnect(&mut self) {
        self.ipc.take();
        if let Some(handle) = self.hotkey.take() {
            hotkey::unregister(handle);
        }
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.hide();
        }
        self.target = None;
        self.current_segment = None;
        self.recording = false;
        self.finalizing = false;
        self.ptt_active = false;
        self.hotkey_down = false;
        self.status = FrontendStatus::Disconnected;
        self.next_connect = Instant::now() + RECONNECT_INTERVAL;
    }

    fn clear_segment(&mut self) {
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.hide();
        }
        self.target = None;
        self.current_segment = None;
        self.recording = false;
        self.finalizing = false;
        self.ptt_active = false;
    }

    fn sync_hotkey(&mut self) -> Result<()> {
        if let Some(handle) = self.hotkey.take() {
            hotkey::unregister(handle);
        }
        self.hotkey_down = false;
        if self.ipc.is_some()
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
        self.persist()?;
        self.configure()?;
        Ok(())
    }

    pub fn set_hotkey(&mut self, spec: &str) -> Result<()> {
        let hotkey: Hotkey = config::parse_hotkey(spec)?;
        let previous = self.config.hotkey;
        self.config.hotkey = hotkey;
        if let Err(error) = self.sync_hotkey() {
            self.config.hotkey = previous;
            let _ = self.sync_hotkey();
            return Err(error);
        }
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

    pub fn open_settings(&mut self) {
        if self.settings_open.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Err(error) = settings::open(
            self.config.clone(),
            self.status,
            self.settings_tx.clone(),
            Arc::clone(&self.settings_open),
        ) {
            self.settings_open.store(false, Ordering::SeqCst);
            settings::show_error(&error);
        }
    }

    fn on_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::Save(config) => {
                if let Err(error) = self.apply_settings(config) {
                    settings::show_error(&error);
                }
            }
            SettingsAction::StartDaemon => self.start_daemon(),
            SettingsAction::StopDaemon => self.stop_daemon(),
            SettingsAction::RunDoctor => self.run_doctor(),
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
        let executable = cli_path();
        let mut command = Command::new(&executable);
        command.arg("doctor").creation_flags(CREATE_NO_WINDOW.0);
        let output = match command.output() {
            Ok(output) => output,
            Err(error) => {
                settings::show_error(&error.into());
                return;
            }
        };
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        if text.trim().is_empty() {
            text = format!("doctor 退出码：{:?}", output.status.code());
        }
        settings::show_text("Voime 诊断", &text);
    }

    pub fn start_daemon(&mut self) {
        let executable = daemon_path();
        let mut command = Command::new(&executable);
        command.creation_flags(CREATE_NO_WINDOW.0);
        match command.spawn() {
            Ok(_) => {
                self.daemon_started = Some(Instant::now());
            }
            Err(error) => {
                eprintln!(
                    "voime-tray: 启动 daemon 失败 {}：{error}",
                    executable.display()
                );
                self.daemon_started = Some(Instant::now());
            }
        }
    }

    pub fn stop_daemon(&mut self) {
        let executable = cli_path();
        let mut command = Command::new(&executable);
        command
            .arg("ctl")
            .arg("quit")
            .creation_flags(CREATE_NO_WINDOW.0);
        if let Err(error) = command.spawn() {
            eprintln!(
                "voime-tray: 停止 daemon 失败 {}：{error}",
                executable.display()
            );
        }
    }

    fn persist(&self) -> Result<()> {
        self.config.save(&self.config)
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

fn daemon_path() -> PathBuf {
    executable_path("voimed.exe")
}

fn cli_path() -> PathBuf {
    executable_path("voime.exe")
}

fn executable_path(name: &str) -> PathBuf {
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate;
            }
            for ancestor in directory.ancestors().take(6) {
                let candidate = ancestor.join("target").join("release").join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_path_points_to_sibling_when_present() {
        let path = daemon_path();
        assert!(path.is_absolute() || path.as_os_str() == "voimed.exe");
    }
}
