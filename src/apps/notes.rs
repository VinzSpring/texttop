use std::fs;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};

pub struct Notes {
    path: PathBuf,
    lines: Vec<String>,
    cx: usize,
    cy: usize,
    scroll: usize,
    modified: bool,
    status: String,
}

fn bidx(s: &str, cx: usize) -> usize {
    s.char_indices().nth(cx).map(|(i, _)| i).unwrap_or(s.len())
}

fn notes_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".texttop_notes.txt")
}

impl Notes {
    pub fn new() -> Self {
        let path = notes_path();
        let (lines, status) = match fs::read_to_string(&path) {
            Ok(text) => {
                let mut v: Vec<String> = text.replace("\r\n", "\n").split('\n').map(|s| s.to_string()).collect();
                if v.is_empty() {
                    v.push(String::new());
                }
                (v, format!("loaded {} · Ctrl+S saves", path.display()))
            }
            Err(_) => (
                vec![
                    "Sticky notes".into(),
                    String::new(),
                    "Jot anything here — saved to ~/.texttop_notes.txt".into(),
                    String::new(),
                ],
                "new notes file · Ctrl+S saves".into(),
            ),
        };
        Notes {
            path,
            lines,
            cx: 0,
            cy: 0,
            scroll: 0,
            modified: false,
            status,
        }
    }

    fn line_len(&self, y: usize) -> usize {
        self.lines.get(y).map(|l| l.chars().count()).unwrap_or(0)
    }

    fn save(&mut self) {
        match fs::write(&self.path, self.lines.join("\n")) {
            Ok(()) => {
                self.modified = false;
                self.status = format!("saved ✓ · {}", self.path.display());
            }
            Err(e) => self.status = format!("✗ {e}"),
        }
    }

    fn clamp_cx(&mut self) {
        let len = self.line_len(self.cy);
        if self.cx > len {
            self.cx = len;
        }
    }
}

impl AppWindow for Notes {
    fn title(&self) -> String {
        if self.modified {
            "Notes •".into()
        } else {
            "Notes".into()
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        if area.height < 2 {
            return;
        }
        let text_h = area.height as usize - 1;
        if self.cy < self.scroll {
            self.scroll = self.cy;
        }
        if self.cy >= self.scroll + text_h {
            self.scroll = self.cy + 1 - text_h;
        }

        let mut out = Vec::new();
        for (i, line) in self.lines.iter().enumerate().skip(self.scroll).take(text_h) {
            let ln = i + 1;
            let gutter = Style::default().fg(Color::Rgb(80, 90, 120));
            let body = if focused && i == self.cy {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::Rgb(210, 215, 230))
            };
            let mut spans = vec![Span::styled(format!("{ln:3} "), gutter)];
            if focused && i == self.cy {
                let (a, b) = (
                    line.chars().take(self.cx).collect::<String>(),
                    line.chars().skip(self.cx).collect::<String>(),
                );
                spans.push(Span::styled(a, body));
                spans.push(Span::styled(
                    "│",
                    Style::default()
                        .fg(Color::Rgb(0, 220, 255))
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(b, body));
            } else {
                spans.push(Span::styled(line.clone(), body));
            }
            out.push(Line::from(spans));
        }
        f.render_widget(
            Paragraph::new(out),
            Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1)),
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" {} ", self.status),
                Style::default()
                    .fg(Color::Rgb(140, 150, 175))
                    .bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('s') if ctrl => self.save(),
            KeyCode::Up => {
                self.cy = self.cy.saturating_sub(1);
                self.clamp_cx();
            }
            KeyCode::Down => {
                if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.clamp_cx();
                }
            }
            KeyCode::Left => {
                if self.cx > 0 {
                    self.cx -= 1;
                } else if self.cy > 0 {
                    self.cy -= 1;
                    self.cx = self.line_len(self.cy);
                }
            }
            KeyCode::Right => {
                let len = self.line_len(self.cy);
                if self.cx < len {
                    self.cx += 1;
                } else if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.cx = 0;
                }
            }
            KeyCode::Home => self.cx = 0,
            KeyCode::End => self.cx = self.line_len(self.cy),
            KeyCode::Enter => {
                let line = self.lines[self.cy].clone();
                let i = bidx(&line, self.cx);
                let (a, b) = line.split_at(i);
                self.lines[self.cy] = a.to_string();
                self.lines.insert(self.cy + 1, b.to_string());
                self.cy += 1;
                self.cx = 0;
                self.modified = true;
            }
            KeyCode::Backspace => {
                if self.cx > 0 {
                    let line = &mut self.lines[self.cy];
                    let i = bidx(line, self.cx - 1);
                    let j = bidx(line, self.cx);
                    line.replace_range(i..j, "");
                    self.cx -= 1;
                    self.modified = true;
                } else if self.cy > 0 {
                    let cur = self.lines.remove(self.cy);
                    self.cy -= 1;
                    self.cx = self.line_len(self.cy);
                    self.lines[self.cy].push_str(&cur);
                    self.modified = true;
                }
            }
            KeyCode::Delete => {
                let len = self.line_len(self.cy);
                if self.cx < len {
                    let line = &mut self.lines[self.cy];
                    let i = bidx(line, self.cx);
                    let j = bidx(line, self.cx + 1);
                    line.replace_range(i..j, "");
                    self.modified = true;
                } else if self.cy + 1 < self.lines.len() {
                    let next = self.lines.remove(self.cy + 1);
                    self.lines[self.cy].push_str(&next);
                    self.modified = true;
                }
            }
            KeyCode::Char(c) if !ctrl => {
                let line = &mut self.lines[self.cy];
                let i = bidx(line, self.cx);
                line.insert(i, c);
                self.cx += 1;
                self.modified = true;
            }
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        match m.kind {
            MouseEventKind::ScrollUp => {
                self.cy = self.cy.saturating_sub(3);
                self.clamp_cx();
            }
            MouseEventKind::ScrollDown => {
                self.cy = (self.cy + 3).min(self.lines.len().saturating_sub(1));
                self.clamp_cx();
            }
            _ => {}
        }
        Action::None
    }
}
