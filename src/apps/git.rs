//! First-class Git client backed by the system `git` CLI.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::gitutil::{self, ShortStat, StatusEntry};
use super::scroll::{self, ScrollState};
use super::{Action, AppWindow, Launch};

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Status,
    Log,
    Diff,
    Branches,
    Commit,
}

#[derive(Clone, Copy)]
enum Prompt {
    Goto,
    CommitMsg,
    NewBranch,
}

pub struct GitApp {
    root: Option<PathBuf>,
    tab: Tab,
    status: String,
    entries: Vec<StatusEntry>,
    sel: usize,
    scroll: ScrollState,
    view_h: u16,
    line_count: usize,
    log: Vec<(String, String)>,
    branches: Vec<(String, bool)>,
    diff_text: String,
    detail_lines: Vec<String>,
    prompt: Option<(Prompt, String)>,
    stat: ShortStat,
    show_help: bool,
}

impl GitApp {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut g = GitApp {
            root: None,
            tab: Tab::Status,
            status: String::new(),
            entries: Vec::new(),
            sel: 0,
            scroll: ScrollState::default(),
            view_h: 20,
            line_count: 0,
            log: Vec::new(),
            branches: Vec::new(),
            diff_text: String::new(),
            detail_lines: Vec::new(),
            prompt: None,
            stat: ShortStat::default(),
            show_help: false,
        };
        let start = path
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        g.set_root(start);
        g
    }

    fn set_root(&mut self, path: PathBuf) {
        if !gitutil::git_available() {
            self.root = None;
            self.status = "git not found on PATH".into();
            return;
        }
        match gitutil::find_root(&path) {
            Some(r) => {
                self.root = Some(r);
                self.refresh();
            }
            None => {
                self.root = None;
                self.status = format!("not a git repo: {}", path.display());
                self.entries.clear();
                self.log.clear();
                self.branches.clear();
            }
        }
    }

    fn refresh(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        gitutil::invalidate(&root);
        self.stat = gitutil::short_stat(&root);
        self.entries = gitutil::status_entries(&root);
        self.log = gitutil::log_oneline(&root, 50);
        self.branches = gitutil::branches(&root);
        self.sel = self.sel.min(self.list_len().saturating_sub(1));
        self.status = self.stat.summary();
        if self.tab == Tab::Diff && self.diff_text.is_empty() {
            self.diff_text = gitutil::diff(&root, None);
        }
    }

    fn list_len(&self) -> usize {
        match self.tab {
            Tab::Status => self.entries.len(),
            Tab::Log => self.log.len(),
            Tab::Branches => self.branches.len(),
            Tab::Diff | Tab::Commit => 0,
        }
    }

    fn open_selected(&self) -> Action {
        match self.tab {
            Tab::Status => {
                if let Some(e) = self.entries.get(self.sel) {
                    if e.path.is_file() {
                        return Action::Launch(Launch::Editor(Some(e.path.clone())));
                    }
                    if e.path.is_dir() {
                        return Action::Launch(Launch::Files(e.path.clone()));
                    }
                }
            }
            Tab::Log => {
                // detail filled on Enter in on_key
            }
            _ => {}
        }
        Action::None
    }

    fn toggle_stage(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(e) = self.entries.get(self.sel).cloned() else {
            return;
        };
        let res = if e.untracked || e.unstaged {
            gitutil::stage(&root, &e.path)
        } else if e.staged {
            gitutil::unstage(&root, &e.path)
        } else {
            gitutil::stage(&root, &e.path)
        };
        match res {
            Ok(()) => {
                self.status = "ok".into();
                self.refresh();
            }
            Err(err) => self.status = err,
        }
    }

    fn load_diff_for_sel(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if let Some(e) = self.entries.get(self.sel) {
            self.diff_text = gitutil::diff(&root, Some(&e.path));
        } else {
            self.diff_text = gitutil::diff(&root, None);
        }
        self.scroll.offset = 0;
    }

    fn tab_label(t: Tab) -> &'static str {
        match t {
            Tab::Status => "Status",
            Tab::Log => "Log",
            Tab::Diff => "Diff",
            Tab::Branches => "Branches",
            Tab::Commit => "Commit",
        }
    }

    fn content_lines(&self) -> Vec<Line<'static>> {
        let dim = Style::default().fg(Color::Rgb(130, 140, 165));
        let txt = Style::default().fg(Color::Rgb(210, 215, 230));
        let sel_bg = Color::Rgb(0, 90, 140);
        match self.tab {
            Tab::Status => {
                if self.entries.is_empty() {
                    return vec![Line::from(Span::styled("  clean working tree", dim))];
                }
                self.entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let name = e
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| e.path.display().to_string());
                        let rel = self
                            .root
                            .as_ref()
                            .and_then(|r| e.path.strip_prefix(r).ok())
                            .map(|p| p.display().to_string())
                            .unwrap_or(name);
                        let flag = format!(
                            "{}{}{}",
                            if e.staged { "S" } else { " " },
                            if e.unstaged { "W" } else { " " },
                            if e.untracked { "?" } else { " " }
                        );
                        let style = if i == self.sel {
                            Style::default()
                                .fg(Color::White)
                                .bg(sel_bg)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            txt
                        };
                        Line::from(vec![
                            Span::styled(
                                format!(" {} ", e.letter.as_char()),
                                Style::default().fg(e.letter.color()).bg(if i == self.sel {
                                    sel_bg
                                } else {
                                    Color::Reset
                                }),
                            ),
                            Span::styled(format!("{flag} {rel}"), style),
                        ])
                    })
                    .collect()
            }
            Tab::Log => {
                if !self.detail_lines.is_empty() {
                    return self
                        .detail_lines
                        .iter()
                        .map(|l| Line::from(Span::styled(l.clone(), txt)))
                        .collect();
                }
                if self.log.is_empty() {
                    return vec![Line::from(Span::styled("  no commits", dim))];
                }
                self.log
                    .iter()
                    .enumerate()
                    .map(|(i, (h, s))| {
                        let style = if i == self.sel {
                            Style::default()
                                .fg(Color::White)
                                .bg(sel_bg)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            txt
                        };
                        Line::from(vec![
                            Span::styled(
                                format!(" {h} "),
                                Style::default().fg(Color::Rgb(255, 200, 90)).bg(
                                    if i == self.sel {
                                        sel_bg
                                    } else {
                                        Color::Reset
                                    },
                                ),
                            ),
                            Span::styled(s.clone(), style),
                        ])
                    })
                    .collect()
            }
            Tab::Diff => self
                .diff_text
                .lines()
                .map(|l| {
                    let color = if l.starts_with('+') && !l.starts_with("+++") {
                        Color::Rgb(110, 220, 140)
                    } else if l.starts_with('-') && !l.starts_with("---") {
                        Color::Rgb(255, 120, 120)
                    } else if l.starts_with("@@") {
                        Color::Rgb(140, 180, 255)
                    } else {
                        Color::Rgb(180, 185, 200)
                    };
                    Line::from(Span::styled(l.to_string(), Style::default().fg(color)))
                })
                .collect(),
            Tab::Branches => {
                if self.branches.is_empty() {
                    return vec![Line::from(Span::styled("  no branches", dim))];
                }
                self.branches
                    .iter()
                    .enumerate()
                    .map(|(i, (name, cur))| {
                        let mark = if *cur { "* " } else { "  " };
                        let style = if i == self.sel {
                            Style::default()
                                .fg(Color::White)
                                .bg(sel_bg)
                                .add_modifier(Modifier::BOLD)
                        } else if *cur {
                            Style::default()
                                .fg(Color::Rgb(110, 220, 140))
                                .add_modifier(Modifier::BOLD)
                        } else {
                            txt
                        };
                        Line::from(Span::styled(format!(" {mark}{name}"), style))
                    })
                    .collect()
            }
            Tab::Commit => vec![
                Line::from(Span::styled(
                    "  Type a commit message (Ctrl+Enter or open prompt with c)",
                    dim,
                )),
                Line::default(),
                Line::from(Span::styled(
                    format!("  staged: {}  ·  {}", self.stat.staged, self.stat.summary()),
                    txt,
                )),
                Line::default(),
                Line::from(Span::styled(
                    "  c  compose · Ctrl+Enter commit staged · F5 refresh",
                    dim,
                )),
            ],
        }
    }
}

