//! Project search — uses `rg` when available, else a simple recursive walk.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::scroll::{self, ScrollState};
use super::ui::{self, Controls, Item, Look};
use super::{Action, AppWindow, Launch};

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    Query,
    Search,
    ChangeRoot,
    ParentRoot,
    RootOk,
    RootCancel,
    Open,
    OpenFolder,
    CopyPath,
}

struct Hit {
    path: PathBuf,
    line: usize,
    text: String,
}

pub struct SearchApp {
    root: PathBuf,
    query: String,
    editing: bool,
    /// When set, the top line edits the search root instead of the query.
    root_input: Option<String>,
    hits: Vec<Hit>,
    sel: usize,
    scroll: ScrollState,
    view_h: u16,
    status: String,
    use_rg: bool,
    last_click: Option<(std::time::Instant, usize)>,
    ui: Controls<Cmd>,
}

impl SearchApp {
    pub fn new(root: Option<PathBuf>) -> Self {
        let root = root
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let use_rg = Command::new("rg")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        SearchApp {
            root,
            query: String::new(),
            editing: true,
            root_input: None,
            hits: Vec::new(),
            sel: 0,
            scroll: ScrollState::default(),
            view_h: 20,
            status: if use_rg {
                "type what to look for, then Search (uses ripgrep)".into()
            } else {
                "type what to look for, then Search".into()
            },
            use_rg,
            last_click: None,
            ui: Controls::new(),
        }
    }

    fn commit_root(&mut self) {
        let Some(input) = self.root_input.take() else {
            return;
        };
        let p = PathBuf::from(input.trim());
        if p.is_dir() {
            self.root = p;
            self.status = format!("root: {}", self.root.display());
            if !self.query.trim().is_empty() {
                self.run_search();
            }
        } else {
            self.status = format!("not a folder: {}", p.display());
        }
    }

    fn run(&mut self, c: Cmd) -> Action {
        let hit = self.hits.get(self.sel).map(|h| h.path.clone());
        match c {
            Cmd::Query => {
                self.root_input = None;
                self.editing = true;
            }
            Cmd::Search => self.run_search(),
            Cmd::ChangeRoot => {
                self.root_input = Some(self.root.to_string_lossy().to_string());
            }
            Cmd::ParentRoot => {
                if let Some(p) = self.root.parent() {
                    self.root_input = Some(p.to_string_lossy().to_string());
                    self.commit_root();
                }
            }
            Cmd::RootOk => self.commit_root(),
            Cmd::RootCancel => self.root_input = None,
            Cmd::Open => {
                if let Some(p) = hit {
                    return Action::Launch(Launch::Editor(Some(p)));
                }
            }
            Cmd::OpenFolder => {
                if let Some(dir) = hit.as_ref().and_then(|p| p.parent()) {
                    return Action::Launch(Launch::Files(dir.to_path_buf()));
                }
            }
            Cmd::CopyPath => {
                if let Some(p) = hit {
                    crate::clipboard::copy(&p.to_string_lossy());
                    self.status = "path copied".into();
                }
            }
        }
        Action::None
    }

    fn body(area: Rect) -> Rect {
        Rect::new(area.x, area.y + 2, area.width, area.height.saturating_sub(3))
    }

    fn run_search(&mut self) {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            self.status = "empty query".into();
            return;
        }
        self.hits.clear();
        self.sel = 0;
        self.scroll.offset = 0;
        if self.use_rg {
            self.search_rg(&q);
        } else {
            self.search_walk(&q);
        }
        self.status = format!("{} hits in {}", self.hits.len(), self.root.display());
        self.editing = false;
    }

    fn search_rg(&mut self, q: &str) {
        let mut cmd = Command::new("rg");
        cmd.args([
            "--line-number",
            "--no-heading",
            "--color",
            "never",
            "--max-count",
            "5",
            "-m",
            "200",
            q,
        ])
        .current_dir(&self.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let Ok(out) = cmd.output() else {
            self.status = "rg failed".into();
            return;
        };
        for line in String::from_utf8_lossy(&out.stdout).lines().take(400) {
            // path:line:text
            let mut parts = line.splitn(3, ':');
            let path = parts.next().unwrap_or("").to_string();
            let ln: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let text = parts.next().unwrap_or("").to_string();
            if path.is_empty() {
                continue;
            }
            self.hits.push(Hit {
                path: self.root.join(path),
                line: ln,
                text,
            });
        }
    }

    fn search_walk(&mut self, q: &str) {
        let q_lower = q.to_lowercase();
        let mut stack = vec![self.root.clone()];
        let mut files = 0usize;
        while let Some(dir) = stack.pop() {
            let Ok(rd) = fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let path = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                files += 1;
                if files > 4000 || self.hits.len() >= 400 {
                    return;
                }
                if !is_textish(&path) {
                    continue;
                }
                let Ok(f) = fs::File::open(&path) else {
                    continue;
                };
                let reader = BufReader::new(f);
                for (i, line) in reader.lines().enumerate().take(2000) {
                    let Ok(line) = line else { break };
                    if line.to_lowercase().contains(&q_lower) {
                        self.hits.push(Hit {
                            path: path.clone(),
                            line: i + 1,
                            text: line.chars().take(120).collect(),
                        });
                        if self.hits.len() >= 400 {
                            return;
                        }
                        break; // one hit per file for walk mode
                    }
                }
            }
        }
    }
}

