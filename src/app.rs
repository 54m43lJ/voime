use std::time::Duration;

use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::cli::Mode;
use crate::clipboard::ClipboardWriter;
use crate::pipeline::{Pipeline, PipelineEvent};
use crate::ui::Tui;

pub enum Control {
    Continue,
    Quit,
}

pub struct App {
    pub mode: Mode,
    pub transcript: Vec<String>,
    pub partial: String,
    pub recording: bool,
    pub finalizing: bool,
    pub scroll: ScrollState,
    pub clipboard: ClipboardWriter,
    pub fatal: Option<String>,
    pub viewport_height: u16,
}

pub fn run(mode: Mode, pipeline: &mut Pipeline, clipboard: ClipboardWriter) -> anyhow::Result<()> {
    let mut tui = Tui::init()?;
    let mut app = App::new(mode, clipboard);
    let mut exit_error = None;

    loop {
        let mut quit = false;
        loop {
            match pipeline.events.try_recv() {
                Ok(event) => {
                    app.on_pipeline(event);
                    if let Some(error) = app.fatal.take() {
                        exit_error = Some(anyhow::anyhow!(error));
                        quit = true;
                        break;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    exit_error = Some(anyhow::anyhow!("pipeline 工作线程意外退出"));
                    quit = true;
                    break;
                }
            }
        }
        if quit {
            break;
        }

        tui.draw(&mut app)?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                event::Event::Key(key) => {
                    if let Control::Quit = app.on_key(key, pipeline) {
                        if let Some(error) = app.fatal.take() {
                            exit_error = Some(anyhow::anyhow!(error));
                        }
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

impl App {
    pub fn new(mode: Mode, clipboard: ClipboardWriter) -> Self {
        Self {
            mode,
            transcript: Vec::new(),
            partial: String::new(),
            recording: false,
            finalizing: false,
            scroll: ScrollState::default(),
            clipboard,
            fatal: None,
            viewport_height: 24,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, pipeline: &mut Pipeline) -> Control {
        if key.kind != KeyEventKind::Press {
            return Control::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            return Control::Quit;
        }
        match key.code {
            KeyCode::Char('q') if key.modifiers.is_empty() => return Control::Quit,
            KeyCode::Char(' ') if self.mode == Mode::Ptt && key.modifiers.is_empty() => {
                if !self.recording && !self.finalizing {
                    match pipeline.start_segment() {
                        Ok(()) => {
                            self.partial.clear();
                            self.recording = true;
                            self.finalizing = false;
                            self.scroll.end();
                        }
                        Err(error) => {
                            self.fatal = Some(format!("{error:#}"));
                            return Control::Quit;
                        }
                    }
                } else if self.recording {
                    match pipeline.finish_segment() {
                        Ok(()) => {
                            self.recording = false;
                            self.finalizing = true;
                        }
                        Err(error) => {
                            self.fatal = Some(format!("{error:#}"));
                            return Control::Quit;
                        }
                    }
                }
            }
            KeyCode::PageUp => self.scroll.page_up(self.viewport_height),
            KeyCode::PageDown => self.scroll.page_down(self.viewport_height),
            KeyCode::Home => self.scroll.home(),
            KeyCode::End => self.scroll.end(),
            _ => {}
        }
        Control::Continue
    }

    pub fn on_pipeline(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::Recording(true) => {
                self.recording = true;
                self.finalizing = false;
                self.partial.clear();
            }
            PipelineEvent::Recording(false) => {
                self.recording = false;
                if self.mode == Mode::Ptt {
                    self.finalizing = true;
                }
            }
            PipelineEvent::Partial(text) => self.set_partial(text),
            PipelineEvent::Final(line) => {
                self.push_final(line);
                self.partial.clear();
                self.recording = false;
                self.finalizing = false;
                let _ = self.clipboard.set_transcript(&self.transcript);
            }
            PipelineEvent::SegmentDropped => {
                self.recording = false;
                self.finalizing = false;
                self.partial.clear();
            }
            PipelineEvent::Fatal(error) => {
                self.fatal = Some(error);
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
}
