//! Process list for the developer desktop.

use std::process::{Command, Stdio};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::scroll::{self, ScrollState};
use super::{Action, AppWindow};

struct Proc {
    pid: String,
    name: String,
    extra: String,
}

pub struct Processes {
    procs: Vec<Proc>,
    filtered: Vec<usize>,
    sel: usize,
    scroll: ScrollState,
    view_h: u16,
    filter: String,
    filtering: bool,
    status: String,
    confirm_kill: Option<String>,
    tick: u32,
}

impl Processes {
    pub fn new() -> Self {
        let mut p = Processes {
            procs: Vec::new(),
            filtered: Vec::new(),
            sel: 0,
            scroll: ScrollState::default(),
            view_h: 20,
            filter: String::new(),
            filtering: false,
            status: String::new(),
            confirm_kill: None,
            tick: 0,
        };
        p.refresh();
        p
    }

    fn refresh(&mut self) {
        self.procs = list_procs();
        self.apply_filter();
        self.status = format!("{} processes", self.procs.len());
    }

    fn apply_filter(&mut self) {
        let q = self.filter.to_lowercase();
        self.filtered = self
            .procs
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                q.is_empty()
                    || p.name.to_lowercase().contains(&q)
                    || p.pid.contains(&q)
                    || p.extra.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect();
        if self.sel >= self.filtered.len() {
            self.sel = self.filtered.len().saturating_sub(1);
        }
    }

    fn selected(&self) -> Option<&Proc> {
        self.filtered
            .get(self.sel)
            .and_then(|&i| self.procs.get(i))
    }

    fn kill_selected(&mut self) {
        let Some(p) = self.selected().map(|p| (p.pid.clone(), p.name.clone())) else {
            return;
        };
        let (pid, name) = p;
        let ok = kill_pid(&pid);
        self.confirm_kill = None;
        if ok {
            self.status = format!("killed {name} ({pid})");
            self.refresh();
        } else {
            self.status = format!("kill failed: {name} ({pid})");
        }
    }
}

fn list_procs() -> Vec<Proc> {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("tasklist");
        cmd.args(["/FO", "CSV", "/NH"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let Ok(out) = cmd.output() else {
            return Vec::new();
        };
        let mut procs = Vec::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            // "name","pid","session","session#","mem"
            let cols: Vec<&str> = line.split(',').collect();
            if cols.len() < 2 {
                continue;
            }
            let name = cols[0].trim_matches('"').to_string();
            let pid = cols[1].trim_matches('"').to_string();
            let extra = cols.get(4).map(|s| s.trim_matches('"').to_string()).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            procs.push(Proc { pid, name, extra });
        }
        procs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        procs
    }
    #[cfg(not(windows))]
    {
        let out = Command::new("ps")
            .args(["-axo", "pid=,comm=,args="])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        let Ok(out) = out else {
            return Vec::new();
        };
        let mut procs = Vec::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let line = line.trim();
            let mut parts = line.split_whitespace();
            let Some(pid) = parts.next() else { continue };
            let Some(name) = parts.next() else { continue };
            let args: Vec<&str> = parts.collect();
            procs.push(Proc {
                pid: pid.to_string(),
                name: name.to_string(),
                extra: args.join(" ").chars().take(60).collect(),
            });
        }
        procs
    }
}

fn kill_pid(pid: &str) -> bool {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", pid, "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.status().map(|s| s.success()).unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .args([pid])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl AppWindow for Processes {
    fn title(&self) -> String {
        "Processes".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 3 {
            return;
        }
        let header = if self.filtering {
            format!(" filter: {}█ ", self.filter)
        } else if let Some(pid) = &self.confirm_kill {
            format!(" kill PID {pid}? y/n ")
        } else {
            format!(" {} procs · / filter · Del kill · F5 refresh ", self.filtered.len())
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                header,
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(28, 36, 62))
                    .add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(Color::Rgb(28, 36, 62))),
            Rect::new(area.x, area.y, area.width, 1),
        );

        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
        let sel_bg = Color::Rgb(0, 90, 140);
        let lines: Vec<Line> = self
            .filtered
            .iter()
            .enumerate()
            .map(|(i, &pi)| {
                let p = &self.procs[pi];
                let style = if i == self.sel {
                    Style::default()
                        .fg(Color::White)
                        .bg(sel_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Rgb(210, 215, 230))
                };
                Line::from(Span::styled(
                    format!(" {:>7}  {:<24}  {}", p.pid, truncate(&p.name, 24), p.extra),
                    style,
                ))
            })
            .collect();
        self.view_h = body.height;
        if self.sel < self.scroll.offset as usize {
            self.scroll.offset = self.sel as u16;
        }
        if self.sel >= self.scroll.offset as usize + self.view_h as usize {
            self.scroll.offset = (self.sel + 1).saturating_sub(self.view_h as usize) as u16;
        }
        scroll::render_scrollable(f, body, &lines, &mut self.scroll);

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

        if let Some(pid) = self.confirm_kill.clone() {
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.kill_selected(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_kill = None;
                    self.status = format!("keep {pid}");
                }
                _ => {}
            }
            return Action::None;
        }

        if self.filtering {
            match k.code {
                KeyCode::Esc => {
                    self.filtering = false;
                }
                KeyCode::Enter => {
                    self.filtering = false;
                    self.apply_filter();
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.apply_filter();
                }
                KeyCode::Char(c) if !ctrl => {
                    self.filter.push(c);
                    self.apply_filter();
                }
                _ => {}
            }
            return Action::None;
        }

        match k.code {
            KeyCode::Char('/') => {
                self.filtering = true;
            }
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => {
                if !self.filtered.is_empty() {
                    self.sel = (self.sel + 1).min(self.filtered.len() - 1);
                }
            }
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => {
                if !self.filtered.is_empty() {
                    self.sel = (self.sel + 10).min(self.filtered.len() - 1);
                }
            }
            KeyCode::F(5) => self.refresh(),
            KeyCode::Enter => {
                if let Some(p) = self.selected() {
                    self.status = format!("PID {} · {}", p.pid, p.name);
                }
            }
            KeyCode::Delete | KeyCode::Char('k') if ctrl || matches!(k.code, KeyCode::Delete) => {
                if let Some((pid, name)) = self.selected().map(|p| (p.pid.clone(), p.name.clone())) {
                    self.confirm_kill = Some(pid.clone());
                    self.status = format!("kill {name} ({pid})?");
                }
            }
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
            MouseEventKind::Down(MouseButton::Left)
                if m.row >= body.y && m.row < body.y + body.height =>
            {
                let idx = self.scroll.offset as usize + (m.row - body.y) as usize;
                if idx < self.filtered.len() {
                    self.sel = idx;
                }
            }
            MouseEventKind::ScrollUp => self.sel = self.sel.saturating_sub(3),
            MouseEventKind::ScrollDown => {
                if !self.filtered.is_empty() {
                    self.sel = (self.sel + 3).min(self.filtered.len() - 1);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn on_tick(&mut self) {
        self.tick += 1;
        if self.tick % 40 == 0 {
            self.refresh();
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}
