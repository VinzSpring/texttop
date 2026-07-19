use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{file_icon, launch_for_path, Action, AppWindow, Launch};
use crate::gfx;

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    is_up: bool,
    size: u64,
}

pub struct FileBrowser {
    cwd: Option<PathBuf>, // None = drive list
    entries: Vec<Entry>,
    sel: usize,
    scroll: usize,
    show_hidden: bool,
    status: String,
    last_click: Option<(Instant, usize)>,
}

impl FileBrowser {
    pub fn new(path: PathBuf) -> Self {
        let mut fb = FileBrowser {
            cwd: if path.exists() { Some(path) } else { None },
            entries: Vec::new(),
            sel: 0,
            scroll: 0,
            show_hidden: false,
            status: String::new(),
            last_click: None,
        };
        fb.refresh();
        fb
    }

    fn refresh(&mut self) {
        self.entries.clear();
        self.sel = 0;
        self.scroll = 0;
        match &self.cwd {
            None => {
                #[cfg(windows)]
                {
                    for letter in b'A'..=b'Z' {
                        let root = format!("{}:\\", letter as char);
                        if fs::metadata(&root).is_ok() {
                            self.entries.push(Entry {
                                name: root.clone(),
                                path: PathBuf::from(root),
                                is_dir: true,
                                is_up: false,
                                size: 0,
                            });
                        }
                    }
                    self.status = format!("{} drives", self.entries.len());
                }
                #[cfg(not(windows))]
                {
                    self.cwd = Some(PathBuf::from("/"));
                    return self.refresh();
                }
            }
            Some(dir) => {
                if dir.parent().is_some() || true {
                    self.entries.push(Entry {
                        name: "..".into(),
                        path: dir.clone(),
                        is_dir: true,
                        is_up: true,
                        size: 0,
                    });
                }
                match fs::read_dir(dir) {
                    Ok(rd) => {
                        let mut items: Vec<Entry> = Vec::new();
                        for e in rd.flatten() {
                            let name = e.file_name().to_string_lossy().to_string();
                            if !self.show_hidden && name.starts_with('.') {
                                continue;
                            }
                            let md = e.metadata().ok();
                            let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                            items.push(Entry {
                                name,
                                path: e.path(),
                                is_dir,
                                is_up: false,
                                size: md.map(|m| m.len()).unwrap_or(0),
                            });
                        }
                        items.sort_by(|a, b| {
                            b.is_dir
                                .cmp(&a.is_dir)
                                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                        });
                        let n = items.len();
                        self.entries.extend(items);
                        self.status = format!("{n} items");
                    }
                    Err(e) => self.status = format!("error: {e}"),
                }
            }
        }
    }

    fn go_up(&mut self) {
        let parent = self.cwd.as_ref().and_then(|d| d.parent().map(|p| p.to_path_buf()));
        // Windows: None -> drive list. Unix: stop at /
        #[cfg(windows)]
        {
            self.cwd = parent;
        }
        #[cfg(not(windows))]
        {
            self.cwd = Some(parent.unwrap_or_else(|| PathBuf::from("/")));
        }
        self.refresh();
    }

    fn open_sel(&mut self) -> Action {
        if self.entries.is_empty() {
            return Action::None;
        }
        let e = &self.entries[self.sel];
        if e.is_up {
            self.go_up();
            return Action::None;
        }
        if e.is_dir {
            self.cwd = Some(e.path.clone());
            self.refresh();
            return Action::None;
        }
        Action::Launch(launch_for_path(&e.path))
    }

    fn move_sel(&mut self, delta: i64) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as i64;
        self.sel = (self.sel as i64 + delta).clamp(0, n - 1) as usize;
    }

    fn list_area(area: Rect) -> Rect {
        Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        )
    }
}

