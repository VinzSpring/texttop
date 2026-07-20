//! Visual diff of N selected files — pick any two to compare.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::scroll::{self, ScrollState};
use super::snaputil::{self, DiffKind, DiffLine};
use super::{Action, AppWindow};

pub struct CompareApp {
    paths: Vec<PathBuf>,
    left: usize,
    right: usize,
    focus_list: bool,
    list_sel: usize,
    diff: Vec<DiffLine>,
    scroll: ScrollState,
    status: String,
    view_h: u16,
}

impl CompareApp {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        let mut paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| p.is_file())
            .collect();
        paths.sort();
        paths.dedup();
        let mut app = CompareApp {
            left: 0,
            right: paths.len().saturating_sub(1).min(1),
            focus_list: true,
            list_sel: 0,
            diff: Vec::new(),
            scroll: ScrollState::default(),
            status: String::new(),
            view_h: 20,
            paths,
        };
        if app.paths.len() < 2 {
            app.status = "need at least 2 files to compare".into();
        } else {
            app.recompute();
        }
        app
    }

    fn label(&self, i: usize) -> String {
        self.paths
            .get(i)
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "?".into())
    }

    fn recompute(&mut self) {
        if self.paths.len() < 2 {
            self.diff.clear();
            return;
        }
        self.left = self.left.min(self.paths.len() - 1);
        self.right = self.right.min(self.paths.len() - 1);
        if self.left == self.right && self.paths.len() > 1 {
            self.right = (self.left + 1) % self.paths.len();
        }
        match snaputil::diff_files(&self.paths[self.left], &self.paths[self.right]) {
            Ok(d) => {
                self.diff = d;
                self.status = format!(
                    "{} ↔ {} · {} lines",
                    self.label(self.left),
                    self.label(self.right),
                    self.diff.len()
                );
            }
            Err(e) => {
                self.diff = vec![DiffLine {
                    kind: DiffKind::Header,
                    text: e,
                }];
                self.status = "diff error".into();
            }
        }
        self.scroll.offset = 0;
    }

    fn colored_lines(&self) -> Vec<Line<'static>> {
        self.diff
            .iter()
            .map(|d| {
                let (fg, bold) = match d.kind {
                    DiffKind::Add => (Color::Rgb(120, 220, 140), false),
                    DiffKind::Del => (Color::Rgb(255, 120, 120), false),
                    DiffKind::Header => (Color::Rgb(100, 180, 255), true),
                    DiffKind::Context => (Color::Rgb(170, 175, 190), false),
                };
                let mut st = Style::default().fg(fg);
                if bold {
                    st = st.add_modifier(Modifier::BOLD);
                }
                Line::from(Span::styled(d.text.clone(), st))
            })
            .collect()
    }
}

