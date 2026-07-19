use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{file_icon, launch_for_path, Action, AppWindow, Launch};
use crate::gfx;

#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    Name,
    Size,
    Modified,
}

enum Prompt {
    Filter,
    Rename,
    Mkdir,
    NewFile,
    Goto,
    DeleteConfirm,
}

#[derive(Clone)]
enum ClipOp {
    Copy,
    Cut,
}

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    is_up: bool,
    size: u64,
    modified: Option<SystemTime>,
}

pub struct FileBrowser {
    cwd: Option<PathBuf>, // None = drive list
    entries: Vec<Entry>,
    filtered: Vec<usize>,
    sel: usize,
    scroll: usize,
    show_hidden: bool,
    sort: SortBy,
    preview: bool,
    status: String,
    last_click: Option<(Instant, usize)>,
    prompt: Option<(Prompt, String)>,
    filter: String,
    clipboard: Option<(ClipOp, PathBuf)>,
    preview_text: String,
}

impl FileBrowser {
    pub fn new(path: PathBuf) -> Self {
        let mut fb = FileBrowser {
            cwd: if path.exists() { Some(path) } else { None },
            entries: Vec::new(),
            filtered: Vec::new(),
            sel: 0,
            scroll: 0,
            show_hidden: false,
            sort: SortBy::Name,
            preview: false,
            status: String::new(),
            last_click: None,
            prompt: None,
            filter: String::new(),
            clipboard: None,
            preview_text: String::new(),
        };
        fb.refresh();
        fb
    }

