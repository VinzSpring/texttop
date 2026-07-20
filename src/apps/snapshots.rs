//! Per-file snapshot history: view, diff, restore.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::scroll::{self, ScrollState};
use super::snaputil::{self, DiffKind, DiffLine, SnapMeta};
use super::{Action, AppWindow, Launch};

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    History,
    Diff,
    Preview,
}

#[derive(Clone, Copy)]
enum Prompt {
    RestoreConfirm,
    Note,
}

pub struct SnapshotsApp {
    path: PathBuf,
    snaps: Vec<SnapMeta>,
    sel: usize,
    mark: Option<usize>,
    tab: Tab,
    diff: Vec<DiffLine>,
    preview: String,
    scroll: ScrollState,
    status: String,
    prompt: Option<(Prompt, String)>,
    view_h: u16,
}

impl SnapshotsApp {
    pub fn new(path: PathBuf) -> Self {
        let mut app = SnapshotsApp {
            path,
            snaps: Vec::new(),
            sel: 0,
            mark: None,
            tab: Tab::History,
            diff: Vec::new(),
            preview: String::new(),
            scroll: ScrollState::default(),
            status: String::new(),
            prompt: None,
            view_h: 20,
        };
        app.reload();
        app
    }

    fn reload(&mut self) {
        self.snaps = snaputil::history(&self.path);
        if self.sel >= self.snaps.len() {
            self.sel = self.snaps.len().saturating_sub(1);
        }
        if let Some(m) = self.mark {
            if m >= self.snaps.len() {
                self.mark = None;
            }
        }
        self.status = if self.snaps.is_empty() {
            "no snapshots yet · press n / Create from Files".into()
        } else {
            format!("{} snapshot(s)", self.snaps.len())
        };
    }

    fn selected_id(&self) -> Option<&str> {
        self.snaps.get(self.sel).map(|s| s.id.as_str())
    }

    fn load_preview(&mut self) {
        self.preview.clear();
        let Some(id) = self.selected_id().map(|s| s.to_string()) else {
            self.preview = "(nothing selected)".into();
            return;
        };
        match snaputil::read_blob_text(&id) {
            Ok(t) => self.preview = t,
            Err(e) => self.preview = e,
        }
        self.scroll.offset = 0;
    }

    fn load_diff(&mut self) {
        self.diff.clear();
        let Some(cur) = self.snaps.get(self.sel).cloned() else {
            self.diff.push(DiffLine {
                kind: DiffKind::Header,
                text: "no snapshot selected".into(),
            });
            return;
        };
        let a_text = match snaputil::read_blob_text(&cur.id) {
            Ok(t) => t,
            Err(e) => {
                self.diff.push(DiffLine {
                    kind: DiffKind::Header,
                    text: e,
                });
                return;
            }
        };
        let (b_label, b_text) = if let Some(mi) = self.mark.filter(|&i| i != self.sel) {
            let other = &self.snaps[mi];
            match snaputil::read_blob_text(&other.id) {
                Ok(t) => (format!("snap {}", other.when_local()), t),
                Err(e) => {
                    self.diff.push(DiffLine {
                        kind: DiffKind::Header,
                        text: e,
                    });
                    return;
                }
            }
        } else {
            match snaputil::read_path_text(&self.path) {
                Ok(t) => ("working copy".into(), t),
                Err(e) => {
                    self.diff.push(DiffLine {
                        kind: DiffKind::Header,
                        text: format!("working copy: {e}"),
                    });
                    return;
                }
            }
        };
        self.diff = snaputil::diff_texts(
            &format!("snap {}", cur.when_local()),
            &a_text,
            &b_label,
            &b_text,
        );
        self.scroll.offset = 0;
        self.status = format!("diff · {} lines", self.diff.len());
    }

    fn create_now(&mut self, note: &str) {
        match snaputil::create_snapshot(&self.path, note) {
            Ok(m) => {
                self.status = format!("created {}", m.id);
                self.reload();
            }
            Err(e) => self.status = e,
        }
    }