impl AppWindow for FileBrowser {
    fn title(&self) -> String {
        match &self.cwd {
            None => "Drives".into(),
            Some(p) => {
                let s = p.to_string_lossy().to_string();
                if s.chars().count() > 30 {
                    format!("…{}", s.chars().skip(s.chars().count() - 29).collect::<String>())
                } else {
                    s
                }
            }
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        if area.height < 3 {
            return;
        }
        // path bar
        let path_str = match &self.cwd {
            None => " 💻 This PC ".to_string(),
            Some(p) => format!(" 📂 {} ", p.to_string_lossy()),
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                path_str,
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(28, 36, 62))
                    .add_modifier(Modifier::BOLD),
            )))
            .style(Style::default().bg(Color::Rgb(28, 36, 62))),
            Rect::new(area.x, area.y, area.width, 1),
        );

        let list = Self::list_area(area);
        let vis = list.height as usize;
        if vis > 0 {
            if self.sel < self.scroll {
                self.scroll = self.sel;
            }
            if self.sel >= self.scroll + vis {
                self.scroll = self.sel + 1 - vis;
            }
        }
        let mut lines: Vec<Line> = Vec::new();
        for (i, e) in self
            .entries
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(vis)
        {
            let (icon, color) = if e.is_up {
                ("↩", Color::Rgb(140, 150, 180))
            } else {
                file_icon(&e.name, e.is_dir)
            };
            let size = if e.is_dir {
                "<DIR>".to_string()
            } else {
                gfx::fmt_size(e.size)
            };
            let name_w = list.width.saturating_sub(14) as usize;
            let mut name = e.name.clone();
            if name.chars().count() > name_w {
                name = name.chars().take(name_w.saturating_sub(1)).collect::<String>() + "…";
            }
            let pad = name_w.saturating_sub(name.chars().count());
            let selected = i == self.sel;
            let base = if selected && focused {
                Style::default().fg(Color::White).bg(Color::Rgb(0, 96, 150))
            } else if selected {
                Style::default().fg(Color::White).bg(Color::Rgb(48, 52, 74))
            } else {
                Style::default().fg(color)
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {icon} "), base),
                Span::styled(
                    format!("{}{} ", name, " ".repeat(pad)),
                    if selected { base.add_modifier(Modifier::BOLD) } else { base },
                ),
                Span::styled(
                    format!("{size:>9} "),
                    base.fg(if selected { Color::White } else { Color::Rgb(130, 135, 155) }),
                ),
            ]));
        }
        f.render_widget(Paragraph::new(lines), list);

        // status bar
        let hint = " Enter open · Bksp up · Ctrl+H hidden · F5 refresh · F4 edit ";
        let status = format!(" {}  │{}", self.status, hint);
        f.render_widget(
            Paragraph::new(Span::styled(
                status,
                Style::default().fg(Color::Rgb(140, 150, 175)).bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Down => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-12),
            KeyCode::PageDown => self.move_sel(12),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = self.entries.len().saturating_sub(1),
            KeyCode::Enter => return self.open_sel(),
            KeyCode::Backspace => self.go_up(),
            KeyCode::Char('h') if ctrl => {
                self.show_hidden = !self.show_hidden;
                self.refresh();
            }
            KeyCode::F(5) => self.refresh(),
            KeyCode::F(4) => {
                if let Some(e) = self.entries.get(self.sel) {
                    if !e.is_dir && !e.is_up {
                        return Action::Launch(Launch::Editor(Some(e.path.clone())));
                    }
                }
            }
            KeyCode::Char(c) if !ctrl => {
                // type-to-jump: next entry starting with this letter
                let lc = c.to_ascii_lowercase();
                let n = self.entries.len();
                for off in 1..=n {
                    let i = (self.sel + off) % n;
                    if self.entries[i]
                        .name
                        .chars()
                        .next()
                        .map(|f| f.to_ascii_lowercase() == lc)
                        .unwrap_or(false)
                    {
                        self.sel = i;
                        break;
                    }
                }
            }
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        let list = Self::list_area(area);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if m.row >= list.y && m.row < list.y + list.height {
                    let idx = self.scroll + (m.row - list.y) as usize;
                    if idx < self.entries.len() {
                        let dbl = matches!(self.last_click, Some((t, li))
                            if li == idx && t.elapsed().as_millis() < 450);
                        self.sel = idx;
                        if dbl {
                            self.last_click = None;
                            return self.open_sel();
                        }
                        self.last_click = Some((Instant::now(), idx));
                    }
                }
            }
            MouseEventKind::ScrollUp => self.move_sel(-3),
            MouseEventKind::ScrollDown => self.move_sel(3),
            _ => {}
        }
        Action::None
    }
}
