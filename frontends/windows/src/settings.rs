use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use eframe::egui;
use voime_ipc::Mode;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, VK_BACK, VK_BROWSER_BACK, VK_DELETE, VK_DOWN, VK_END,
    VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT, VK_LEFT, VK_NEXT, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4,
    VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR,
    VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK,
};

use crate::config::Config;
use crate::diagnostics;
use crate::hotkey::{self, Hotkey};
use crate::tray::FrontendStatus;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(82, 125, 255);
const SUCCESS: egui::Color32 = egui::Color32::from_rgb(74, 188, 126);
const WARNING: egui::Color32 = egui::Color32::from_rgb(235, 170, 70);
const DANGER: egui::Color32 = egui::Color32::from_rgb(236, 104, 104);

pub enum SettingsAction {
    Save {
        config: Config,
        ack: Sender<Result<(), String>>,
    },
}

pub fn run_process(config: Config, status: FrontendStatus, focus_hotkey: bool) -> Result<()> {
    let (sender, receiver) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("voime-settings-actions".to_string())
        .spawn(move || {
            while let Ok(action) = receiver.recv() {
                match action {
                    SettingsAction::Save { config, ack } => {
                        let result = hotkey::check_available(&config.hotkey)
                            .and_then(|_| config.save())
                            .map_err(|error| format!("{error:#}"));
                        let _ = ack.send(result);
                    }
                }
            }
        })
        .context("创建设置动作线程失败")?;
    run_window(config, status, sender, focus_hotkey)
}

pub fn show_error(error: &anyhow::Error) {
    show_error_text(&format!("{error:#}"));
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

struct Notice {
    text: String,
    error: bool,
}

impl Notice {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
        }
    }
}

enum DoctorState {
    Running(Receiver<String>),
    Text(String),
}

struct SettingsApp {
    sender: Sender<SettingsAction>,
    status: FrontendStatus,
    mode: Mode,
    hotkey: Hotkey,
    overlay_enabled: bool,
    start_at_login: bool,
    capturing: bool,
    capture_modifiers: u32,
    want_capture: bool,
    notice: Option<Notice>,
    save_wait: Option<Receiver<Result<(), String>>>,
    saving: bool,
    daemon_hint: Option<String>,
    doctor: Option<DoctorState>,
}

impl SettingsApp {
    fn new(
        config: Config,
        status: FrontendStatus,
        sender: Sender<SettingsAction>,
        focus_hotkey: bool,
    ) -> Self {
        Self {
            sender,
            status,
            mode: config.mode,
            hotkey: config.hotkey,
            overlay_enabled: config.overlay_enabled,
            start_at_login: config.start_at_login,
            capturing: false,
            capture_modifiers: 0,
            want_capture: focus_hotkey,
            notice: None,
            save_wait: None,
            saving: false,
            daemon_hint: None,
            doctor: None,
        }
    }

    fn start_capture(&mut self) {
        if self.mode == Mode::Continuous || self.capturing {
            return;
        }
        self.capturing = true;
        self.capture_modifiers = hotkey::modifier_state();
        self.notice = None;
    }

    fn stop_capture(&mut self) {
        self.capturing = false;
    }

