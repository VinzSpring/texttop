//! Project search — uses `rg` when available, else a simple recursive walk.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::scroll::{self, ScrollState};
use super::{Action, AppWindow, Launch};

struct Hit {
    path: PathBuf,
    line: usize,
    text: String,
}

pub struct SearchApp {
    root: PathBuf,
    query: String,
    editing: bool,
    hits: Vec<Hit>,
    sel: usize,
    scroll: ScrollState,
    view_h: u16,
    status: String,
    use_rg: bool,
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
            hits: Vec::new(),
            sel: 0,
            scroll: ScrollState::default(),
            view_h: 20,
            status: if use_rg {
                "ripgrep ready · type query · Enter search".into()
            } else {
                "builtin walk · type query · Enter search".into()
            },
            use_rg,
        }
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
        let q_style = if self.editing {
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(40, 70, 110))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Rgb(200, 210, 230)).bg(Color::Rgb(28, 36, 62))
        };
        let qline = if self.editing {
            format!(" 🔍 {}█  in {} ", self.query, self.root.display())
        } else {
            format!(" 🔍 {}  in {} ", self.query, self.root.display())
        };
        f.render_widget(
            Paragraph::new(Span::styled(qline, q_style)).style(q_style),
            Rect::new(area.x, area.y, area.width, 1),
        );

        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
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

        f.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    " {} │ / edit query · Enter search/open · Ctrl+L root · ↑↓ · q ",
                    self.status
                ),
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
        if self.editing {
            match k.code {
                KeyCode::Esc => {
                    if self.hits.is_empty() {
                        return Action::Close;
                    }
                    self.editing = false;
                }
                KeyCode::Enter => self.run_search(),
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(c) if !ctrl => self.query.push(c),
                KeyCode::Char('l') if ctrl => {
                    self.query = self.root.to_string_lossy().to_string();
                    // treat as path change on next enter? simpler: prompt via query
                    self.status = "edit root path then Ctrl+L again… use Files".into();
                }
                _ => {}
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
                self.editing = true;
                self.status = format!("root: {}", self.root.display());
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
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
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
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if m.row == area.y => {
                self.editing = true;
            }
            MouseEventKind::Down(MouseButton::Left)
                if m.row >= body.y && m.row < body.y + body.height =>
            {
                let idx = self.scroll.offset as usize + (m.row - body.y) as usize;
                if idx < self.hits.len() {
                    self.sel = idx;
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
