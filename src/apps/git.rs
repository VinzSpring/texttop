//! First-class Git client backed by the system `git` CLI.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::gitutil::{self, ShortStat, StatusEntry};
use super::scroll::{self, ScrollState};
use super::ui::{self, Controls, Item, Look};
use super::{Action, AppWindow, Launch};

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    Tab(Tab),
    Refresh,
    Repo,
    ToggleStage,
    /// The checkbox on a Status row.
    StageRow(usize),
    StageAll,
    DiffSel,
    DiffAll,
    Open,
    Commit,
    ShowCommit,
    BackToLog,
    Checkout,
    NewBranch,
    PromptOk,
    PromptCancel,
}

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
    ui: Controls<Cmd>,
    last_click: Option<(std::time::Instant, usize)>,
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
            ui: Controls::new(),
            last_click: None,
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
                        // the box itself is a button drawn over this cell
                        let flag = "    ";
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
                Line::default(),
                Line::from(Span::styled(
                    format!("  staged: {}  ·  {}", self.stat.staged, self.stat.summary()),
                    txt,
                )),
                Line::default(),
                Line::from(Span::styled(
                    if self.stat.staged == 0 {
                        "  Nothing staged yet — tick files in Status, or use Stage all."
                    } else {
                        "  Write a message below and click Commit."
                    },
                    dim,
                )),
            ],
        }
    }
}

impl GitApp {
    fn switch(&mut self, t: Tab) {
        self.tab = t;
        self.sel = 0;
        self.scroll.offset = 0;
        self.detail_lines.clear();
        match t {
            Tab::Diff => self.load_diff_for_sel(),
            Tab::Commit => self.begin_commit(),
            _ => {}
        }
    }

    fn begin_commit(&mut self) {
        if !matches!(self.prompt, Some((Prompt::CommitMsg, _))) {
            self.prompt = Some((Prompt::CommitMsg, String::new()));
        }
    }

