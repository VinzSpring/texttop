use std::fs;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};

pub struct Editor {
    path: Option<PathBuf>,
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

impl Editor {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut status = String::from("Ctrl+S saves");
        let lines = match &path {
            Some(p) => match fs::read(p) {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    if bytes.contains(&0) {
                        status = "⚠ binary file — editing not recommended".into();
                    }
                    let mut v: Vec<String> =
                        text.replace("\r\n", "\n").split('\n').map(|s| s.to_string()).collect();
                    if v.is_empty() {
                        v.push(String::new());
                    }
                    v
                }
                Err(e) => {
                    status = format!("✗ {e}");
                    vec![String::new()]
                }
            },
            None => vec![String::new()],
        };
        Editor {
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
        match &self.path {
            None => self.status = "untitled — open a file from the file manager to save".into(),
            Some(p) => match fs::write(p, self.lines.join("\n")) {
                Ok(_) => {
                    self.modified = false;
                    self.status = format!("saved {} lines ✓", self.lines.len());
                }
                Err(e) => self.status = format!("✗ save failed: {e}"),
            },
        }
    }
}

impl AppWindow for Editor {
    fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".into());
        if self.modified {
            format!("{name} •")
        } else {
            name
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
        let gutter = 5u16;
        let content_w = area.width.saturating_sub(gutter) as usize;
        let hscroll = self.cx.saturating_sub(content_w.saturating_sub(1));

        let mut out: Vec<Line> = Vec::new();
        for (i, line) in self.lines.iter().enumerate().skip(self.scroll).take(text_h) {
            let num = Span::styled(
                format!("{:>4} ", i + 1),
                Style::default().fg(Color::Rgb(90, 95, 120)),
            );
            let visible: String = line.chars().skip(hscroll).take(content_w).collect();
            let body = Span::styled(visible, Style::default().fg(Color::Rgb(220, 222, 232)));
            out.push(Line::from(vec![num, body]));
        }
        f.render_widget(Paragraph::new(out), Rect::new(area.x, area.y, area.width, text_h as u16));

        // cursor
        if focused {
            let cur_x = area.x + gutter + (self.cx - hscroll.min(self.cx)) as u16;
            let cur_y = area.y + (self.cy - self.scroll) as u16;
            if cur_x < area.x + area.width && cur_y < area.y + text_h as u16 {
                if let Some(c) = f.buffer_mut().cell_mut((cur_x, cur_y)) {
                    c.modifier |= Modifier::REVERSED;
                }
            }
        }

        let status = format!(
            " {}  ·  Ln {}, Col {}  ·  {} ",
            if self.modified { "modified •" } else { "saved ✓" },
            self.cy + 1,
            self.cx + 1,
            self.status
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                status,
                Style::default().fg(Color::Rgb(140, 150, 175)).bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('s') if ctrl => self.save(),
            KeyCode::Char(c) if !ctrl && !alt => {
                let i = bidx(&self.lines[self.cy], self.cx);
                self.lines[self.cy].insert(i, c);
                self.cx += 1;
                self.modified = true;
            }
            KeyCode::Tab => {
                let i = bidx(&self.lines[self.cy], self.cx);
                self.lines[self.cy].insert_str(i, "    ");
                self.cx += 4;
                self.modified = true;
            }
            KeyCode::Enter => {
                let i = bidx(&self.lines[self.cy], self.cx);
                let rest = self.lines[self.cy].split_off(i);
                self.lines.insert(self.cy + 1, rest);
                self.cy += 1;
                self.cx = 0;
                self.modified = true;
            }
            KeyCode::Backspace => {
                if self.cx > 0 {
                    let i = bidx(&self.lines[self.cy], self.cx - 1);
                    self.lines[self.cy].remove(i);
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
                if self.cx < self.line_len(self.cy) {
                    let i = bidx(&self.lines[self.cy], self.cx);
                    self.lines[self.cy].remove(i);
                    self.modified = true;
                } else if self.cy + 1 < self.lines.len() {
                    let next = self.lines.remove(self.cy + 1);
                    self.lines[self.cy].push_str(&next);
                    self.modified = true;
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
                if self.cx < self.line_len(self.cy) {
                    self.cx += 1;
                } else if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.cx = 0;
                }
            }
            KeyCode::Up => {
                self.cy = self.cy.saturating_sub(1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::Down => {
                self.cy = (self.cy + 1).min(self.lines.len() - 1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::PageUp => {
                self.cy = self.cy.saturating_sub(20);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::PageDown => {
                self.cy = (self.cy + 20).min(self.lines.len() - 1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::Home => self.cx = 0,
            KeyCode::End => self.cx = self.line_len(self.cy),
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        match m.kind {
            MouseEventKind::ScrollUp => {
                self.cy = self.cy.saturating_sub(3);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            MouseEventKind::ScrollDown => {
                self.cy = (self.cy + 3).min(self.lines.len() - 1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            MouseEventKind::Down(_) => {
                let row = m.row.saturating_sub(area.y) as usize + self.scroll;
                if row < self.lines.len() {
                    self.cy = row;
                    let col = m.column.saturating_sub(area.x + 5) as usize;
                    self.cx = col.min(self.line_len(self.cy));
                }
            }
            _ => {}
        }
        Action::None
    }
}
