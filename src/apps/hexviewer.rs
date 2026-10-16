use std::fs;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::scroll::ScrollState;
use super::ui::{self, Controls, Item, Look};
use super::{Action, AppWindow, Launch};
use crate::gfx;

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    Open,
    Top,
    PageUp,
    PageDown,
    End,
    Narrower,
    Wider,
}

pub struct HexViewer {
    path: Option<PathBuf>,
    data: Vec<u8>,
    offset: usize,
    status: String,
    cols: usize,
    /// Visible data rows at the last render, for paging.
    rows: usize,
    ui: Controls<Cmd>,
    bar: ScrollState,
}

impl HexViewer {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut hv = HexViewer {
            path: None,
            data: Vec::new(),
            offset: 0,
            status: "no file".into(),
            cols: 16,
            rows: 20,
            ui: Controls::new(),
            bar: ScrollState::default(),
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

    fn scroll_by(&mut self, rows: isize) {
        let step = self.cols as isize * rows;
        let max = self.max_offset(self.rows) as isize;
        self.offset = (self.offset as isize + step).clamp(0, max.max(0)) as usize;
        self.offset -= self.offset % self.cols;
    }

    /// Scrollbar units: rows, scaled down for files longer than a u16 of rows.
    fn bar_scale(&self) -> usize {
        let total = self.data.len().div_ceil(self.cols.max(1));
        total.div_ceil(u16::MAX as usize - 1).max(1)
    }

    fn sync_bar(&mut self) {
        let scale = self.bar_scale();
        self.bar.offset = (self.offset / self.cols.max(1) / scale) as u16;
    }

    fn run(&mut self, c: Cmd) -> Action {
        let page = self.rows.max(1) as isize;
        match c {
            Cmd::Open => {
                let dir = self
                    .path
                    .as_ref()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(super::state_home);
                return Action::Launch(Launch::Files(dir));
            }
            Cmd::Top => self.offset = 0,
            Cmd::PageUp => self.scroll_by(-page),
            Cmd::PageDown => self.scroll_by(page),
            Cmd::End => {
                self.offset = self.max_offset(self.rows);
                self.scroll_by(1);
            }
            Cmd::Narrower => self.cols = self.cols.saturating_sub(4).max(8),
            Cmd::Wider => self.cols = (self.cols + 4).min(32),
        }
        Action::None
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

    fn render(&mut self, f: &mut Frame, full: Rect, _focused: bool) {
        if full.height < 3 {
            return;
        }
        self.ui.begin();
        let loaded = !self.data.is_empty();
        let nav = Look::enabled_if(loaded);
        // never wider than the window can show: " XXXXXXXX  " + 3*cols + 2 + cols
        let fit = (full.width.saturating_sub(14) as usize / 4).max(8);
        self.cols = self.cols.min(fit).max(8);
        self.ui.toolbar(
            f.buffer_mut(),
            Rect::new(full.x, full.y, full.width, 1),
            &[
                Item::btn("📂 Open…", Cmd::Open),
                Item::Sep,
                Item::with("⤒", Cmd::Top, nav),
                Item::with("▲ Page", Cmd::PageUp, nav),
                Item::with("▼ Page", Cmd::PageDown, nav),
                Item::with("⤓", Cmd::End, nav),
            ],
            &[
                Item::Label(format!("{} bytes/row", self.cols), ui::DIM),
                Item::with("−", Cmd::Narrower, Look::enabled_if(self.cols > 8)),
                Item::with("+", Cmd::Wider, Look::enabled_if(self.cols < 32.min(fit))),
            ],
        );
        let area = Rect::new(full.x, full.y + 1, full.width, full.height - 1);
        let rows = area.height.saturating_sub(1) as usize;
        self.rows = rows;
        let total_rows = self.data.len().div_ceil(self.cols.max(1));
        let overflow = total_rows > rows;
        let area = Rect::new(area.x, area.y, area.width - u16::from(overflow), area.height);

        let addr = Style::default().fg(Color::Rgb(100, 180, 255));
        let hex = Style::default().fg(Color::Rgb(210, 215, 230));
        let asc = Style::default().fg(Color::Rgb(140, 220, 160));
        let dim = Style::default().fg(Color::Rgb(120, 130, 150));

        let mut lines = Vec::new();
        if self.data.is_empty() {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                "  No file loaded — click Open…, or drop a file onto this window.",
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
        self.sync_bar();
        let scale = self.bar_scale();
        let bar = overflow.then(|| Rect::new(area.x + area.width, area.y, 1, rows as u16));
        self.bar.show_bar(f.buffer_mut(), bar, total_rows.div_ceil(scale), (rows / scale).max(1) as u16);
        f.render_widget(
            Paragraph::new(Span::styled(
                if loaded {
                    let pct = (self.offset + self.cols * rows).min(self.data.len()) * 100
                        / self.data.len().max(1);
                    format!(" {}  │ offset {:08x} · {pct}% ", self.status, self.offset)
                } else {
                    format!(" {} ", self.status)
                },
                Style::default()
                    .fg(Color::Rgb(140, 150, 175))
                    .bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
        self.ui.render_menu(f.buffer_mut(), full);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Up => self.scroll_by(-1),
            KeyCode::Down => self.scroll_by(1),
            KeyCode::PageUp => return self.run(Cmd::PageUp),
            KeyCode::PageDown => return self.run(Cmd::PageDown),
            KeyCode::Home => return self.run(Cmd::Top),
            KeyCode::End => return self.run(Cmd::End),
            KeyCode::Char('+') | KeyCode::Char('=') => return self.run(Cmd::Wider),
            KeyCode::Char('-') => return self.run(Cmd::Narrower),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            KeyCode::Char('g') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.offset = 0;
            }
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        if let Some(hit) = self.ui.route(&m) {
            return hit.map_or(Action::None, |c| self.run(c));
        }
        if !matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
            self.sync_bar();
            let scale = self.bar_scale();
            let total = self.data.len().div_ceil(self.cols.max(1)).div_ceil(scale);
            if self.bar.on_mouse(m, total, (self.rows / scale).max(1) as u16) {
                let row = self.bar.offset as usize * scale;
                self.offset = (row * self.cols).min(self.max_offset(self.rows));
                self.offset -= self.offset % self.cols.max(1);
                return Action::None;
            }
        }
        match m.kind {
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::ScrollDown => self.scroll_by(3),
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