fn is_textish(p: &Path) -> bool {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "rs" | "toml" | "md" | "txt" | "py" | "js" | "ts" | "tsx" | "jsx" | "json"
            | "yaml" | "yml" | "html" | "css" | "c" | "cpp" | "h" | "go" | "java"
            | "sh" | "bat" | "ps1" | "sql" | "xml" | "ini" | "cfg" | ""
    ) || ext.is_empty()
}

impl AppWindow for SearchApp {
    fn title(&self) -> String {
        "Search".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 3 {
            return;
        }
        self.ui.begin();
        let buf = f.buffer_mut();
        // row 0: the query field and its button
        let field = self.ui.toolbar(
            buf,
            Rect::new(area.x, area.y, area.width, 1),
            &[],
            &[
                Item::with("🔍 Search", Cmd::Search, Look::enabled_if(!self.query.trim().is_empty())),
                Item::with("Open", Cmd::Open, Look::enabled_if(!self.hits.is_empty())),
            ],
        );
        let fbg = if self.editing { Color::Rgb(12, 16, 30) } else { Color::Rgb(18, 22, 38) };
        let fr = Rect::new(field.x + 1, field.y, field.width.saturating_sub(2), 1);
        ui::fill_row(buf, fr, fbg);
        let text = if self.query.is_empty() && !self.editing {
            Span::styled(" find in files…", Style::default().fg(ui::DIM).bg(fbg))
        } else {
            let caret = if self.editing { "▏" } else { "" };
            Span::styled(format!(" {}{caret}", self.query), Style::default().fg(Color::White).bg(fbg))
        };
        let shown: String = text.content.chars().take(fr.width as usize).collect();
        buf.set_string(fr.x, fr.y, shown, text.style);
        self.ui.add(fr, Cmd::Query);

        // row 1: where to search
        let root_row = Rect::new(area.x, area.y + 1, area.width, 1);
        if let Some(input) = &self.root_input {
            self.ui
                .input_row(buf, root_row, "Search in", input, ("Set", Cmd::RootOk), Cmd::RootCancel);
        } else {
            let room = area.width.saturating_sub(24) as usize;
            let root = self.root.display().to_string();
            let root = if root.chars().count() > room {
                let skip = root.chars().count() - room.saturating_sub(1);
                format!("…{}", root.chars().skip(skip).collect::<String>())
            } else {
                root
            };
            self.ui.toolbar(
                buf,
                root_row,
                &[
                    Item::Label(format!(" in 📁 {root}"), Color::Rgb(170, 180, 205)),
                ],
                &[
                    Item::btn("⬆", Cmd::ParentRoot),
                    Item::btn("Change…", Cmd::ChangeRoot),
                ],
            );
        }

        let body = Self::body(area);
        let sel_bg = Color::Rgb(0, 90, 140);
        let lines: Vec<Line> = if self.hits.is_empty() {
            vec![Line::from(Span::styled(
                "  no results",
                Style::default().fg(Color::Rgb(130, 140, 165)),
            ))]
        } else {
            self.hits
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let rel = h
                        .path
                        .strip_prefix(&self.root)
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| h.path.display().to_string());
                    let style = if i == self.sel {
                        Style::default()
                            .fg(Color::White)
                            .bg(sel_bg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Rgb(210, 215, 230))
                    };
                    Line::from(Span::styled(
                        format!(" {rel}:{}  {}", h.line, h.text),
                        style,
                    ))
                })
                .collect()
        };
        let n = lines.len();
        self.view_h = body.height;
        if self.sel < self.scroll.offset as usize {
            self.scroll.offset = self.sel as u16;
        }
        if self.sel >= self.scroll.offset as usize + self.view_h as usize {
            self.scroll.offset = (self.sel + 1).saturating_sub(self.view_h as usize) as u16;
        }
        scroll::render_scrollable(f, body, &lines, &mut self.scroll);
        let _ = n;

        ui::status_bar(
            f.buffer_mut(),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
            &self.status,
        );
        self.ui.render_menu(f.buffer_mut(), area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.ui.menu_open() && k.code == KeyCode::Esc {
            self.ui.close_menu();
            return Action::None;
        }
        // root path editing takes over the input line
        if self.root_input.is_some() {
            match k.code {
                KeyCode::Esc => {
                    self.root_input = None;
                    self.status = "root edit cancelled".into();
                }
                KeyCode::Enter => self.commit_root(),
                _ => {
                    if let Some(s) = &mut self.root_input {
                        super::text_edit(s, k);
                    }
                }
            }
            return Action::None;
        }
        if self.editing {
            match k.code {
                KeyCode::Esc => {
                    if self.hits.is_empty() {
                        return Action::Close;
                    }
                    self.editing = false;
                }
                KeyCode::Enter => self.run_search(),
                KeyCode::Char('l') if ctrl => {
                    self.editing = false;
                    self.root_input = Some(self.root.to_string_lossy().to_string());
                }
                _ => {
                    super::text_edit(&mut self.query, k);
                }
            }
            return Action::None;
        }
        match k.code {
            KeyCode::Char('/') => {
                self.editing = true;
            }
            KeyCode::Char('f') if ctrl => {
                self.editing = true;
            }
            KeyCode::Char('l') if ctrl => {
                self.root_input = Some(self.root.to_string_lossy().to_string());
            }
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => {
                if !self.hits.is_empty() {
                    self.sel = (self.sel + 1).min(self.hits.len() - 1);
                }
            }
            KeyCode::Enter => {
                if let Some(h) = self.hits.get(self.sel) {
                    return Action::Launch(Launch::Editor(Some(h.path.clone())));
                }
            }
            KeyCode::F(5) => self.run_search(),
            KeyCode::Esc => return Action::Close,
            // typing goes to the search field, like any search box
            _ => {
                if super::text_edit(&mut self.query, k) {
                    self.editing = true;
                }
            }
        }
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        if let Some(root) = &mut self.root_input {
            super::paste_line(root, text);
        } else if self.editing {
            super::paste_line(&mut self.query, text);
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        if let Some(hit) = self.ui.route(&m) {
            return hit.map_or(Action::None, |c| self.run(c));
        }
        if !matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown)
            && self.scroll.on_mouse(m, self.hits.len(), self.view_h)
        {
            let top = self.scroll.offset as usize;
            let bottom = (top + self.view_h as usize).saturating_sub(1);
            self.sel = self.sel.clamp(top, bottom.max(top)).min(self.hits.len().saturating_sub(1));
            return Action::None;
        }
        let body = Self::body(area);
        match m.kind {
            MouseEventKind::Down(MouseButton::Right)
                if m.row >= body.y && m.row < body.y + body.height =>
            {
                let idx = self.scroll.offset as usize + (m.row - body.y) as usize;
                if idx < self.hits.len() {
                    self.sel = idx;
                    self.ui.open_menu(
                        m.column,
                        m.row,
                        vec![
                            ("Open in editor".into(), Some(Cmd::Open)),
                            ("Show in Files".into(), Some(Cmd::OpenFolder)),
                            (String::new(), None),
                            ("Copy path".into(), Some(Cmd::CopyPath)),
                        ],
                    );
                }
            }
            MouseEventKind::Down(MouseButton::Left)
                if m.row >= body.y && m.row < body.y + body.height =>
            {
                let idx = self.scroll.offset as usize + (m.row - body.y) as usize;
                if idx < self.hits.len() {
                    self.sel = idx;
                    self.editing = false;
                    let now = std::time::Instant::now();
                    let dbl = matches!(self.last_click, Some((t, li))
                        if li == idx && now.duration_since(t).as_millis() < 450);
                    if dbl {
                        self.last_click = None;
                        if let Some(h) = self.hits.get(idx) {
                            return Action::Launch(Launch::Editor(Some(h.path.clone())));
                        }
                    }
                    self.last_click = Some((now, idx));
                }
            }
            MouseEventKind::ScrollUp => self.sel = self.sel.saturating_sub(3),
            MouseEventKind::ScrollDown => {
                if !self.hits.is_empty() {
                    self.sel = (self.sel + 3).min(self.hits.len() - 1);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn workspace_path(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }
}