    fn refresh(&mut self) {
        let keep_name = self
            .current()
            .map(|e| e.name.clone());
        self.entries.clear();
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
                                modified: None,
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
                self.entries.push(Entry {
                    name: "..".into(),
                    path: dir.clone(),
                    is_dir: true,
                    is_up: true,
                    size: 0,
                    modified: None,
                });
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
                                size: md.as_ref().map(|m| m.len()).unwrap_or(0),
                                modified: md.and_then(|m| m.modified().ok()),
                            });
                        }
                        self.sort_items(&mut items);
                        let n = items.len();
                        self.entries.extend(items);
                        self.status = format!("{n} items · sort:{}", self.sort_label());
                    }
                    Err(e) => self.status = format!("error: {e}"),
                }
            }
        }
        self.apply_filter();
        if let Some(name) = keep_name {
            if let Some(i) = self
                .filtered
                .iter()
                .position(|&ei| self.entries[ei].name == name)
            {
                self.sel = i;
            } else {
                self.sel = 0;
            }
        } else {
            self.sel = 0;
        }
        self.scroll = 0;
        self.update_preview();
    }

    fn sort_label(&self) -> &'static str {
        match self.sort {
            SortBy::Name => "name",
            SortBy::Size => "size",
            SortBy::Modified => "date",
        }
    }

    fn sort_items(&self, items: &mut [Entry]) {
        items.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then_with(|| match self.sort {
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortBy::Size => a.size.cmp(&b.size).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                SortBy::Modified => a
                    .modified
                    .cmp(&b.modified)
                    .then(a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            })
        });
    }

    fn apply_filter(&mut self) {
        let q = self.filter.to_lowercase();
        self.filtered = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                if e.is_up {
                    return q.is_empty();
                }
                q.is_empty() || e.name.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect();
        if self.sel >= self.filtered.len() {
            self.sel = self.filtered.len().saturating_sub(1);
        }
    }

    fn current(&self) -> Option<&Entry> {
        self.filtered
            .get(self.sel)
            .and_then(|&i| self.entries.get(i))
    }

    fn go_up(&mut self) {
        let parent = self.cwd.as_ref().and_then(|d| d.parent().map(|p| p.to_path_buf()));
        #[cfg(windows)]
        {
            self.cwd = parent;
        }
        #[cfg(not(windows))]
        {
            self.cwd = Some(parent.unwrap_or_else(|| PathBuf::from("/")));
        }
        self.filter.clear();
        self.refresh();
    }

    fn open_sel(&mut self) -> Action {
        let Some(e) = self.current() else {
            return Action::None;
        };
        if e.is_up {
            self.go_up();
            return Action::None;
        }
        if e.is_dir {
            self.cwd = Some(e.path.clone());
            self.filter.clear();
            self.refresh();
            return Action::None;
        }
        Action::Launch(launch_for_path(&e.path))
    }

    fn move_sel(&mut self, delta: i64) {
        if self.filtered.is_empty() {
            return;
        }
        let n = self.filtered.len() as i64;
        self.sel = (self.sel as i64 + delta).clamp(0, n - 1) as usize;
        self.update_preview();
    }

    fn list_area(area: Rect, preview: bool) -> Rect {
        let w = if preview {
            area.width.saturating_sub(area.width / 3).max(20)
        } else {
            area.width
        };
        Rect::new(
            area.x,
            area.y + 1,
            w,
            area.height.saturating_sub(2),
        )
    }

    fn preview_area(area: Rect) -> Rect {
        let lw = area.width.saturating_sub(area.width / 3).max(20);
        Rect::new(
            area.x + lw,
            area.y + 1,
            area.width.saturating_sub(lw),
            area.height.saturating_sub(2),
        )
    }

    fn update_preview(&mut self) {
        self.preview_text.clear();
        if !self.preview {
            return;
        }
        let Some(e) = self.current() else {
            self.preview_text = " (nothing selected)".into();
            return;
        };
        if e.is_up {
            self.preview_text = " ↑ parent directory".into();
            return;
        }
        if e.is_dir {
            match fs::read_dir(&e.path) {
                Ok(rd) => {
                    let mut names: Vec<String> = rd
                        .flatten()
                        .map(|x| x.file_name().to_string_lossy().to_string())
                        .collect();
                    names.sort_by_key(|s| s.to_lowercase());
                    let n = names.len();
                    let shown: Vec<_> = names.into_iter().take(40).collect();
                    self.preview_text = format!(
                        " 📁 {} items\n\n{}",
                        n,
                        shown
                            .iter()
                            .map(|n| format!("  {n}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                }
                Err(err) => self.preview_text = format!(" error: {err}"),
            }
            return;
        }
        match fs::File::open(&e.path) {
            Ok(mut f) => {
                let mut buf = vec![0u8; 4096];
                match f.read(&mut buf) {
                    Ok(n) => {
                        buf.truncate(n);
                        if buf.iter().any(|&b| b == 0) {
                            self.preview_text = format!(
                                " binary · {} · F3 hex view\n\n{}",
                                gfx::fmt_size(e.size),
                                hex_preview(&buf)
                            );
                        } else {
                            let text = String::from_utf8_lossy(&buf);
                            self.preview_text = format!(
                                " {} · text preview\n\n{}",
                                gfx::fmt_size(e.size),
                                text.replace('\r', "")
                            );
                        }
                    }
                    Err(err) => self.preview_text = format!(" error: {err}"),
                }
            }
            Err(err) => self.preview_text = format!(" error: {err}"),
        }
    }

    fn start_prompt(&mut self, kind: Prompt, initial: String) {
        self.prompt = Some((kind, initial));
    }

    fn commit_prompt(&mut self) -> Action {
        let Some((kind, input)) = self.prompt.take() else {
            return Action::None;
        };
        let input = input.trim().to_string();
        match kind {
            Prompt::Filter => {
                self.filter = input;
                self.apply_filter();
                self.sel = 0;
                self.scroll = 0;
                self.status = if self.filter.is_empty() {
                    format!("{} items", self.filtered.len().saturating_sub(1))
                } else {
                    format!("filter: {} → {} matches", self.filter, self.filtered.len())
                };
                self.update_preview();
            }
            Prompt::Rename => {
                if input.is_empty() {
                    self.status = "rename cancelled".into();
                    return Action::None;
                }
                if let Some(e) = self.current() {
                    if e.is_up {
                        self.status = "cannot rename ..".into();
                    } else {
                        let dest = e.path.with_file_name(&input);
                        match fs::rename(&e.path, &dest) {
                            Ok(()) => {
                                self.status = format!("renamed → {input}");
                                self.refresh();
                            }
                            Err(err) => self.status = format!("rename failed: {err}"),
                        }
                    }
                }
            }
            Prompt::Mkdir => {
                if input.is_empty() {
                    self.status = "mkdir cancelled".into();
                    return Action::None;
                }
                if let Some(dir) = &self.cwd {
                    let dest = dir.join(&input);
                    match fs::create_dir(&dest) {
                        Ok(()) => {
                            self.status = format!("created folder {input}");
                            self.refresh();
                        }
                        Err(err) => self.status = format!("mkdir failed: {err}"),
                    }
                }
            }
            Prompt::NewFile => {
                if input.is_empty() {
                    self.status = "new file cancelled".into();
                    return Action::None;
                }
                if let Some(dir) = &self.cwd {
                    let dest = dir.join(&input);
                    if dest.exists() {
                        self.status = "file already exists".into();
                    } else {
                        match fs::File::create(&dest) {
                            Ok(_) => {
                                self.status = format!("created {input}");
                                self.refresh();
                                return Action::Launch(Launch::Editor(Some(dest)));
                            }
                            Err(err) => self.status = format!("create failed: {err}"),
                        }
                    }
                }
            }
            Prompt::Goto => {
                if input.is_empty() {
                    self.status = "goto cancelled".into();
                    return Action::None;
                }
                let path = expand_path(&input);
                if path.is_dir() {
                    self.cwd = Some(path);
                    self.filter.clear();
                    self.refresh();
                } else if path.is_file() {
                    if let Some(parent) = path.parent() {
                        self.cwd = Some(parent.to_path_buf());
                        self.filter.clear();
                        self.refresh();
                        if let Some(i) = self.filtered.iter().position(|&ei| self.entries[ei].path == path)
                        {
                            self.sel = i;
                            self.update_preview();
                        }
                    }
                    return Action::Launch(launch_for_path(&path));
                } else {
                    self.status = format!("not found: {input}");
                }
            }
            Prompt::DeleteConfirm => {
                if !matches!(input.to_lowercase().as_str(), "y" | "yes") {
                    self.status = "delete cancelled".into();
                    return Action::None;
                }
                if let Some(e) = self.current() {
                    if e.is_up {
                        self.status = "cannot delete ..".into();
                    } else {
                        let name = e.name.clone();
                        let path = e.path.clone();
                        let is_dir = e.is_dir;
                        let result = if is_dir {
                            fs::remove_dir_all(&path)
                        } else {
                            fs::remove_file(&path)
                        };
                        match result {
                            Ok(()) => {
                                self.status = format!("deleted {name}");
                                self.refresh();
                            }
                            Err(err) => self.status = format!("delete failed: {err}"),
                        }
                    }
                }
            }
        }
        Action::None
    }

    fn copy_sel(&mut self, cut: bool) {
        let Some((name, path, is_up)) = self.current().map(|e| {
            (e.name.clone(), e.path.clone(), e.is_up)
        }) else {
            return;
        };
        if is_up {
            self.status = "cannot clipboard ..".into();
            return;
        }
        let op = if cut { ClipOp::Cut } else { ClipOp::Copy };
        self.status = format!("{} {name}", if cut { "cut" } else { "copied" });
        self.clipboard = Some((op, path));
    }

    fn paste(&mut self) {
        let Some((op, src)) = self.clipboard.clone() else {
            self.status = "clipboard empty".into();
            return;
        };
        let Some(dir) = self.cwd.clone() else {
            self.status = "cannot paste here".into();
            return;
        };
        let Some(name) = src.file_name() else {
            self.status = "invalid clipboard path".into();
            return;
        };
        let mut dest = dir.join(name);
        if dest.exists() {
            dest = unique_path(&dest);
        }
        let result = if src.is_dir() {
            copy_dir_all(&src, &dest)
        } else {
            fs::copy(&src, &dest).map(|_| ())
        };
        match result {
            Ok(()) => {
                if matches!(op, ClipOp::Cut) {
                    let _ = if src.is_dir() {
                        fs::remove_dir_all(&src)
                    } else {
                        fs::remove_file(&src)
                    };
                    self.clipboard = None;
                    self.status = format!("moved → {}", dest.file_name().unwrap().to_string_lossy());
                } else {
                    self.status = format!("pasted → {}", dest.file_name().unwrap().to_string_lossy());
                }
                self.refresh();
            }
            Err(err) => self.status = format!("paste failed: {err}"),
        }
    }

    fn jump_bookmark(&mut self, which: char) {
        let path = match which {
            'h' | 'H' => dirs::home_dir(),
            'd' | 'D' => dirs::desktop_dir().or_else(dirs::home_dir),
            'o' | 'O' => dirs::document_dir().or_else(dirs::home_dir),
            'l' | 'L' => dirs::download_dir().or_else(dirs::home_dir),
            _ => None,
        };
        if let Some(p) = path {
            if p.is_dir() {
                self.cwd = Some(p);
                self.filter.clear();
                self.refresh();
            } else {
                self.status = "bookmark path missing".into();
            }
        }
    }

    fn fmt_mtime(t: Option<SystemTime>) -> String {
        let Some(t) = t else {
            return "        ".into();
        };
        let dt: chrono::DateTime<chrono::Local> = t.into();
        dt.format("%Y-%m-%d").to_string()
    }
}

fn expand_path(s: &str) -> PathBuf {
    let s = s.trim();
    if s == "~" || s.starts_with("~/") || s.starts_with("~\\") {
        if let Some(home) = dirs::home_dir() {
            if s.len() == 1 {
                return home;
            }
            return home.join(&s[2..]);
        }
    }
    PathBuf::from(s)
}

fn unique_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "copy".into());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    for i in 1..1000 {
        let candidate = parent.join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}-copy{ext}"))
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

fn hex_preview(buf: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in buf.chunks(8).take(12).enumerate() {
        let hex: String = chunk.iter().map(|b| format!("{b:02x} ")).collect();
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
        out.push_str(&format!("  {:04x}  {hex:<24} {ascii}\n", i * 8));
    }
    out
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
        let path_str = match &self.cwd {
            None => " 💻 This PC ".to_string(),
            Some(p) => {
                let mut s = format!(" 📂 {} ", p.to_string_lossy());
                if !self.filter.is_empty() {
                    s.push_str(&format!("🔍 {} ", self.filter));
                }
                s
            }
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

        let list = Self::list_area(area, self.preview);
        let vis = list.height as usize;
        if vis > 0 && !self.filtered.is_empty() {
            if self.sel < self.scroll {
                self.scroll = self.sel;
            }
            if self.sel >= self.scroll + vis {
                self.scroll = self.sel + 1 - vis;
            }
        }
        let mut lines: Vec<Line> = Vec::new();
        for (row, &ei) in self
            .filtered
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(vis)
        {
            let e = &self.entries[ei];
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
            let date = Self::fmt_mtime(e.modified);
            let meta_w = 22usize; // date + size
            let name_w = list.width.saturating_sub(4 + meta_w as u16) as usize;
            let mut name = e.name.clone();
            if name.chars().count() > name_w {
                name = name.chars().take(name_w.saturating_sub(1)).collect::<String>() + "…";
            }
            let pad = name_w.saturating_sub(name.chars().count());
            let selected = row == self.sel;
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
                    if selected {
                        base.add_modifier(Modifier::BOLD)
                    } else {
                        base
                    },
                ),
                Span::styled(
                    format!("{date} "),
                    base.fg(if selected {
                        Color::White
                    } else {
                        Color::Rgb(100, 110, 135)
                    }),
                ),
                Span::styled(
                    format!("{size:>8} "),
                    base.fg(if selected {
                        Color::White
                    } else {
                        Color::Rgb(130, 135, 155)
                    }),
                ),
            ]));
        }
        f.render_widget(Paragraph::new(lines), list);

        if self.preview {
            let pre = Self::preview_area(area);
            let mut plines: Vec<Line> = vec![Line::from(Span::styled(
                " Preview ",
                Style::default()
                    .fg(Color::Rgb(180, 200, 255))
                    .bg(Color::Rgb(22, 28, 48))
                    .add_modifier(Modifier::BOLD),
            ))];
            for line in self.preview_text.lines().take(pre.height.saturating_sub(1) as usize) {
                let mut s = line.to_string();
                let max = pre.width.saturating_sub(1) as usize;
                if s.chars().count() > max {
                    s = s.chars().take(max.saturating_sub(1)).collect::<String>() + "…";
                }
                plines.push(Line::from(Span::styled(
                    s,
                    Style::default().fg(Color::Rgb(170, 180, 200)),
                )));
            }
            f.render_widget(
                Paragraph::new(plines).style(Style::default().bg(Color::Rgb(16, 20, 34))),
                pre,
            );
        }

        let status = if let Some((kind, input)) = &self.prompt {
            let label = match kind {
                Prompt::Filter => "Filter",
                Prompt::Rename => "Rename",
                Prompt::Mkdir => "New folder",
                Prompt::NewFile => "New file",
                Prompt::Goto => "Go to",
                Prompt::DeleteConfirm => "Delete? (y/n)",
            };
            format!(" {label}: {input}█  Esc cancel ")
        } else {
            let clip = match &self.clipboard {
                Some((ClipOp::Copy, p)) => format!(
                    " · 📋{}",
                    p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                ),
                Some((ClipOp::Cut, p)) => format!(
                    " · ✂{}",
                    p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                ),
                None => String::new(),
            };
            format!(
                " {}{}  │ / filter · F2 rename · Del · F7/F8 new · Tab sort · Ctrl+P preview · Ctrl+T term ",
                self.status, clip
            )
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                status,
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
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        if let Some((_, ref mut input)) = self.prompt {
            match k.code {
                KeyCode::Esc => {
                    self.prompt = None;
                    self.status = "cancelled".into();
                }
                KeyCode::Enter => return self.commit_prompt(),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if !ctrl => input.push(c),
                _ => {}
            }
            return Action::None;
        }

        match k.code {
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Down => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-12),
            KeyCode::PageDown => self.move_sel(12),
            KeyCode::Home => {
                self.sel = 0;
                self.update_preview();
            }
            KeyCode::End => {
                self.sel = self.filtered.len().saturating_sub(1);
                self.update_preview();
            }
            KeyCode::Enter => return self.open_sel(),
            KeyCode::Backspace => self.go_up(),
            KeyCode::Tab => {
                self.sort = match self.sort {
                    SortBy::Name => SortBy::Size,
                    SortBy::Size => SortBy::Modified,
                    SortBy::Modified => SortBy::Name,
                };
                self.refresh();
            }
            KeyCode::Char('p') if ctrl => {
                self.preview = !self.preview;
                self.update_preview();
                self.status = if self.preview {
                    "preview on".into()
                } else {
                    "preview off".into()
                };
            }
            KeyCode::Char('/') if !ctrl => {
                self.start_prompt(Prompt::Filter, self.filter.clone());
            }
            KeyCode::Char('f') if ctrl => {
                self.start_prompt(Prompt::Filter, self.filter.clone());
            }
            KeyCode::Char('l') if ctrl => {
                let initial = self
                    .cwd
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.start_prompt(Prompt::Goto, initial);
            }
            KeyCode::Char('c') if ctrl => self.copy_sel(false),
            KeyCode::Char('x') if ctrl => self.copy_sel(true),
            KeyCode::Char('v') if ctrl => self.paste(),
            KeyCode::Char('t') if ctrl => {
                if let Some(dir) = self.cwd.clone() {
                    return Action::Launch(Launch::Terminal {
                        program: None,
                        args: Vec::new(),
                        cwd: Some(dir),
                    });
                }
            }
            KeyCode::Char('h') if ctrl => {
                self.show_hidden = !self.show_hidden;
                self.refresh();
            }
            KeyCode::Char(c) if alt => self.jump_bookmark(c),
            KeyCode::F(2) => {
                if let Some((name, is_up)) = self.current().map(|e| (e.name.clone(), e.is_up)) {
                    if !is_up {
                        self.start_prompt(Prompt::Rename, name);
                    }
                }
            }
            KeyCode::F(3) => {
                if let Some(path) = self
                    .current()
                    .filter(|e| !e.is_dir && !e.is_up)
                    .map(|e| e.path.clone())
                {
                    return Action::Launch(Launch::Hex(Some(path)));
                }
            }
            KeyCode::F(4) => {
                if let Some(path) = self
                    .current()
                    .filter(|e| !e.is_dir && !e.is_up)
                    .map(|e| e.path.clone())
                {
                    return Action::Launch(Launch::Editor(Some(path)));
                }
            }
            KeyCode::F(5) => self.refresh(),
            KeyCode::F(7) => self.start_prompt(Prompt::Mkdir, String::new()),
            KeyCode::F(8) => self.start_prompt(Prompt::NewFile, String::new()),
            KeyCode::Delete => {
                if let Some((name, is_up)) = self.current().map(|e| (e.name.clone(), e.is_up)) {
                    if !is_up {
                        self.start_prompt(Prompt::DeleteConfirm, String::new());
                        self.status = format!("delete {name}?");
                    }
                }
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                let lc = c.to_ascii_lowercase();
                let n = self.filtered.len();
                if n == 0 {
                    return Action::None;
                }
                for off in 1..=n {
                    let i = (self.sel + off) % n;
                    let ei = self.filtered[i];
                    if self.entries[ei]
                        .name
                        .chars()
                        .next()
                        .map(|f| f.to_ascii_lowercase() == lc)
                        .unwrap_or(false)
                    {
                        self.sel = i;
                        self.update_preview();
                        break;
                    }
                }
            }
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        if self.prompt.is_some() {
            return Action::None;
        }
        let list = Self::list_area(area, self.preview);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if m.row >= list.y && m.row < list.y + list.height {
                    let idx = self.scroll + (m.row - list.y) as usize;
                    if idx < self.filtered.len() {
                        let dbl = matches!(self.last_click, Some((t, li))
                            if li == idx && t.elapsed().as_millis() < 450);
                        self.sel = idx;
                        self.update_preview();
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