    fn colored_diff(&self) -> Vec<Line<'static>> {
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

impl AppWindow for SnapshotsApp {
    fn title(&self) -> String {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string());
        format!("Snapshots · {name}")
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        if area.height < 3 {
            return;
        }
        let tabs = match self.tab {
            Tab::History => " [History]  Diff  Preview ",
            Tab::Diff => " History  [Diff]  Preview ",
            Tab::Preview => " History  Diff  [Preview] ",
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" ⏱ {tabs}"),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(40, 36, 60))
                    .add_modifier(Modifier::BOLD),
            )))
            .style(Style::default().bg(Color::Rgb(40, 36, 60))),
            Rect::new(area.x, area.y, area.width, 1),
        );

        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
        self.view_h = body.height;

        match self.tab {
            Tab::History => {
                let mut lines = Vec::new();
                if self.snaps.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "  No snapshots for this file.",
                        Style::default().fg(Color::Rgb(150, 155, 175)),
                    )));
                    lines.push(Line::from(Span::styled(
                        "  Press n to snapshot the working copy.",
                        Style::default().fg(Color::Rgb(150, 155, 175)),
                    )));
                }
                for (i, s) in self.snaps.iter().enumerate() {
                    let mark = if Some(i) == self.mark { "●" } else { " " };
                    let selected = i == self.sel;
                    let base = if selected && focused {
                        Style::default().fg(Color::White).bg(Color::Rgb(0, 96, 150))
                    } else if selected {
                        Style::default().fg(Color::White).bg(Color::Rgb(48, 52, 74))
                    } else {
                        Style::default().fg(Color::Rgb(200, 205, 220))
                    };
                    let note = if s.note.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", s.note)
                    };
                    lines.push(Line::from(Span::styled(
                        format!(
                            " {mark} {}  {:>8}  {}{}",
                            s.when_local(),
                            crate::gfx::fmt_size(s.size),
                            &s.id[s.id.len().saturating_sub(12)..],
                            note
                        ),
                        base,
                    )));
                }
                f.render_widget(Paragraph::new(lines), body);
            }
            Tab::Diff => {
                let lines = self.colored_diff();
                scroll::render_scrollable(f, body, &lines, &mut self.scroll);
            }
            Tab::Preview => {
                let lines: Vec<Line> = self
                    .preview
                    .lines()
                    .map(|l| {
                        Line::from(Span::styled(
                            l.to_string(),
                            Style::default().fg(Color::Rgb(200, 205, 220)),
                        ))
                    })
                    .collect();
                scroll::render_scrollable(f, body, &lines, &mut self.scroll);
            }
        }

        let footer = if let Some((p, input)) = &self.prompt {
            let label = match p {
                Prompt::RestoreConfirm => "Restore? type y",
                Prompt::Note => "Snapshot note",
            };
            format!(" {label}: {input}█  Esc cancel ")
        } else {
            format!(
                " {} │ Tab panes · n new · Space mark · d diff · r restore · Enter edit · q ",
                self.status
            )
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                footer,
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

        if let Some((prompt, ref mut input)) = self.prompt.as_mut() {
            let prompt = *prompt;
            match k.code {
                KeyCode::Esc => {
                    self.prompt = None;
                    self.status = "cancelled".into();
                }
                KeyCode::Enter => {
                    let input = input.clone();
                    self.prompt = None;
                    match prompt {
                        Prompt::RestoreConfirm => {
                            if input.trim().eq_ignore_ascii_case("y") {
                                if let Some(id) = self.selected_id().map(|s| s.to_string()) {
                                    match snaputil::restore(&self.path, &id) {
                                        Ok(()) => {
                                            self.status = "restored (pre-restore snap saved)".into();
                                            self.reload();
                                        }
                                        Err(e) => self.status = e,
                                    }
                                }
                            } else {
                                self.status = "restore cancelled".into();
                            }
                        }
                        Prompt::Note => self.create_now(&input),
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if !ctrl => input.push(c),
                _ => {}
            }
            return Action::None;
        }

        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            KeyCode::Tab => {
                self.tab = match self.tab {
                    Tab::History => {
                        self.load_diff();
                        Tab::Diff
                    }
                    Tab::Diff => {
                        self.load_preview();
                        Tab::Preview
                    }
                    Tab::Preview => Tab::History,
                };
                self.scroll.offset = 0;
            }
            KeyCode::Char('1') => self.tab = Tab::History,
            KeyCode::Char('2') => {
                self.load_diff();
                self.tab = Tab::Diff;
            }
            KeyCode::Char('3') => {
                self.load_preview();
                self.tab = Tab::Preview;
            }
            KeyCode::Char('n') if !ctrl => {
                self.prompt = Some((Prompt::Note, String::new()));
            }
            KeyCode::Char(' ') if self.tab == Tab::History => {
                if self.mark == Some(self.sel) {
                    self.mark = None;
                    self.status = "unmarked".into();
                } else {
                    self.mark = Some(self.sel);
                    self.status = "marked for version diff (d)".into();
                }
            }
            KeyCode::Char('d') if !ctrl => {
                self.load_diff();
                self.tab = Tab::Diff;
            }
            KeyCode::Char('r') if !ctrl => {
                if self.snaps.is_empty() {
                    self.status = "nothing to restore".into();
                } else {
                    self.prompt = Some((Prompt::RestoreConfirm, String::new()));
                    self.status = "restore overwrites working file".into();
                }
            }
            KeyCode::F(5) => self.reload(),
            KeyCode::Enter if self.tab == Tab::History => {
                return Action::Launch(Launch::Editor(Some(self.path.clone())));
            }
            KeyCode::Up if self.tab == Tab::History => {
                if !self.snaps.is_empty() {
                    self.sel = self.sel.checked_sub(1).unwrap_or(self.snaps.len() - 1);
                }
            }
            KeyCode::Down if self.tab == Tab::History => {
                if !self.snaps.is_empty() {
                    self.sel = (self.sel + 1) % self.snaps.len();
                }
            }
            _ => {
                let len = match self.tab {
                    Tab::History => self.snaps.len(),
                    Tab::Diff => self.diff.len(),
                    Tab::Preview => self.preview.lines().count(),
                };
                if self.tab != Tab::History {
                    let _ = self.scroll.on_key(k, len, self.view_h);
                } else {
                    match k.code {
                        KeyCode::PageUp => {
                            self.sel = self.sel.saturating_sub(10);
                        }
                        KeyCode::PageDown => {
                            if !self.snaps.is_empty() {
                                self.sel = (self.sel + 10).min(self.snaps.len() - 1);
                            }
                        }
                        KeyCode::Home => self.sel = 0,
                        KeyCode::End => {
                            self.sel = self.snaps.len().saturating_sub(1);
                        }
                        _ => {}
                    }
                }
            }
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
        if self.tab == Tab::History
            && matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
            && m.row >= body.y
            && m.row < body.y + body.height
        {
            let idx = (m.row - body.y) as usize;
            if idx < self.snaps.len() {
                self.sel = idx;
            }
            return Action::None;
        }
        let len = match self.tab {
            Tab::Diff => self.diff.len(),
            Tab::Preview => self.preview.lines().count(),
            Tab::History => 0,
        };
        if len > 0 {
            let _ = self.scroll.on_mouse(m, len, self.view_h);
        }
        Action::None
    }
}
