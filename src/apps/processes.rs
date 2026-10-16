//! Process list for the developer desktop.

use std::process::{Command, Stdio};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::scroll::{self, ScrollState};
use super::ui::{self, Controls, Item, Look};
use super::{Action, AppWindow};

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    Search,
    ClearSearch,
    Refresh,
    Kill,
    ConfirmKill,
    CancelKill,
    CopyPid,
    CopyName,
}

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
    status: String,
    ui: Controls<Cmd>,
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
            status: String::new(),
            ui: Controls::new(),
            confirm_kill: None,
            tick: 0,
        };
        p.refresh();
        p
    }

    fn refresh(&mut self) {
        self.procs = list_procs();
        self.apply_filter();
        self.status.clear();
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

    fn ask_kill(&mut self) {
        if let Some((pid, name)) = self.selected().map(|p| (p.pid.clone(), p.name.clone())) {
            self.confirm_kill = Some(pid);
            self.status = format!("end {name}?");
        }
    }

    fn run(&mut self, c: Cmd) {
        match c {
            Cmd::Search => {}
            Cmd::ClearSearch => {
                self.filter.clear();
                self.apply_filter();
            }
            Cmd::Refresh => self.refresh(),
            Cmd::Kill => self.ask_kill(),
            Cmd::ConfirmKill => self.kill_selected(),
            Cmd::CancelKill => {
                self.confirm_kill = None;
                self.status.clear();
            }
            Cmd::CopyPid | Cmd::CopyName => {
                if let Some(p) = self.selected() {
                    let text = if c == Cmd::CopyPid { p.pid.clone() } else { p.name.clone() };
                    crate::clipboard::copy(&text);
                    self.status = format!("copied {text}");
                }
            }
        }
    }

    fn body(area: Rect) -> Rect {
        Rect::new(area.x, area.y + 2, area.width, area.height.saturating_sub(3))
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

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        if area.height < 4 {
            return;
        }
        self.ui.begin();
        let buf = f.buffer_mut();
        let bar = Rect::new(area.x, area.y, area.width, 1);
        if let Some(pid) = &self.confirm_kill {
            let name = self.selected().map(|p| p.name.clone()).unwrap_or_default();
            self.ui.toolbar(
                buf,
                bar,
                &[Item::Label(format!(" End {name} (PID {pid})? "), Color::White)],
                &[
                    Item::with("✖ End task", Cmd::ConfirmKill, Look::Danger),
                    Item::with("Cancel", Cmd::CancelKill, Look::Raised),
                ],
            );
        } else {
            let has = self.selected().is_some();
            let field = self.ui.toolbar(
                buf,
                bar,
                &[Item::btn("⟳ Refresh", Cmd::Refresh)],
                &[Item::with("✖ End task", Cmd::Kill, Look::enabled_if(has))],
            );
            // The search box: always live, so typing anywhere filters.
            let fx = field.x + 1;
            let fw = field.width.saturating_sub(2).min(40);
            if fw >= 8 {
                let r = Rect::new(fx, bar.y, fw, 1);
                let bg = Color::Rgb(12, 16, 30);
                ui::fill_row(buf, r, bg);
                let text = if self.filter.is_empty() {
                    Span::styled(" 🔍 type to filter…", Style::default().fg(ui::DIM).bg(bg))
                } else {
                    let caret = if focused { "▏" } else { "" };
                    Span::styled(
                        format!(" 🔍 {}{caret}", self.filter),
                        Style::default().fg(Color::White).bg(bg),
                    )
                };
                buf.set_string(r.x, r.y, text.content.chars().take(fw as usize - 2).collect::<String>(), text.style);
                self.ui.add(r, Cmd::Search);
                if !self.filter.is_empty() {
                    buf.set_string(r.x + r.width - 2, r.y, "✕ ", Style::default().fg(ui::FG).bg(bg));
                    self.ui.add(Rect::new(r.x + r.width - 2, r.y, 2, 1), Cmd::ClearSearch);
                }
            }
        }

        // column headings
        buf.set_string(
            area.x,
            area.y + 1,
            format!("{:<width$}", format!(" {:>7}  {:<24}  {}", "PID", "Name", MEM_HEAD), width = area.width as usize),
            Style::default()
                .fg(Color::Rgb(150, 160, 190))
                .bg(Color::Rgb(22, 27, 45))
                .add_modifier(Modifier::BOLD),
        );

        let body = Self::body(area);
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

        let status = if self.status.is_empty() || self.confirm_kill.is_some() {
            format!("{} of {} processes", self.filtered.len(), self.procs.len())
        } else {
            self.status.clone()
        };
        ui::status_bar(
            f.buffer_mut(),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
            &status,
        );
        self.ui.render_menu(f.buffer_mut(), area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        if self.ui.menu_open() && k.code == KeyCode::Esc {
            self.ui.close_menu();
            return Action::None;
        }
        if self.confirm_kill.is_some() {
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.run(Cmd::ConfirmKill),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.run(Cmd::CancelKill),
                _ => {}
            }
            return Action::None;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
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
            KeyCode::Delete => self.ask_kill(),
            KeyCode::Char('k') if ctrl => self.ask_kill(),
            KeyCode::Esc if !self.filter.is_empty() => self.run(Cmd::ClearSearch),
            KeyCode::Esc => return Action::Close,
            _ => {
                if super::text_edit(&mut self.filter, k) {
                    self.apply_filter();
                }
            }
        }
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        super::paste_line(&mut self.filter, text);
        self.apply_filter();
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        if let Some(hit) = self.ui.route(&m) {
            if let Some(c) = hit {
                self.run(c);
            }
            return Action::None;
        }
        if !matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown)
            && self.scroll.on_mouse(m, self.filtered.len(), self.view_h)
        {
            // Dragging the scrollbar: carry the selection along so the
            // keep-selection-visible rule in render does not snap back.
            let top = self.scroll.offset as usize;
            let bottom = (top + self.view_h as usize).saturating_sub(1);
            self.sel = self.sel.clamp(top, bottom.max(top)).min(self.filtered.len().saturating_sub(1));
            return Action::None;
        }
        let body = Self::body(area);
        let in_body = m.row >= body.y && m.row < body.y + body.height;
        match m.kind {
            MouseEventKind::Down(b) if in_body && self.confirm_kill.is_none() => {
                let idx = self.scroll.offset as usize + (m.row - body.y) as usize;
                if idx < self.filtered.len() {
                    self.sel = idx;
                    if b == MouseButton::Right {
                        self.ui.open_menu(
                            m.column,
                            m.row,
                            vec![
                                ("✖ End task".into(), Some(Cmd::Kill)),
                                (String::new(), None),
                                ("Copy PID".into(), Some(Cmd::CopyPid)),
                                ("Copy name".into(), Some(Cmd::CopyName)),
                            ],
                        );
                    }
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
        if self.tick % 40 == 0 && self.confirm_kill.is_none() {
            let status = std::mem::take(&mut self.status);
            self.refresh();
            self.status = status;
        }
    }
}

/// Heading of the third column: memory on Windows, the command line elsewhere.
#[cfg(windows)]
const MEM_HEAD: &str = "Memory";
#[cfg(not(windows))]
const MEM_HEAD: &str = "Command";

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}
