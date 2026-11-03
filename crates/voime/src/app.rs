use std::time::Duration;

use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use voime_ipc::{
    Backend, ClientMessage, FrontendState, Mode, Phase, ServerMessage, Status, TriggerSource,
};

use crate::client::Client;
use crate::ui::Tui;

pub enum Control {
    Continue,
    Detach,
}

pub struct App {
    pub mode: Option<Mode>,
    pub hotkey: Option<String>,
    pub backend: Backend,
    pub transcript: Vec<String>,
    pub partial: String,
    pub recording: bool,
    pub finalizing: bool,
    pub addon: FrontendState,
    pub scroll: ScrollState,
    pub viewport_height: u16,
    client: Option<Client>,
    detached: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            mode: None,
            hotkey: None,
            backend: Backend::Cpu,
            transcript: Vec::new(),
            partial: String::new(),
            recording: false,
            finalizing: false,
            addon: FrontendState::Disconnected,
            scroll: ScrollState::default(),
            viewport_height: 24,
            client: None,
            detached: false,
        }
    }

    pub fn run(mut self, client: Client) -> anyhow::Result<()> {
        self.client = Some(client);
        let mut tui = Tui::init()?;
        let mut exit_error = None;

        loop {
            while let Some(event) = self.client.as_mut().and_then(Client::try_event) {
                self.on_event(event);
            }
            if self.detached {
                break;
            }
            if self.client.as_ref().is_some_and(Client::closed) {
                exit_error = Some(anyhow::anyhow!("daemon 连接已断开"));
                break;
            }

            tui.draw(&mut self)?;

            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    event::Event::Key(key) => {
                        if let Control::Detach = self.on_key(key) {
                            break;
                        }
                    }
                    event::Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }

        tui.restore();
        match exit_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Control {
        if key.kind != KeyEventKind::Press {
            return Control::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            return Control::Detach;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('x') | KeyCode::Char('X'))
        {
            self.send(&ClientMessage::ClearHistory {});
            return Control::Continue;
        }
        match key.code {
            KeyCode::Char('q') if key.modifiers.is_empty() => Control::Detach,
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                if self.recording {
                    self.send(&ClientMessage::Stop {
                        segment_id: 0,
                        source: TriggerSource::Tui,
                    });
                } else if !self.finalizing {
                    self.send(&ClientMessage::Start {
                        segment_id: 0,
                        source: TriggerSource::Tui,
                    });
                }
                Control::Continue
            }
            KeyCode::PageUp => {
                self.scroll.page_up(self.viewport_height.saturating_sub(1));
                Control::Continue
            }
            KeyCode::PageDown => {
                self.scroll
                    .page_down(self.viewport_height.saturating_sub(1));
                Control::Continue
            }
            KeyCode::Home => {
                self.scroll.home();
                Control::Continue
            }
            KeyCode::End => {
                self.scroll.end();
                Control::Continue
            }
            _ => Control::Continue,
        }
    }

    pub fn on_event(&mut self, event: ServerMessage) {
        match event {
            ServerMessage::Snapshot {
                mode,
                phase,
                partial,
                transcript,
                addon,
                hotkey,
                backend,
            } => {
                self.mode = mode;
                self.hotkey = hotkey;
                self.backend = backend;
                self.transcript = transcript;
                self.partial = partial.unwrap_or_default();
                self.addon = addon;
                self.set_phase(phase);
            }
            ServerMessage::BackendChanged { backend } => {
                self.backend = backend;
            }
            ServerMessage::SegmentStarted { .. } => {
                self.recording = true;
                self.finalizing = false;
                self.partial.clear();
                self.scroll.end();
            }
            ServerMessage::State {
                recording,
                finalizing,
                ..
            } => {
                self.recording = recording;
                self.finalizing = finalizing;
                if !recording && !finalizing {
                    self.partial.clear();
                }
            }
            ServerMessage::Partial { text, .. } => self.set_partial(text),
            ServerMessage::Final { text, .. } => {
                self.push_final(text);
                self.partial.clear();
                self.recording = false;
                self.finalizing = false;
            }
            ServerMessage::HistoryCleared {} => {
                self.transcript.clear();
            }
            ServerMessage::Status { status } => {
                self.apply_status(status);
            }
            ServerMessage::Error { code, message } => {
                eprintln!("voime: daemon 错误（{code:?}）：{message}");
            }
            ServerMessage::Bye {} => {
                self.detached = true;
            }
        }
    }

    pub fn push_final(&mut self, line: String) {
        let line = sanitize(&line);
        if !line.is_empty() {
            self.transcript.push(line);
        }
    }

    pub fn set_partial(&mut self, text: String) {
        let text = sanitize(&text);
        if !text.is_empty() {
            self.partial = text;
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        self.recording = phase == Phase::Recording;
        self.finalizing = phase == Phase::Finalizing;
    }

    fn apply_status(&mut self, status: Status) {
        self.mode = status.mode;
        self.hotkey = status.hotkey;
        self.addon = status.addon;
        self.backend = status.backend;
        self.recording = status.recording;
        self.finalizing = status.finalizing;
    }

    fn send(&mut self, message: &ClientMessage) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        if let Err(error) = client.send(message) {
            eprintln!("voime: 发送命令失败：{error:#}");
            self.detached = true;
        }
    }
}

fn sanitize(text: &str) -> String {
    text.replace(['\r', '\n'], " ").trim().to_string()
}

#[derive(Clone, Copy, Debug)]
pub struct ScrollState {
    pub offset: usize,
    pub follow: bool,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            offset: 0,
            follow: true,
        }
    }
}

impl ScrollState {
    pub fn page_up(&mut self, height: u16) {
        self.follow = false;
        self.offset = self.offset.saturating_sub(page_step(height));
    }

    pub fn page_down(&mut self, height: u16) {
        self.offset = self.offset.saturating_add(page_step(height));
    }

    pub fn home(&mut self) {
        self.follow = false;
        self.offset = 0;
    }

    pub fn end(&mut self) {
        self.follow = true;
    }
}

fn page_step(height: u16) -> usize {
    height.saturating_sub(1).max(1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_up_stops_following() {
        let mut scroll = ScrollState {
            offset: 10,
            follow: true,
        };
        scroll.page_up(5);
        assert!(!scroll.follow);
        assert_eq!(scroll.offset, 6);
    }

    #[test]
    fn end_resumes_following() {
        let mut scroll = ScrollState {
            offset: 10,
            follow: false,
        };
        scroll.end();
        assert!(scroll.follow);
    }

    #[test]
    fn snapshot_populates_view() {
        let mut app = App::new();
        app.on_event(ServerMessage::Snapshot {
            mode: Some(Mode::Ptt),
            phase: Phase::Recording,
            partial: Some("临时".to_string()),
            transcript: vec!["历史".to_string()],
            addon: FrontendState::Connected,
            hotkey: Some("Super+F2".to_string()),
            backend: Backend::Gpu,
        });
        assert_eq!(app.mode, Some(Mode::Ptt));
        assert!(app.recording);
        assert_eq!(app.backend, Backend::Gpu);
        assert_eq!(app.transcript, vec!["历史".to_string()]);
        assert_eq!(app.partial, "临时");
    }
}