    fn capture_from_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|input| input.events.clone());
        let mut handled = false;
        for event in &events {
            let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } = event
            else {
                continue;
            };
            handled = true;
            if *key == egui::Key::Escape {
                self.stop_capture();
                self.notice = None;
                break;
            }
            let mut modifiers_mask = 0;
            if modifiers.ctrl {
                modifiers_mask |= MOD_CONTROL.0;
            }
            if modifiers.alt {
                modifiers_mask |= MOD_ALT.0;
            }
            if modifiers.shift {
                modifiers_mask |= MOD_SHIFT.0;
            }
            if hotkey::win_down() {
                modifiers_mask |= MOD_WIN.0;
            }
            if modifiers_mask == 0 {
                self.notice = Some(Notice::error(
                    "请至少按住一个修饰键：Ctrl / Alt / Shift / Win",
                ));
                continue;
            }
            let Some(key) = egui_key_to_vk(*key) else {
                self.notice = Some(Notice::error("这个按键不能用作全局热键"));
                continue;
            };
            self.hotkey = Hotkey::new(modifiers_mask, key);
            let label = self.hotkey.to_string();
            self.stop_capture();
            self.notice = Some(Notice::info(format!("已录制热键：{label}")));
            break;
        }
        if handled {
            ctx.input_mut(|input| {
                input.events.retain(|event| {
                    !matches!(event, egui::Event::Key { .. } | egui::Event::Text(_))
                })
            });
        }
    }
    fn save(&mut self, ctx: &egui::Context) {
        if self.saving {
            return;
        }
        let config = Config {
            mode: self.mode,
            hotkey: self.hotkey,
            start_at_login: self.start_at_login,
            overlay_enabled: self.overlay_enabled,
        };
        let (ack, wait) = crossbeam_channel::bounded(1);
        if self
            .sender
            .send(SettingsAction::Save { config, ack })
            .is_err()
        {
            self.notice = Some(Notice::error("设置通道已关闭"));
            return;
        }
        self.save_wait = Some(wait);
        self.saving = true;
        self.notice = None;
        ctx.request_repaint();
    }

    fn poll_save(&mut self, ctx: &egui::Context) {
        let result = self.save_wait.as_ref().map(|wait| wait.try_recv());
        match result {
            Some(Ok(Ok(()))) => {
                self.save_wait = None;
                self.saving = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(Ok(Err(message))) => {
                self.save_wait = None;
                self.saving = false;
                self.notice = Some(Notice::error(message));
            }
            Some(Err(TryRecvError::Empty)) => {}
            Some(Err(TryRecvError::Disconnected)) => {
                self.save_wait = None;
                self.saving = false;
                self.notice = Some(Notice::error("保存请求没有响应"));
            }
            None => {}
        }
    }

    fn start_doctor(&mut self) {
        let (sender, receiver) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send(diagnostics::doctor_text());
        });
        self.doctor = Some(DoctorState::Running(receiver));
    }

    fn poll_doctor(&mut self) {
        let result = match self.doctor.as_ref() {
            Some(DoctorState::Running(receiver)) => match receiver.try_recv() {
                Ok(text) => Some(text),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some("诊断进程已退出但没有输出".to_string()),
            },
            _ => None,
        };
        if let Some(text) = result {
            self.doctor = Some(DoctorState::Text(text));
        }
    }

    fn show_doctor(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut close = false;
        if let Some(state) = self.doctor.as_ref() {
            egui::Window::new("Voime 诊断")
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_width(560.0)
                .show(ctx, |ui| {
                    match state {
                        DoctorState::Running(_) => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("正在运行 doctor…");
                            });
                        }
                        DoctorState::Text(text) => {
                            egui::ScrollArea::vertical()
                                .max_height(360.0)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(text).monospace().size(12.5),
                                        )
                                        .wrap(),
                                    );
                                });
                        }
                    }
                    ui.add_space(8.0);
                    if ui.button("关闭").clicked() {
                        close = true;
                    }
                });
        }
        if !open || close {
            self.doctor = None;
        }
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let focused = ctx.input(|input| input.focused);
        self.poll_save(ctx);
        self.poll_doctor();
        if self.capturing {
            self.capture_from_input(ctx);
        }
        if self.capturing && !focused {
            self.stop_capture();
        }
        if self.want_capture && ctx.input(|input| input.focused) {
            self.want_capture = false;
            self.start_capture();
        }
        if self.capturing {
            self.capture_modifiers = hotkey::modifier_state();
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.saving || matches!(self.doctor.as_ref(), Some(DoctorState::Running(_))) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(egui::Margin::same(22)),
            )
            .show(ctx, |ui| {
                let footer_height = 54.0;
                let content_height = (ui.available_height() - footer_height).max(0.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), content_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                self.header(ui);
                                ui.add_space(10.0);
                                self.mode_section(ui);
                                ui.add_space(10.0);
                                self.hotkey_section(ui);
                                if let Some(notice) = self.notice.as_ref() {
                                    ui.add_space(8.0);
                                    self.notice(ui, notice);
                                }
                                ui.add_space(10.0);
                                self.options_section(ui);
                                ui.add_space(10.0);
                                self.daemon_section(ui, ctx);
                                ui.add_space(8.0);
                            });
                    },
                );
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                self.footer(ui, ctx);
            });
        self.show_doctor(ctx);
    }
}