    fn finish_prompt(&mut self) {
        let Some((prompt, input)) = self.prompt.take() else {
            return;
        };
        match prompt {
            Prompt::Goto => self.set_root(PathBuf::from(input.trim())),
            Prompt::CommitMsg => {
                if let Some(root) = self.root.clone() {
                    match gitutil::commit(&root, &input) {
                        Ok(()) => {
                            self.status = "committed".into();
                            self.refresh();
                            self.tab = Tab::Log;
                            self.sel = 0;
                        }
                        Err(e) => {
                            self.status = e;
                            // keep the message so a fix does not mean retyping
                            self.prompt = Some((Prompt::CommitMsg, input));
                        }
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

    fn show_commit(&mut self) {
        if let Some((h, _)) = self.log.get(self.sel).cloned() {
            if let Some(root) = &self.root {
                let text = gitutil::show_commit(root, &h);
                self.detail_lines = text.lines().map(|l| l.to_string()).collect();
                self.scroll.offset = 0;
            }
        }
    }

    fn checkout(&mut self) {
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

    fn run(&mut self, c: Cmd) -> Action {
        match c {
            Cmd::Tab(t) => self.switch(t),
            Cmd::Refresh => self.refresh(),
            Cmd::Repo => {
                let initial = self
                    .root
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.prompt = Some((Prompt::Goto, initial));
            }
            Cmd::ToggleStage => self.toggle_stage(),
            Cmd::StageRow(i) => {
                self.sel = i;
                self.toggle_stage();
            }
            Cmd::StageAll => {
                if let Some(root) = self.root.clone() {
                    match gitutil::stage_all(&root) {
                        Ok(()) => {
                            self.status = "staged everything".into();
                            self.refresh();
                        }
                        Err(e) => self.status = e,
                    }
                }
            }
            Cmd::DiffSel => {
                self.tab = Tab::Diff;
                self.load_diff_for_sel();
            }
            Cmd::DiffAll => {
                if let Some(root) = self.root.clone() {
                    self.diff_text = gitutil::diff(&root, None);
                    self.scroll.offset = 0;
                }
            }
            Cmd::Open => return self.open_selected(),
            Cmd::Commit => {
                self.tab = Tab::Commit;
                self.begin_commit();
            }
            Cmd::ShowCommit => self.show_commit(),
            Cmd::BackToLog => {
                self.detail_lines.clear();
                self.scroll.offset = 0;
            }
            Cmd::Checkout => self.checkout(),
            Cmd::NewBranch => self.prompt = Some((Prompt::NewBranch, String::new())),
            Cmd::PromptOk => self.finish_prompt(),
            Cmd::PromptCancel => {
                self.prompt = None;
                self.status = "cancelled".into();
            }
        }
        Action::None
    }

    fn body(area: Rect) -> Rect {
        Rect::new(area.x, area.y + 2, area.width, area.height.saturating_sub(3))
    }

    /// The second row: what can be done in the current tab.
    fn action_items(&self) -> (Vec<Item<Cmd>>, Vec<Item<Cmd>>) {
        let repo = self.root.is_some();
        let has = |b: bool| Look::enabled_if(repo && b);
        match self.tab {
            Tab::Status => {
                let e = self.entries.get(self.sel);
                let stage_label = match e {
                    Some(e) if e.staged && !e.unstaged && !e.untracked => "− Unstage",
                    _ => "+ Stage",
                };
                (
                    vec![
                        Item::with(stage_label, Cmd::ToggleStage, has(e.is_some())),
                        Item::with("+ Stage all", Cmd::StageAll, has(!self.entries.is_empty())),
                        Item::Sep,
                        Item::with("Diff", Cmd::DiffSel, has(e.is_some())),
                        Item::with("Open", Cmd::Open, has(e.is_some())),
                    ],
                    vec![Item::with("✔ Commit…", Cmd::Commit, if repo && self.stat.staged > 0 { Look::Primary } else { Look::Disabled })],
                )
            }
            Tab::Log if !self.detail_lines.is_empty() => {
                (vec![Item::btn("◀ Back to log", Cmd::BackToLog)], vec![])
            }
            Tab::Log => (
                vec![Item::with("Show commit", Cmd::ShowCommit, has(!self.log.is_empty()))],
                vec![],
            ),
            Tab::Diff => (vec![Item::with("All changes", Cmd::DiffAll, has(true))], vec![]),
            Tab::Branches => {
                let cur = self.branches.get(self.sel).is_some_and(|(_, c)| *c);
                (
                    vec![
                        Item::with("Checkout", Cmd::Checkout, has(!self.branches.is_empty() && !cur)),
                        Item::with("+ New branch…", Cmd::NewBranch, has(true)),
                    ],
                    vec![],
                )
            }
            Tab::Commit => (
                vec![Item::with("+ Stage all", Cmd::StageAll, has(!self.entries.is_empty()))],
                vec![],
            ),
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
        if area.height < 4 {
            return;
        }
        self.ui.begin();
        let tabs = [Tab::Status, Tab::Log, Tab::Diff, Tab::Branches, Tab::Commit];
        let left: Vec<Item<Cmd>> = tabs
            .iter()
            .map(|&t| Item::with(Self::tab_label(t), Cmd::Tab(t), Look::on_if(t == self.tab)))
            .collect();
        self.ui.toolbar(
            f.buffer_mut(),
            Rect::new(area.x, area.y, area.width, 1),
            &left,
            &[Item::btn("⟳", Cmd::Refresh), Item::btn("📂 Repo…", Cmd::Repo)],
        );
        let (l, r) = self.action_items();
        self.ui.toolbar(
            f.buffer_mut(),
            Rect::new(area.x, area.y + 1, area.width, 1),
            &l,
            &r,
        );

        let body = Self::body(area);
        let lines = self.content_lines();
        self.line_count = lines.len();
        self.view_h = body.height;
        // keep selection visible for list tabs
        if matches!(self.tab, Tab::Status | Tab::Log | Tab::Branches)
            && self.list_len() > 0
            && self.detail_lines.is_empty()
        {
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

        // stage checkboxes on Status rows
        if self.tab == Tab::Status {
            let buf = f.buffer_mut();
            let first = self.scroll.offset as usize;
            for (row, i) in (first..self.entries.len()).enumerate().take(body.height as usize) {
                let e = &self.entries[i];
                let mark = if e.staged && !e.unstaged && !e.untracked {
                    "☑"
                } else if e.staged {
                    "◩"
                } else {
                    "☐"
                };
                let r = Rect::new(body.x + 3, body.y + row as u16, 3, 1);
                let hot = self.ui.hot(r);
                let style = Style::default()
                    .fg(if e.staged { Color::Rgb(110, 220, 140) } else { Color::Rgb(170, 178, 200) })
                    .bg(if hot { ui::HOVER_BG } else if i == self.sel { Color::Rgb(0, 90, 140) } else { Color::Reset });
                buf.set_string(r.x, r.y, format!(" {mark} "), style);
                self.ui.add(r, Cmd::StageRow(i));
            }
        }

        let footer = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        let buf = f.buffer_mut();
        if let Some((p, input)) = &self.prompt {
            let (label, ok) = match p {
                Prompt::Goto => ("Repository folder", "Open"),
                Prompt::CommitMsg => ("Commit message", "✔ Commit"),
                Prompt::NewBranch => ("New branch", "Create"),
            };
            self.ui
                .input_row(buf, footer, label, input, (ok, Cmd::PromptOk), Cmd::PromptCancel);
        } else {
            let root_s = self
                .root
                .as_ref()
                .map(|r| r.display().to_string())
                .unwrap_or_else(|| "(no repo — click 📂 Repo…)".into());
            ui::status_bar(buf, footer, &format!("{} │ {}", self.status, root_s));
        }
        self.ui.render_menu(f.buffer_mut(), area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.ui.menu_open() && k.code == KeyCode::Esc {
            self.ui.close_menu();
            return Action::None;
        }

        if let Some((_, ref mut input)) = self.prompt.as_mut() {
            match k.code {
                KeyCode::Esc => return self.run(Cmd::PromptCancel),
                KeyCode::Enter => self.finish_prompt(),
                _ => {
                    super::text_edit(input, k);
                }
            }
            return Action::None;
        }

        match k.code {
            KeyCode::Tab | KeyCode::BackTab => {
                let order = [Tab::Status, Tab::Log, Tab::Diff, Tab::Branches, Tab::Commit];
                let i = order.iter().position(|t| *t == self.tab).unwrap_or(0);
                let next = if k.code == KeyCode::Tab { (i + 1) % 5 } else { (i + 4) % 5 };
                self.switch(order[next]);
            }
            KeyCode::F(5) => self.refresh(),
            KeyCode::Char('l') if ctrl => return self.run(Cmd::Repo),
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
                Tab::Log => self.show_commit(),
                Tab::Branches => self.checkout(),
                Tab::Diff => self.load_diff_for_sel(),
                Tab::Commit => self.begin_commit(),
            },
            KeyCode::Esc => {
                if !self.detail_lines.is_empty() {
                    self.detail_lines.clear();
                    self.scroll.offset = 0;
                } else {
                    return Action::Close;
                }
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
        if area.height < 4 {
            return Action::None;
        }
        if let Some(hit) = self.ui.route(&m) {
            return hit.map_or(Action::None, |c| self.run(c));
        }
        if !matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown)
            && self.scroll.on_mouse(m, self.line_count, self.view_h)
        {
            if self.list_len() > 0 && self.detail_lines.is_empty() {
                let top = self.scroll.offset as usize;
                let bottom = (top + self.view_h as usize).saturating_sub(1);
                self.sel = self.sel.clamp(top, bottom.max(top)).min(self.list_len() - 1);
            }
            return Action::None;
        }
        let body = Self::body(area);
        match m.kind {
            MouseEventKind::Down(button)
                if m.row >= body.y && m.row < body.y + body.height =>
            {
                if matches!(self.tab, Tab::Status | Tab::Log | Tab::Branches)
                    && self.detail_lines.is_empty()
                {
                    let idx = self.scroll.offset as usize + (m.row - body.y) as usize;
                    if idx >= self.list_len() {
                        return Action::None;
                    }
                    self.sel = idx;
                    if button == MouseButton::Right {
                        let items = match self.tab {
                            Tab::Status => {
                                let staged = self.entries[idx].staged;
                                vec![
                                    (if staged { "− Unstage" } else { "+ Stage" }.into(), Some(Cmd::ToggleStage)),
                                    ("Diff".into(), Some(Cmd::DiffSel)),
                                    ("Open".into(), Some(Cmd::Open)),
                                ]
                            }
                            Tab::Log => vec![("Show commit".into(), Some(Cmd::ShowCommit))],
                            _ => vec![
                                ("Checkout".into(), Some(Cmd::Checkout)),
                                ("+ New branch…".into(), Some(Cmd::NewBranch)),
                            ],
                        };
                        self.ui.open_menu(m.column, m.row, items);
                        return Action::None;
                    }
                    // double-click: the tab's main action
                    let now = std::time::Instant::now();
                    let dbl = matches!(self.last_click, Some((t, li))
                        if li == idx && now.duration_since(t).as_millis() < 450);
                    self.last_click = if dbl { None } else { Some((now, idx)) };
                    if dbl {
                        return match self.tab {
                            Tab::Status => self.open_selected(),
                            Tab::Log => self.run(Cmd::ShowCommit),
                            _ => self.run(Cmd::Checkout),
                        };
                    }
                }
            }
            // lists keep their selection in view, so the wheel moves it
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if self.list_len() > 0 && self.detail_lines.is_empty() =>
            {
                self.sel = if m.kind == MouseEventKind::ScrollUp {
                    self.sel.saturating_sub(3)
                } else {
                    (self.sel + 3).min(self.list_len() - 1)
                };
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
