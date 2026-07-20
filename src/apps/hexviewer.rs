use std::fs;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};
use crate::gfx;

pub struct HexViewer {
    path: Option<PathBuf>,
    data: Vec<u8>,
    offset: usize,
    status: String,
    cols: usize,
}

impl HexViewer {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut hv = HexViewer {
            path: None,
            data: Vec::new(),
            offset: 0,
            status: "open a file from Files (F3) or drop a path".into(),
            cols: 16,
        };
        if let Some(p) = path {
            hv.load(p);
        }
        hv
    }

    fn load(&mut self, path: PathBuf) {
        match fs::read(&path) {
            Ok(bytes) => {
                let n = bytes.len();
                self.status = format!("{} · {}", path.display(), gfx::fmt_size(n as u64));
                self.data = bytes;
                self.path = Some(path);
                self.offset = 0;
            }
            Err(e) => {
                self.status = format!("✗ {e}");
                self.data.clear();
                self.path = None;
            }
        }
    }

    fn max_offset(&self, rows: usize) -> usize {
        let per = self.cols * rows.max(1);
        self.data.len().saturating_sub(per)
    }
}

impl AppWindow for HexViewer {
    fn title(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| format!("Hex · {}", n.to_string_lossy()))
            .unwrap_or_else(|| "Hex Viewer".into())
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 2 {
            return;
        }
        let rows = area.height.saturating_sub(1) as usize;
        // fit columns to width: " XXXXXXXX  " + 3*cols + 2 + cols
        let avail = area.width.saturating_sub(14) as usize;
        self.cols = (avail / 4).clamp(8, 32);

        let addr = Style::default().fg(Color::Rgb(100, 180, 255));
        let hex = Style::default().fg(Color::Rgb(210, 215, 230));
        let asc = Style::default().fg(Color::Rgb(140, 220, 160));
        let dim = Style::default().fg(Color::Rgb(120, 130, 150));

        let mut lines = Vec::new();
        if self.data.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No file loaded. In Files, select a file and press F3.",
                dim,
            )));
        } else {
            let mut off = self.offset;
            for _ in 0..rows {
                if off >= self.data.len() {
                    break;
                }
                let end = (off + self.cols).min(self.data.len());
                let chunk = &self.data[off..end];
                let mut hex_s = String::new();
                for (i, b) in chunk.iter().enumerate() {
                    if i > 0 && i % 8 == 0 {
                        hex_s.push(' ');
                    }
                    hex_s.push_str(&format!("{b:02x} "));
                }
                let pad = self.cols.saturating_sub(chunk.len());
                for _ in 0..pad {
                    hex_s.push_str("   ");
                }
                let ascii: String = chunk
                    .iter()
                    .map(|&b| {
                        if (32..127).contains(&b) {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                lines.push(Line::from(vec![
                    Span::styled(format!(" {off:08x}  "), addr),
                    Span::styled(hex_s, hex),
                    Span::styled(format!(" {ascii}"), asc),
                ]));
                off += self.cols;
            }
        }

        f.render_widget(Paragraph::new(lines), Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1)));
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" {}  │ ↑↓ PgUp/PgDn · Home/End · +/- width ", self.status),
                Style::default()
                    .fg(Color::Rgb(140, 150, 175))
                    .bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let rows = 20usize;
        let step = self.cols;
        match k.code {
            KeyCode::Up => self.offset = self.offset.saturating_sub(step),
            KeyCode::Down => {
                self.offset = (self.offset + step).min(self.max_offset(rows));
            }
            KeyCode::PageUp => self.offset = self.offset.saturating_sub(step * 16),
            KeyCode::PageDown => {
                self.offset = (self.offset + step * 16).min(self.max_offset(rows));
            }
            KeyCode::Home => self.offset = 0,
            KeyCode::End => self.offset = self.max_offset(rows),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.cols = (self.cols + 4).min(32);
            }
            KeyCode::Char('-') => {
                self.cols = self.cols.saturating_sub(4).max(8);
            }
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            KeyCode::Char('g') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.offset = 0;
            }
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        match m.kind {
            MouseEventKind::ScrollUp => self.offset = self.offset.saturating_sub(self.cols),
            MouseEventKind::ScrollDown => {
                self.offset = (self.offset + self.cols).min(self.data.len().saturating_sub(1));
            }
            _ => {}
        }
        Action::None
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    fn on_drop(&mut self, paths: &[PathBuf], _cut: bool) -> Action {
        if let Some(p) = paths.first().filter(|p| p.is_file()) {
            self.load(p.clone());
        }
        Action::None
    }
}