impl SettingsApp {
    fn header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Voime")
                        .size(26.0)
                        .strong()
                        .color(ui.visuals().text_color()),
                );
                ui.label(
                    egui::RichText::new("语音听写设置")
                        .size(13.5)
                        .color(ui.visuals().weak_text_color()),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                        .size(12.0)
                        .color(ui.visuals().weak_text_color()),
                );
            });
        });
    }

    fn mode_section(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "输入模式");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (mode, label) in [
                (Mode::Ptt, "按住说话"),
                (Mode::Toggle, "按一下切换"),
                (Mode::Continuous, "自动分段"),
            ] {
                let selected = self.mode == mode;
                let text = egui::RichText::new(label).size(14.0).color(if selected {
                    egui::Color32::WHITE
                } else {
                    ui.visuals().text_color()
                });
                let button = egui::Button::new(text)
                    .fill(if selected {
                        ACCENT
                    } else {
                        ui.visuals().faint_bg_color
                    })
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        if selected {
                            ACCENT
                        } else {
                            ui.visuals().widgets.noninteractive.bg_stroke.color
                        },
                    ))
                    .corner_radius(8.0)
                    .min_size(egui::vec2(126.0, 34.0));
                let response = ui.add(button);
                if response.clicked() {
                    if self.mode != mode {
                        self.stop_capture();
                    }
                    self.mode = mode;
                }
            }
        });
        ui.add_space(6.0);
        let description = match self.mode {
            Mode::Ptt => "按住热键说话，松开后识别并提交；适合短句和快速输入。",
            Mode::Toggle => "按一下开始，再按一下结束；适合较长的一段话。",
            Mode::Continuous => "由 VAD 自动检测语音起止，不需要热键。",
        };
        ui.label(
            egui::RichText::new(description)
                .size(12.5)
                .color(ui.visuals().weak_text_color()),
        );
    }

    fn hotkey_section(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "全局热键");
        ui.add_space(6.0);
        let enabled = self.mode != Mode::Continuous;
        let capturing = self.capturing;
        let label = if capturing {
            "请按下组合键…".to_string()
        } else if self.hotkey.is_empty() {
            "未设置（点击录制）".to_string()
        } else {
            self.hotkey.to_string()
        };
        let text = egui::RichText::new(label)
            .size(19.0)
            .strong()
            .color(if capturing {
                ACCENT
            } else if enabled {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            });
        let button = egui::Button::new(text)
            .fill(if capturing {
                egui::Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 36)
            } else {
                ui.visuals().extreme_bg_color
            })
            .stroke(egui::Stroke::new(
                1.0_f32,
                if capturing {
                    ACCENT
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke.color
                },
            ))
            .corner_radius(10.0);
        let width = ui.available_width();
        let response = ui
            .add_enabled_ui(enabled, |ui| ui.add_sized([width, 48.0], button))
            .inner;
        if response.clicked() {
            if capturing {
                self.stop_capture();
            } else {
                self.start_capture();
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.hotkey.is_empty(), egui::Button::new("清除热键"))
                .clicked()
            {
                self.stop_capture();
                self.hotkey = Hotkey::new(0, 0);
                self.notice = None;
            }
            if ui
                .add_enabled(!self.hotkey.is_empty(), egui::Button::new("恢复默认"))
                .clicked()
            {
                self.stop_capture();
                self.hotkey = Hotkey::default();
                self.notice = None;
            }
            if capturing {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("Esc 取消录制")
                            .size(12.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                });
            }
        });
        ui.add_space(4.0);
        let hint = if self.mode == Mode::Continuous {
            "自动分段模式不使用全局热键。".to_string()
        } else if capturing {
            let modifiers = hotkey::modifier_names(self.capture_modifiers);
            if modifiers.is_empty() {
                "已进入录制，请按住修饰键，再按主键；按 Esc 取消。".to_string()
            } else {
                format!("已按住 {modifiers}，请按主键；按 Esc 取消。")
            }
        } else {
            "点击上面的按钮后直接按键。支持 Ctrl / Alt / Shift / Win 与字母、数字、F1–F24、方向键、导航键、标点键。".to_string()
        };
        ui.label(
            egui::RichText::new(hint)
                .size(12.0)
                .color(ui.visuals().weak_text_color()),
        );
    }

    fn options_section(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "其他");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.overlay_enabled, "显示临时浮层");
            ui.checkbox(&mut self.start_at_login, "登录时启动");
        });
    }

    fn daemon_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section_title(ui, "后台服务");
        ui.add_space(6.0);
        let (label, color) = match self.status {
            FrontendStatus::Connected => ("已连接", SUCCESS),
            FrontendStatus::Disconnected => ("未连接", WARNING),
            FrontendStatus::Error => ("连接错误", DANGER),
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("●").size(16.0).color(color));
            ui.label(label);
            if let Some(hint) = self.daemon_hint.as_deref() {
                ui.label(
                    egui::RichText::new(hint)
                        .size(12.5)
                        .color(ui.visuals().weak_text_color()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("诊断…").clicked() {
                    self.start_doctor();
                }
                if ui.button("停止服务并退出").clicked() {
                    if crate::tray::send_command(crate::tray::TrayCommand::StopDaemon) {
                        self.daemon_hint = Some("正在停止服务并退出".to_string());
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else {
                        self.daemon_hint = Some("托盘未运行，无法停止服务".to_string());
                    }
                }
                if ui.button("启动服务").clicked() {
                    if crate::tray::send_command(crate::tray::TrayCommand::StartDaemon) {
                        self.daemon_hint = Some("正在启动…".to_string());
                    } else {
                        self.daemon_hint = Some("托盘未运行，请先启动托盘".to_string());
                    }
                }
            });
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("后台服务随托盘启动和退出，不再单独驻留。")
                .size(12.0)
                .color(ui.visuals().weak_text_color()),
        );
    }

    fn notice(&self, ui: &mut egui::Ui, notice: &Notice) {
        let color = if notice.error { DANGER } else { SUCCESS };
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(
                color.r(),
                color.g(),
                color.b(),
                28,
            ))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(&notice.text).color(color));
            });
    }

    fn footer(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let save = egui::Button::new(
                egui::RichText::new(if self.saving {
                    "保存中…"
                } else {
                    "保存"
                })
                .color(egui::Color32::WHITE),
            )
            .fill(ACCENT)
            .corner_radius(8.0)
            .min_size(egui::vec2(96.0, 34.0));
            if ui.add_enabled(!self.saving, save).clicked() {
                self.save(ctx);
            }
            let cancel = egui::Button::new("取消")
                .corner_radius(8.0)
                .min_size(egui::vec2(96.0, 34.0));
            if ui.add_enabled(!self.saving, cancel).clicked() {
                self.stop_capture();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

fn egui_key_to_vk(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    Some(match key {
        ArrowDown => VK_DOWN.0 as u32,
        ArrowLeft => VK_LEFT.0 as u32,
        ArrowRight => VK_RIGHT.0 as u32,
        ArrowUp => VK_UP.0 as u32,
        Escape => VK_ESCAPE.0 as u32,
        Tab => VK_TAB.0 as u32,
        Backspace => VK_BACK.0 as u32,
        Enter => VK_RETURN.0 as u32,
        Space => VK_SPACE.0 as u32,
        Insert => VK_INSERT.0 as u32,
        Delete => VK_DELETE.0 as u32,
        Home => VK_HOME.0 as u32,
        End => VK_END.0 as u32,
        PageUp => VK_PRIOR.0 as u32,
        PageDown => VK_NEXT.0 as u32,
        Colon | Semicolon => VK_OEM_1.0 as u32,
        Comma => VK_OEM_COMMA.0 as u32,
        Backslash | Pipe => VK_OEM_5.0 as u32,
        Slash | Questionmark => VK_OEM_2.0 as u32,
        OpenBracket | OpenCurlyBracket => VK_OEM_4.0 as u32,
        CloseBracket | CloseCurlyBracket => VK_OEM_6.0 as u32,
        Backtick => VK_OEM_3.0 as u32,
        Minus => VK_OEM_MINUS.0 as u32,
        Period => VK_OEM_PERIOD.0 as u32,
        Plus | Equals => VK_OEM_PLUS.0 as u32,
        Quote => VK_OEM_7.0 as u32,
        Exclamationmark => '1' as u32,
        Num0 => '0' as u32,
        Num1 => '1' as u32,
        Num2 => '2' as u32,
        Num3 => '3' as u32,
        Num4 => '4' as u32,
        Num5 => '5' as u32,
        Num6 => '6' as u32,
        Num7 => '7' as u32,
        Num8 => '8' as u32,
        Num9 => '9' as u32,
        A => 'A' as u32,
        B => 'B' as u32,
        C => 'C' as u32,
        D => 'D' as u32,
        E => 'E' as u32,
        F => 'F' as u32,
        G => 'G' as u32,
        H => 'H' as u32,
        I => 'I' as u32,
        J => 'J' as u32,
        K => 'K' as u32,
        L => 'L' as u32,
        M => 'M' as u32,
        N => 'N' as u32,
        O => 'O' as u32,
        P => 'P' as u32,
        Q => 'Q' as u32,
        R => 'R' as u32,
        S => 'S' as u32,
        T => 'T' as u32,
        U => 'U' as u32,
        V => 'V' as u32,
        W => 'W' as u32,
        X => 'X' as u32,
        Y => 'Y' as u32,
        Z => 'Z' as u32,
        F1 => VK_F1.0 as u32,
        F2 => VK_F1.0 as u32 + 1,
        F3 => VK_F1.0 as u32 + 2,
        F4 => VK_F1.0 as u32 + 3,
        F5 => VK_F1.0 as u32 + 4,
        F6 => VK_F1.0 as u32 + 5,
        F7 => VK_F1.0 as u32 + 6,
        F8 => VK_F1.0 as u32 + 7,
        F9 => VK_F1.0 as u32 + 8,
        F10 => VK_F1.0 as u32 + 9,
        F11 => VK_F1.0 as u32 + 10,
        F12 => VK_F1.0 as u32 + 11,
        F13 => VK_F1.0 as u32 + 12,
        F14 => VK_F1.0 as u32 + 13,
        F15 => VK_F1.0 as u32 + 14,
        F16 => VK_F1.0 as u32 + 15,
        F17 => VK_F1.0 as u32 + 16,
        F18 => VK_F1.0 as u32 + 17,
        F19 => VK_F1.0 as u32 + 18,
        F20 => VK_F1.0 as u32 + 19,
        F21 => VK_F1.0 as u32 + 20,
        F22 => VK_F1.0 as u32 + 21,
        F23 => VK_F1.0 as u32 + 22,
        F24 => VK_F1.0 as u32 + 23,
        BrowserBack => VK_BROWSER_BACK.0 as u32,
        _ => return None,
    })
}

fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(14.0)
            .strong()
            .color(ui.visuals().text_color()),
    );
}