impl AppWindow for CompareApp {
    fn title(&self) -> String {
        if self.paths.len() < 2 {
            "Compare".into()
        } else {
            format!(
                "Compare · {} ↔ {}",
                self.label(self.left),
                self.label(self.right)
            )
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        if area.height < 3 {
            return;
        }
        let list_w = if self.paths.len() > 2 {
            area.width.min(28).max(16)
        } else {
            0
        };
        let header = Rect::new(area.x, area.y, area.width, 1);
        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
        let footer = Rect::new(area.x, area.y + area.height - 1, area.width, 1);

        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    " ↔ Compare  A:{}  B:{} ",
                    self.label(self.left),
                    self.label(self.right)
                ),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(36, 48, 72))
                    .add_modifier(Modifier::BOLD),
            )))
            .style(Style::default().bg(Color::Rgb(36, 48, 72))),
            header,
        );

        let diff_area = if list_w > 0 {
            let list_area = Rect::new(body.x, body.y, list_w, body.height);
            let mut lines = Vec::new();
            for (i, p) in self.paths.iter().enumerate() {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string());
                let tag = if i == self.left && i == self.right {
                    "AB"
                } else if i == self.left {
                    "A "
                } else if i == self.right {
                    "B "
                } else {
                    "  "
                };
                let selected = i == self.list_sel && self.focus_list;
                let base = if selected && focused {
                    Style::default().fg(Color::White).bg(Color::Rgb(0, 96, 150))
                } else if i == self.left || i == self.right {
                    Style::default().fg(Color::Rgb(255, 220, 120))
                } else {
                    Style::default().fg(Color::Rgb(180, 185, 200))
                };
                lines.push(Line::from(Span::styled(format!(" {tag} {name}"), base)));
            }
            f.render_widget(
                Paragraph::new(lines).style(Style::default().bg(Color::Rgb(18, 22, 36))),
                list_area,
            );
            let div = Rect::new(body.x + list_w, body.y, 1, body.height);
            let div_lines: Vec<Line> = (0..body.height)
                .map(|_| {
                    Line::from(Span::styled(
                        "│",
                        Style::default().fg(Color::Rgb(60, 70, 90)),
                    ))
                })
                .collect();
            f.render_widget(
                Paragraph::new(div_lines).style(Style::default().bg(Color::Rgb(14, 18, 28))),
                div,
            );
            Rect::new(
                body.x + list_w + 1,
                body.y,
                body.width.saturating_sub(list_w + 1),
                body.height,
            )
        } else {
            body
        };

        self.view_h = diff_area.height;
        let lines = self.colored_lines();
        scroll::render_scrollable(f, diff_area, &lines, &mut self.scroll);

        let hint = if self.paths.len() > 2 {
            " ↑↓ pick · a/b set sides · Tab focus · wheel scroll · q "
        } else {
            " ↑↓/PgUp/PgDn scroll · q quit "
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" {} │{hint}", self.status),
                Style::default()
                    .fg(Color::Rgb(140, 150, 175))
                    .bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            footer,
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let _ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            KeyCode::Tab => {
                self.focus_list = !self.focus_list;
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                if !self.paths.is_empty() {
                    self.left = self.list_sel.min(self.paths.len() - 1);
                    self.recompute();
                }
            }
            KeyCode::Char('b') | KeyCode::Char('B') => {
                if !self.paths.is_empty() {
                    self.right = self.list_sel.min(self.paths.len() - 1);
                    self.recompute();
                }
            }
            KeyCode::Enter => {
                if self.focus_list && self.paths.len() > 2 {
                    // cycle: unset → A → B
                    if self.list_sel == self.left {
                        self.right = self.list_sel;
                    } else {
                        self.left = self.list_sel;
                    }
                    self.recompute();
                }
            }
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
            | KeyCode::Home | KeyCode::End
                if self.focus_list && self.paths.len() > 2 =>
            {
                let n = self.paths.len();
                match k.code {
                    KeyCode::Up => {
                        self.list_sel = self.list_sel.checked_sub(1).unwrap_or(n - 1);
                    }
                    KeyCode::Down => self.list_sel = (self.list_sel + 1) % n,
                    KeyCode::Home => self.list_sel = 0,
                    KeyCode::End => self.list_sel = n - 1,
                    KeyCode::PageUp => {
                        self.list_sel = self.list_sel.saturating_sub(8);
                    }
                    KeyCode::PageDown => {
                        self.list_sel = (self.list_sel + 8).min(n - 1);
                    }
                    _ => {}
                }
            }
            _ => {
                let lines = self.diff.len();
                let _ = self.scroll.on_key(k, lines, self.view_h);
            }
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        let list_w = if self.paths.len() > 2 {
            area.width.min(28).max(16)
        } else {
            0
        };
        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
        if list_w > 0 {
            let list_area = Rect::new(body.x, body.y, list_w, body.height);
            if matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
                && m.column >= list_area.x
                && m.column < list_area.x + list_area.width
                && m.row >= list_area.y
                && m.row < list_area.y + list_area.height
            {
                let idx = (m.row - list_area.y) as usize;
                if idx < self.paths.len() {
                    self.list_sel = idx;
                    self.focus_list = true;
                }
                return Action::None;
            }
        }
        let _ = self.scroll.on_mouse(m, self.diff.len(), self.view_h);
        Action::None
    }
}
