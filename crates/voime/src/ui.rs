use std::io::{self, IsTerminal, Stdout};

use anyhow::Context;
use crossterm::cursor::{Hide, Show};
use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Terminal;
use unicode_width::UnicodeWidthChar;

use crate::app::App;

pub mod theme;

pub const GUTTER_WIDTH: u16 = 2;
pub const MARKER: &str = "●";
pub const FOOTER_HEIGHT: u16 = 1;

pub struct Tui {
    terminal: Option<Terminal<CrosstermBackend<Stdout>>>,
    restored: bool,
}

impl Tui {
    pub fn init() -> anyhow::Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            anyhow::bail!("终端不可用：stdin 与 stdout 必须是 TTY");
        }
        enable_raw_mode().context("启用终端 raw mode 失败")?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide, DisableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error).context("进入备用屏幕失败");
        }
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                let mut stdout = io::stdout();
                let _ = execute!(stdout, Show, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(error).context("初始化终端失败");
            }
        };
        let _ = terminal.clear();
        install_panic_hook();
        Ok(Self {
            terminal: Some(terminal),
            restored: false,
        })
    }

    pub fn draw(&mut self, app: &mut App) -> anyhow::Result<()> {
        let Some(terminal) = self.terminal.as_mut() else {
            return Ok(());
        };
        terminal
            .draw(|frame| {
                let area = frame.area();
                let content_area = Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: area.height.saturating_sub(FOOTER_HEIGHT),
                };
                app.viewport_height = content_area.height;
                let lines = build_lines(app, content_area.width);
                let total = lines.len();
                let max_offset = total.saturating_sub(content_area.height as usize);
                if app.scroll.follow || app.scroll.offset > max_offset {
                    app.scroll.offset = max_offset;
                }
                let offset = app.scroll.offset.min(u16::MAX as usize) as u16;
                let paragraph = Paragraph::new(lines).scroll((offset, 0));
                frame.render_widget(paragraph, content_area);

                let footer = Rect {
                    x: area.x,
                    y: area.y + content_area.height,
                    width: area.width,
                    height: FOOTER_HEIGHT.min(area.height),
                };
                let footer_text = footer_text(app);
                frame.render_widget(
                    Paragraph::new(footer_text)
                        .style(Style::default().fg(theme::TEXT).add_modifier(Modifier::DIM)),
                    footer,
                );
            })
            .context("绘制终端界面失败")?;
        Ok(())
    }

    pub fn restore(mut self) {
        self.restore_inner();
    }

    fn restore_inner(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let mut stdout = io::stdout();
        let _ = execute!(stdout, Show, DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        self.terminal.take();
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        self.restore_inner();
    }
}

fn build_lines<'a>(app: &App, area_width: u16) -> Vec<Line<'a>> {
    let content_width = area_width.saturating_sub(GUTTER_WIDTH) as usize;
    let mut lines: Vec<Line<'a>> = Vec::new();

    for text in &app.transcript {
        push_wrapped(&mut lines, text, content_width, false);
    }

    let show_partial = app.recording || app.finalizing || !app.partial.is_empty();
    if show_partial {
        if app.partial.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(format!("{MARKER} "), Style::default().fg(theme::RECORDING)),
                Span::raw(""),
            ]));
        } else {
            push_wrapped(&mut lines, &app.partial, content_width, app.recording);
        }
    }
    lines
}

fn footer_text(app: &App) -> String {
    let mode = app.mode.map(|mode| mode.as_str()).unwrap_or("-");
    let hotkey = app.hotkey.as_deref().unwrap_or("-");
    format!(
        "Mode: {mode}  Hotkey: {hotkey}  Backend: {}  Ctrl+X 清空  q 退出",
        app.backend
    )
}

fn push_wrapped<'a>(lines: &mut Vec<Line<'a>>, text: &str, content_width: usize, marker: bool) {
    let wrapped = wrap_text(text, content_width);
    for (index, visual) in wrapped.into_iter().enumerate() {
        let first = index == 0;
        let prefix = if first && marker {
            Span::styled(format!("{MARKER} "), Style::default().fg(theme::RECORDING))
        } else {
            Span::raw(" ".repeat(GUTTER_WIDTH as usize))
        };
        lines.push(Line::from(vec![prefix, Span::raw(visual)]));
    }
}

pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    if width == 0 {
        return vec![String::new()];
    }
    let mut result = Vec::new();
    for logical in text.split('\n') {
        let mut current = String::new();
        let mut current_width = 0usize;
        for character in logical.chars() {
            let character = if character.is_control() {
                ' '
            } else {
                character
            };
            let character_width = character.width().unwrap_or(0);
            if current_width + character_width > width && !current.is_empty() {
                result.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push(character);
            current_width += character_width;
            if current_width >= width && character_width > 0 {
                result.push(std::mem::take(&mut current));
                current_width = 0;
            }
        }
        if !current.is_empty() || logical.is_empty() {
            result.push(current);
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

fn install_panic_hook() {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let mut stdout = io::stdout();
            let _ = execute!(stdout, Show, DisableMouseCapture, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_ascii_by_display_width() {
        assert_eq!(wrap_text("abcdef", 5), vec!["abcde", "f"]);
    }

    #[test]
    fn wraps_cjk_by_two_cells() {
        assert_eq!(wrap_text("你好世界", 4), vec!["你好", "世界"]);
    }

    #[test]
    fn keeps_empty_line() {
        assert_eq!(wrap_text("", 10), vec![""]);
    }

    #[test]
    fn footer_shows_mode_hotkey_and_actions() {
        let mut app = App::new();
        app.mode = Some(voime_ipc::Mode::Ptt);
        app.hotkey = Some("Super+F2".to_string());
        app.backend = voime_ipc::Backend::Gpu;
        assert_eq!(
            footer_text(&app),
            "Mode: ptt  Hotkey: Super+F2  Backend: gpu  Ctrl+X 清空  q 退出"
        );
    }
}