fn run_window(
    config: Config,
    status: FrontendStatus,
    sender: Sender<SettingsAction>,
    focus_hotkey: bool,
) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([500.0, 640.0])
            .with_min_inner_size([460.0, 540.0])
            .with_title("Voime 设置")
            .with_icon(crate::icon::window_icon()),
        centered: true,
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::windows::EventLoopBuilderExtWindows;
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    eframe::run_native(
        "Voime 设置",
        options,
        Box::new(move |creation| {
            install_fonts(&creation.egui_ctx);
            configure_style(&creation.egui_ctx);
            Ok(Box::new(SettingsApp::new(
                config,
                status,
                sender,
                focus_hotkey,
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!("设置窗口运行失败：{error}"))
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let fonts_dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    if let Ok(bytes) = std::fs::read(fonts_dir.join("segoeui.ttf")) {
        fonts.font_data.insert(
            "system-ui".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            family.insert(0, "system-ui".to_owned());
        }
    }
    for name in ["msyh.ttc", "Deng.ttf", "simhei.ttf", "NotoSansSC-VF.ttf"] {
        let Ok(bytes) = std::fs::read(fonts_dir.join(name)) else {
            continue;
        };
        fonts.font_data.insert(
            "system-cjk".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let list = fonts.families.entry(family).or_default();
            if !list.iter().any(|name| name == "system-cjk") {
                list.push("system-cjk".to_owned());
            }
        }
        break;
    }
    ctx.set_fonts(fonts);
}

fn configure_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.visuals.selection.bg_fill = ACCENT;
        style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
        style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(24.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(14.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
        );
    });
}

fn show_error_text(text: &str) {
    let text = to_wide(text);
    unsafe {
        MessageBoxW(
            HWND::default(),
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
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

    #[test]
    fn maps_egui_keys_to_virtual_keys() {
        assert_eq!(egui_key_to_vk(egui::Key::F9), Some(VK_F1.0 as u32 + 8));
        assert_eq!(
            egui_key_to_vk(egui::Key::Semicolon),
            Some(VK_OEM_1.0 as u32)
        );
        assert_eq!(egui_key_to_vk(egui::Key::Num5), Some('5' as u32));
        assert_eq!(egui_key_to_vk(egui::Key::F25), None);
    }

    #[test]
    fn save_sends_config_and_finishes_on_ack() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut app = SettingsApp::new(
            Config::default(),
            FrontendStatus::Disconnected,
            sender,
            false,
        );
        app.mode = Mode::Toggle;
        app.hotkey = crate::hotkey::parse("Ctrl+Alt+F9").unwrap();
        app.overlay_enabled = false;
        app.start_at_login = false;
        let ctx = egui::Context::default();
        app.save(&ctx);
        let SettingsAction::Save { config, ack } = receiver.try_recv().unwrap();
        assert_eq!(config.mode, Mode::Toggle);
        assert_eq!(config.hotkey.to_string(), "Ctrl+Alt+F9");
        assert!(!config.overlay_enabled);
        assert!(!config.start_at_login);
        ack.send(Ok(())).unwrap();
        app.poll_save(&ctx);
        assert!(!app.saving);
        assert!(app.save_wait.is_none());
    }

    #[test]
    fn settings_window_opens_and_closes() {
        let worker = std::thread::spawn(|| {
            run_process(Config::default(), FrontendStatus::Disconnected, false).unwrap();
        });
        let mut hwnd = HWND::default();
        for _ in 0..500 {
            if let Ok(found) = unsafe { FindWindowW(PCWSTR::null(), w!("Voime 设置")) } {
                hwnd = found;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!hwnd.0.is_null());
        unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        worker.join().unwrap();
    }
}