impl AppWindow for GitApp {
    fn title(&self) -> String {
        match &self.root {
            Some(r) => {
                let name = r
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| r.display().to_string());
                format!("Git · {name}")
            }
            None => "Git".into(),
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 3 {
            return;
        }
        let tabs = [Tab::Status, Tab::Log, Tab::Diff, Tab::Branches, Tab::Commit];
        let mut tab_spans = vec![Span::raw(" ")];
        for t in tabs {
            let on = t == self.tab;
            tab_spans.push(Span::styled(
                format!(" {} ", Self::tab_label(t)),
                if on {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Rgb(0, 190, 255))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Rgb(160, 170, 190))
                        .bg(Color::Rgb(28, 34, 52))
                },
            ));
            tab_spans.push(Span::raw(" "));
        }
        f.render_widget(
            Paragraph::new(Line::from(tab_spans)).style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y, area.width, 1),
        );

        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
        let lines = self.content_lines();
        self.line_count = lines.len();
        self.view_h = body.height;
        // keep selection visible for list tabs
        if matches!(self.tab, Tab::Status | Tab::Log | Tab::Branches) && self.list_len() > 0 {
            let max_off = scroll::ScrollState::max(self.line_count, self.view_h);
            if self.sel < self.scroll.offset as usize {
                self.scroll.offset = self.sel as u16;
            }
            if self.sel >= self.scroll.offset as usize + self.view_h as usize {
                self.scroll.offset = (self.sel + 1).saturating_sub(self.view_h as usize) as u16;
            }
            self.scroll.offset = self.scroll.offset.min(max_off);
        }
        scroll::render_scrollable(f, body, &lines, &mut self.scroll);

        let root_s = self
            .root
            .as_ref()
            .map(|r| r.display().to_string())
            .unwrap_or_else(|| "(no repo)".into());
        let footer = if let Some((p, input)) = &self.prompt {
            let label = match p {
                Prompt::Goto => "Repo path",
                Prompt::CommitMsg => "Commit message",
                Prompt::NewBranch => "New branch",
            };
            format!(" {label}: {input}█  Esc cancel ")
        } else {
            format!(
                " {} │ {} │ Tab panes · Space stage · Ctrl+L path · ? help · q ",
                self.status, root_s
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

        if self.show_help {
            crate::gfx::key_overlay(
                f,
                area,
                "⌨ Git · any key closes",
                &[
                    ("Tab / 1-5", "switch panes"),
                    ("↑↓ / jk", "move selection"),
                    ("Space", "stage / unstage file"),
                    ("d", "diff selected (Status)"),
                    ("Enter", "open file / details"),
                    ("c", "commit staged"),
                    ("n", "new branch (Branches)"),
                    ("Ctrl+L", "change repo path"),
                    ("F5", "refresh"),
                    ("q / Esc", "close"),
                ],
            );
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

        if self.show_help {
            self.show_help = false;
            return Action::None;
        }

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
                        Prompt::Goto => self.set_root(PathBuf::from(input.trim())),
                        Prompt::CommitMsg => {
                            if let Some(root) = self.root.clone() {
                                match gitutil::commit(&root, &input) {
                                    Ok(()) => {
                                        self.status = "committed".into();
                                        self.refresh();
                                        self.tab = Tab::Log;
                                    }
                                    Err(e) => self.status = e,
                                }
                            }
                        }
                        Prompt::NewBranch => {
                            if let Some(root) = self.root.clone() {
                                match gitutil::create_branch(&root, &input) {
                                    Ok(()) => {
                                        self.status = format!("branch {}", input.trim());
                                        self.refresh();
                                    }
                                    Err(e) => self.status = e,
                                }
                            }
                        }
                    }
                }
                _ => {
                    super::text_edit(input, k);
                }
            }
            return Action::None;
        }

        match k.code {
            KeyCode::Tab => {
                self.tab = match self.tab {
                    Tab::Status => Tab::Log,
                    Tab::Log => Tab::Diff,
                    Tab::Diff => Tab::Branches,
                    Tab::Branches => Tab::Commit,
                    Tab::Commit => Tab::Status,
                };
                self.sel = 0;
                self.scroll.offset = 0;
                self.detail_lines.clear();
                if self.tab == Tab::Diff {
                    self.load_diff_for_sel();
                }
            }
            KeyCode::BackTab => {
                self.tab = match self.tab {
                    Tab::Status => Tab::Commit,
                    Tab::Log => Tab::Status,
                    Tab::Diff => Tab::Log,
                    Tab::Branches => Tab::Diff,
                    Tab::Commit => Tab::Branches,
                };
                self.sel = 0;
                self.scroll.offset = 0;
                self.detail_lines.clear();
            }
            KeyCode::Char('1') => {
                self.tab = Tab::Status;
                self.detail_lines.clear();
            }
            KeyCode::Char('2') => {
                self.tab = Tab::Log;
                self.detail_lines.clear();
            }
            KeyCode::Char('3') => {
                self.tab = Tab::Diff;
                self.load_diff_for_sel();
            }
            KeyCode::Char('4') => self.tab = Tab::Branches,
            KeyCode::Char('5') => self.tab = Tab::Commit,
            KeyCode::F(5) => self.refresh(),
            KeyCode::Char('l') if ctrl => {
                let initial = self
                    .root
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.prompt = Some((Prompt::Goto, initial));
            }
            KeyCode::Char('c') if self.tab == Tab::Commit || (!ctrl && self.tab == Tab::Status) => {
                self.prompt = Some((Prompt::CommitMsg, String::new()));
                self.tab = Tab::Commit;
            }
            KeyCode::Enter if ctrl && self.tab == Tab::Commit => {
                self.prompt = Some((Prompt::CommitMsg, String::new()));
            }
            KeyCode::Char('n') if self.tab == Tab::Branches => {
                self.prompt = Some((Prompt::NewBranch, String::new()));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if matches!(self.tab, Tab::Diff) || !self.detail_lines.is_empty() {
                    self.scroll.by(-1, self.line_count, self.view_h);
                } else if self.list_len() > 0 {
                    self.sel = self.sel.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if matches!(self.tab, Tab::Diff) || !self.detail_lines.is_empty() {
                    self.scroll.by(1, self.line_count, self.view_h);
                } else if self.list_len() > 0 {
                    self.sel = (self.sel + 1).min(self.list_len() - 1);
                }
            }
            KeyCode::PageUp => self.scroll.by(-(self.view_h as i32), self.line_count, self.view_h),
            KeyCode::PageDown => self.scroll.by(self.view_h as i32, self.line_count, self.view_h),
            KeyCode::Char(' ') if self.tab == Tab::Status => self.toggle_stage(),
            KeyCode::Enter => match self.tab {
                Tab::Status => return self.open_selected(),
                Tab::Log => {
                    if let Some((h, _)) = self.log.get(self.sel).cloned() {
                        if let Some(root) = &self.root {
                            let text = gitutil::show_commit(root, &h);
                            self.detail_lines = text.lines().map(|l| l.to_string()).collect();
                            self.scroll.offset = 0;
                        }
                    }
                }
                Tab::Branches => {
                    if let Some((name, _)) = self.branches.get(self.sel).cloned() {
                        if let Some(root) = self.root.clone() {
                            match gitutil::checkout(&root, &name) {
                                Ok(()) => {
                                    self.status = format!("checked out {name}");
                                    self.refresh();
                                }
                                Err(e) => self.status = e,
                            }
                        }
                    }
                }
                Tab::Diff => {
                    self.load_diff_for_sel();
                }
                Tab::Commit => {
                    self.prompt = Some((Prompt::CommitMsg, String::new()));
                }
            },
            KeyCode::Esc => {
                if !self.detail_lines.is_empty() {
                    self.detail_lines.clear();
                    self.scroll.offset = 0;
                } else {
                    return Action::Close;
                }
            }
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('q') => return Action::Close,
            KeyCode::Char('d') if self.tab == Tab::Status => {
                self.tab = Tab::Diff;
                self.load_diff_for_sel();
            }
            _ => {}
        }
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        if let Some((_, input)) = &mut self.prompt {
            super::paste_line(input, text);
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        if area.height < 3 {
            return Action::None;
        }
        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if m.row == area.y => {
                // rough tab hit: Status Log Diff Branches Commit
                let rel = m.column.saturating_sub(area.x);
                self.tab = if rel < 10 {
                    Tab::Status
                } else if rel < 18 {
                    Tab::Log
                } else if rel < 26 {
                    Tab::Diff
                } else if rel < 38 {
                    Tab::Branches
                } else {
                    Tab::Commit
                };
                self.sel = 0;
                self.detail_lines.clear();
                if self.tab == Tab::Diff {
                    self.load_diff_for_sel();
                }
            }
            MouseEventKind::Down(MouseButton::Left)
                if m.row >= body.y && m.row < body.y + body.height =>
            {
                if matches!(self.tab, Tab::Status | Tab::Log | Tab::Branches)
                    && self.detail_lines.is_empty()
                {
                    let idx = self.scroll.offset as usize + (m.row - body.y) as usize;
                    if idx < self.list_len() {
                        self.sel = idx;
                    }
                }
            }
            MouseEventKind::ScrollUp => self.scroll.by(-3, self.line_count, self.view_h),
            MouseEventKind::ScrollDown => self.scroll.by(3, self.line_count, self.view_h),
            _ => {}
        }
        Action::None
    }

    fn on_tick(&mut self) {
        // light refresh of summary for status bar
        if let Some(root) = &self.root {
            self.stat = gitutil::short_stat(root);
        }
    }

    fn workspace_path(&self) -> Option<PathBuf> {
        self.root.clone()
    }
}
