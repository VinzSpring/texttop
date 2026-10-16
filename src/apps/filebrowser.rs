use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime};

use notify::{EventKind, RecursiveMode, Watcher};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::gitutil::{self, GitLetter};
use super::snaputil;
use super::ui::{Controls, Item, Look};
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

/// Buttons on the bottom action bar.
#[derive(Clone, Copy, PartialEq)]
enum Bar {
    NewFolder,
    NewFile,
    Copy,
    Cut,
    Paste,
    Rename,
    Delete,
    Filter,
    ClearFilter,
    More,
    UseImage,
    UseFolder,
    Close,
    PromptOk,
    PromptCancel,
}

/// Buttons on the toolbar row.
#[derive(Clone, Copy, PartialEq)]
enum ToolBtn {
    Back,
    Fwd,
    Up,
    Home,
    Refresh,
    Hidden,
    Preview,
}

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    is_up: bool,
    hidden: bool,
    size: u64,
    modified: Option<SystemTime>,
    git: Option<GitLetter>,
}

#[derive(Clone, Copy, PartialEq)]
enum FileMenuAction {
    Open,
    Edit,
    Hex,
    Copy,
    Cut,
    Paste,
    Rename,
    Delete,
    NewFolder,
    NewFile,
    TerminalHere,
    RunHere,
    Refresh,
    TogglePreview,
    ToggleHidden,
    GotoPath,
    DragCopy,
    DragCut,
    PinToDesktop,
    SetAsWallpaper,
    GitStage,
    GitUnstage,
    GitDiff,
    GitOpen,
    SnapshotCreate,
    SnapshotHistory,
    Compare,
}

struct FileContext {
    x: u16,
    y: u16,
    items: Vec<(String, FileMenuAction)>,
    sel: usize,
}

pub struct FileBrowser {
    cwd: Option<PathBuf>, // None = drive list
    entries: Vec<Entry>,
    filtered: Vec<usize>,
    sel: usize,
    scroll: usize,
    show_hidden: bool,
    /// How many entries the hidden-files filter removed on last refresh.
    hidden_skipped: usize,
    sort: SortBy,
    sort_asc: bool,
    /// Navigation history for back / forward (None = drive list).
    back: Vec<Option<PathBuf>>,
    fwd: Vec<Option<PathBuf>>,
    /// Hit boxes rebuilt every render: toolbar buttons, breadcrumb
    /// segments, sortable column headers, and the list scrollbar.
    toolbar_hits: Vec<(u16, u16, ToolBtn)>,
    crumb_hits: Vec<(u16, u16, Option<PathBuf>)>,
    header_hits: Vec<(u16, u16, SortBy)>,
    scroll_rect: Option<Rect>,
    scroll_drag: bool,
    preview: bool,
    status: String,
    last_click: Option<(Instant, usize)>,
    prompt: Option<(Prompt, String)>,
    filter: String,
    clipboard: Option<(ClipOp, PathBuf)>,
    preview_text: String,
    context: Option<FileContext>,
    drag_from: Option<(usize, Instant)>,
    git_root: Option<PathBuf>,
    git_summary: String,
    /// Multi-select marks (canonical paths of files/dirs).
    marked: HashSet<PathBuf>,
    bar: Controls<Bar>,
    /// When true, opening an image sets it as wallpaper and closes this window.
    pick_wallpaper: bool,
    /// When true, this is a folder / file picker: it answers `PickPath`.
    pick_path: bool,
    /// Watches the shown folder so the list follows changes made elsewhere.
    watcher: Option<notify::RecommendedWatcher>,
    watch_rx: Option<Receiver<()>>,
    watched: Option<PathBuf>,
    /// Last change seen; the list reloads once things have been quiet briefly.
    changed_at: Option<Instant>,
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
            hidden_skipped: 0,
            sort: SortBy::Name,
            sort_asc: true,
            back: Vec::new(),
            fwd: Vec::new(),
            toolbar_hits: Vec::new(),
            crumb_hits: Vec::new(),
            header_hits: Vec::new(),
            scroll_rect: None,
            scroll_drag: false,
            preview: false,
            status: String::new(),
            last_click: None,
            prompt: None,
            filter: String::new(),
            clipboard: None,
            preview_text: String::new(),
            context: None,
            drag_from: None,
            git_root: None,
            git_summary: String::new(),
            marked: HashSet::new(),
            bar: Controls::new(),
            pick_wallpaper: false,
            pick_path: false,
            watcher: None,
            watch_rx: None,
            watched: None,
            changed_at: None,
        };
        fb.refresh();
        fb
    }

    /// File browser used to pick a wallpaper image.
    pub fn wallpaper_picker(start: PathBuf) -> Self {
        let start = if start.is_dir() {
            start
        } else if let Some(parent) = start.parent().filter(|p| p.is_dir()) {
            parent.to_path_buf()
        } else {
            dirs::picture_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."))
        };
        let mut fb = Self::new(start);
        fb.pick_wallpaper = true;
        fb.preview = true;
        fb.status = "double-click an image to use it".into();
        fb
    }

    /// File browser used to choose a folder (Use this folder) or a file
    /// (double-click). The owner receives `Action::PickPath`.
    pub fn path_picker(start: PathBuf) -> Self {
        let mut fb = Self::new(start);
        fb.pick_path = true;
        fb.status = "open a folder, or double-click a file".into();
        fb
    }

    fn refresh(&mut self) {
        let keep_name = self
            .current()
            .map(|e| e.name.clone());
        self.entries.clear();
        self.hidden_skipped = 0;
        match &self.cwd {
            None => {
                self.git_root = None;
                self.git_summary.clear();
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
                                hidden: false,
                                size: 0,
                                modified: None,
                                git: None,
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
                    hidden: false,
                    size: 0,
                    modified: None,
                    git: None,
                });
                match fs::read_dir(dir) {
                    Ok(rd) => {
                        let mut items: Vec<Entry> = Vec::new();
                        for e in rd.flatten() {
                            let name = e.file_name().to_string_lossy().to_string();
                            let md = e.metadata().ok();
                            let hidden = is_hidden(&name, md.as_ref());
                            if !self.show_hidden && hidden {
                                self.hidden_skipped += 1;
                                continue;
                            }
                            let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                            items.push(Entry {
                                name,
                                path: e.path(),
                                is_dir,
                                is_up: false,
                                hidden,
                                size: md.as_ref().map(|m| m.len()).unwrap_or(0),
                                modified: md.and_then(|m| m.modified().ok()),
                                git: None,
                            });
                        }
                        self.sort_items(&mut items);
                        let n = items.len();
                        self.entries.extend(items);
                        self.status = if self.hidden_skipped > 0 {
                            format!("{n} items · {} hidden", self.hidden_skipped)
                        } else if self.show_hidden {
                            format!("{n} items · hidden shown")
                        } else {
                            format!("{n} items")
                        };
                    }
                    Err(e) => self.status = format!("error: {e}"),
                }
                // git badges
                self.git_root = gitutil::find_root(dir);
                if let Some(root) = self.git_root.clone() {
                    let st = gitutil::short_stat(&root);
                    self.git_summary = st.summary();
                    for e in &mut self.entries {
                        if e.is_up {
                            continue;
                        }
                        e.git = gitutil::letter_for(&root, &e.name, &e.path);
                    }
                    if !self.git_summary.is_empty() {
                        self.status = format!("{} · {}", self.status, self.git_summary);
                    }
                } else {
                    self.git_summary.clear();
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
        self.watch();
    }

    /// Follow the current folder with the directory watcher.
    fn watch(&mut self) {
        if self.cwd == self.watched {
            return;
        }
        if let (Some(w), Some(old)) = (&mut self.watcher, &self.watched) {
            let _ = w.unwatch(old);
        }
        self.watched = None;
        let Some(dir) = self.cwd.clone() else {
            return;
        };
        if self.watcher.is_none() {
            let (tx, rx) = mpsc::channel();
            let made = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res {
                    if is_listing_change(&ev) {
                        let _ = tx.send(());
                    }
                }
            });
            match made {
                Ok(w) => {
                    self.watcher = Some(w);
                    self.watch_rx = Some(rx);
                }
                Err(_) => return,
            }
        }
        if let Some(w) = &mut self.watcher {
            if w.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
                self.watched = Some(dir);
            }
        }
    }

    /// Reload after an outside change, keeping the selection, scroll position
    /// and any picker hint in place.
    fn auto_refresh(&mut self) {
        let scroll = self.scroll;
        let status = self.status.clone();
        self.refresh();
        self.scroll = scroll.min(self.filtered.len().saturating_sub(1));
        if self.pick_path || self.pick_wallpaper {
            self.status = status;
        }
    }

    fn sort_label(&self) -> String {
        let name = match self.sort {
            SortBy::Name => "name",
            SortBy::Size => "size",
            SortBy::Modified => "date",
        };
        format!("{name}{}", if self.sort_asc { "↑" } else { "↓" })
    }

    fn sort_items(&self, items: &mut [Entry]) {
        items.sort_by(|a, b| {
            let ord = match self.sort {
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortBy::Size => a.size.cmp(&b.size).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())),
                SortBy::Modified => a
                    .modified
                    .cmp(&b.modified)
                    .then(a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            };
            let ord = if self.sort_asc { ord } else { ord.reverse() };
            b.is_dir.cmp(&a.is_dir).then(ord)
        });
    }

    /// Click on a column header: same column flips direction, new column
    /// sorts ascending (except date/size which start descending — biggest
    /// and newest first is what you usually want).
    fn sort_by(&mut self, by: SortBy) {
        if self.sort == by {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort = by;
            self.sort_asc = matches!(by, SortBy::Name);
        }
        self.status = format!("sort: {}", self.sort_label());
        self.refresh();
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

    /// Marked files, or the current file if nothing is marked.
    fn selected_files(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .marked
            .iter()
            .filter(|p| p.is_file())
            .cloned()
            .collect();
        if paths.is_empty() {
            if let Some(e) = self.current().filter(|e| !e.is_up && !e.is_dir) {
                paths.push(e.path.clone());
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }

    fn toggle_mark_current(&mut self) {
        let Some(e) = self.current().filter(|e| !e.is_up) else {
            return;
        };
        let p = e.path.clone();
        if !self.marked.remove(&p) {
            self.marked.insert(p);
        }
        let n = self.marked.len();
        self.status = if n == 0 {
            "selection cleared".into()
        } else {
            format!("{n} selected")
        };
        self.move_sel(1);
    }

    fn mark_all_filtered(&mut self) {
        self.marked.clear();
        for &ei in &self.filtered {
            let e = &self.entries[ei];
            if !e.is_up {
                self.marked.insert(e.path.clone());
            }
        }
        self.status = format!("{} selected", self.marked.len());
    }

    fn clear_marks(&mut self) {
        self.marked.clear();
        self.status = "selection cleared".into();
    }

    /// Navigate to a new location, recording the old one for Back.
    fn set_cwd(&mut self, new: Option<PathBuf>) {
        if new == self.cwd {
            return;
        }
        self.back.push(self.cwd.clone());
        if self.back.len() > 64 {
            self.back.remove(0);
        }
        self.fwd.clear();
        self.cwd = new;
        self.filter.clear();
        self.marked.clear();
        self.refresh();
    }

    fn nav_back(&mut self) {
        let Some(prev) = self.back.pop() else {
            self.status = "no history".into();
            return;
        };
        self.fwd.push(self.cwd.clone());
        self.cwd = prev;
        self.filter.clear();
        self.marked.clear();
        self.refresh();
    }

    fn nav_fwd(&mut self) {
        let Some(next) = self.fwd.pop() else {
            self.status = "no forward history".into();
            return;
        };
        self.back.push(self.cwd.clone());
        self.cwd = next;
        self.filter.clear();
        self.marked.clear();
        self.refresh();
    }

    fn go_up(&mut self) {
        let parent = self.cwd.as_ref().and_then(|d| d.parent().map(|p| p.to_path_buf()));
        #[cfg(windows)]
        {
            self.set_cwd(parent);
        }
        #[cfg(not(windows))]
        {
            self.set_cwd(Some(parent.unwrap_or_else(|| PathBuf::from("/"))));
        }
    }

    fn go_home(&mut self) {
        if let Some(home) = dirs::home_dir() {
            self.set_cwd(Some(home));
        }
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
            let p = e.path.clone();
            self.set_cwd(Some(p));
            return Action::None;
        }
        if self.pick_path {
            return Action::PickPath(e.path.clone());
        }
        if self.pick_wallpaper {
            if crate::apps::wallpaper::is_image_path(&e.path) {
                return Action::SetWallpaperImageAndClose(e.path.clone());
            }
            self.status = "pick an image file (png/jpg/webp/…)".into();
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

    /// Rows: 0 toolbar+breadcrumbs · 1 column headers · list · last status.
    fn list_area(area: Rect, preview: bool) -> Rect {
        let w = if preview {
            area.width.saturating_sub(area.width / 3).max(20)
        } else {
            area.width
        };
        Rect::new(
            area.x,
            area.y + 2,
            w,
            area.height.saturating_sub(3),
        )
    }

    fn preview_area(area: Rect) -> Rect {
        let lw = area.width.saturating_sub(area.width / 3).max(20);
        Rect::new(
            area.x + lw,
            area.y + 2,
            area.width.saturating_sub(lw),
            area.height.saturating_sub(3),
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
                                " binary · {} · right-click → Hex\n\n{}",
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
                    self.set_cwd(Some(path));
                } else if path.is_file() {
                    if let Some(parent) = path.parent() {
                        self.set_cwd(Some(parent.to_path_buf()));
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

    fn paste_paths(&mut self, paths: &[PathBuf], cut: bool) {
        let Some(dir) = self.cwd.clone() else {
            self.status = "cannot paste here".into();
            return;
        };
        for src in paths {
            let Some(name) = src.file_name() else {
                continue;
            };
            let mut dest = dir.join(name);
            if dest.exists() {
                dest = unique_path(&dest);
            }
            let result = if src.is_dir() {
                copy_dir_all(src, &dest)
            } else {
                fs::copy(src, &dest).map(|_| ())
            };
            match result {
                Ok(()) => {
                    if cut {
                        let _ = if src.is_dir() {
                            fs::remove_dir_all(src)
                        } else {
                            fs::remove_file(src)
                        };
                    }
                    self.status = format!(
                        "{} → {}",
                        if cut { "moved" } else { "copied" },
                        dest.file_name().unwrap().to_string_lossy()
                    );
                }
                Err(err) => self.status = format!("drop failed: {err}"),
            }
        }
        self.refresh();
    }

    fn paste(&mut self) {
        let Some((op, src)) = self.clipboard.clone() else {
            self.status = "clipboard empty".into();
            return;
        };
        let cut = matches!(op, ClipOp::Cut);
        self.paste_paths(&[src], cut);
        if cut {
            self.clipboard = None;
        }
    }

    fn open_file_context(&mut self, col: u16, row: u16) {
        let mut items = Vec::new();
        let cur = self.current().map(|e| (e.is_up, e.is_dir));
        match cur {
            Some((true, _)) => {
                items.push(("Open parent".into(), FileMenuAction::Open));
            }
            Some((false, is_dir)) => {
                items.push(("Open".into(), FileMenuAction::Open));
                if !is_dir {
                    items.push(("Edit".into(), FileMenuAction::Edit));
                    items.push(("Hex".into(), FileMenuAction::Hex));
                }
                items.push(("Copy".into(), FileMenuAction::Copy));
                items.push(("Cut".into(), FileMenuAction::Cut));
                items.push(("Drag copy…".into(), FileMenuAction::DragCopy));
                items.push(("Drag cut…".into(), FileMenuAction::DragCut));
                items.push(("Pin to desktop".into(), FileMenuAction::PinToDesktop));
                if !is_dir {
                    if let Some(p) = self.current().map(|e| e.path.clone()) {
                        if crate::apps::wallpaper::is_image_path(&p) {
                            items.push(("Set as wallpaper".into(), FileMenuAction::SetAsWallpaper));
                        }
                    }
                }
                if self.git_root.is_some() {
                    items.push(("Git: stage".into(), FileMenuAction::GitStage));
                    items.push(("Git: unstage".into(), FileMenuAction::GitUnstage));
                    items.push(("Git: diff".into(), FileMenuAction::GitDiff));
                }
                let cur_path = self.current().map(|e| e.path.clone());
                let n_files = self.selected_files().len();
                let marks = self.marked.len();
                if (!is_dir && cur_path.is_some()) || (marks > 0 && n_files > 0) {
                    let label = if n_files > 1 {
                        format!("Create {n_files} snapshots")
                    } else {
                        "Create snapshot".into()
                    };
                    items.push((label, FileMenuAction::SnapshotCreate));
                }
                if !is_dir {
                    if let Some(ref p) = cur_path {
                        let hist = snaputil::count_for(p);
                        let label = if hist > 0 {
                            format!("Snapshot history ({hist})")
                        } else {
                            "Snapshot history".into()
                        };
                        items.push((label, FileMenuAction::SnapshotHistory));
                    }
                }
                if n_files >= 2 {
                    items.push((
                        format!("Compare {n_files} files"),
                        FileMenuAction::Compare,
                    ));
                }
                items.push(("Rename".into(), FileMenuAction::Rename));
                items.push(("Delete".into(), FileMenuAction::Delete));
            }
            None => {}
        }
        if self.clipboard.is_some() {
            items.push(("Paste".into(), FileMenuAction::Paste));
        }
        items.push(("New folder".into(), FileMenuAction::NewFolder));
        items.push(("New file".into(), FileMenuAction::NewFile));
        if self.cwd.is_some() {
            items.push(("Terminal here".into(), FileMenuAction::TerminalHere));
            items.push(("Run app here…".into(), FileMenuAction::RunHere));
        }
        if self.git_root.is_some() || self.cwd.is_some() {
            items.push(("Open Git here".into(), FileMenuAction::GitOpen));
        }
        items.push((
            if self.show_hidden {
                "Hide hidden files".into()
            } else {
                format!(
                    "Show hidden files{}",
                    if self.hidden_skipped > 0 {
                        format!(" ({})", self.hidden_skipped)
                    } else {
                        String::new()
                    }
                )
            },
            FileMenuAction::ToggleHidden,
        ));
        items.push(("Go to path…".into(), FileMenuAction::GotoPath));
        items.push(("Toggle preview".into(), FileMenuAction::TogglePreview));
        items.push(("Refresh".into(), FileMenuAction::Refresh));
        self.context = Some(FileContext {
            x: col,
            y: row,
            items,
            sel: 0,
        });
    }

    fn run_file_menu(&mut self, action: FileMenuAction) -> Action {
        match action {
            FileMenuAction::Open => return self.open_sel(),
            FileMenuAction::Edit => {
                if let Some(path) = self
                    .current()
                    .filter(|e| !e.is_dir && !e.is_up)
                    .map(|e| e.path.clone())
                {
                    return Action::Launch(Launch::Editor(Some(path)));
                }
            }
            FileMenuAction::Hex => {
                if let Some(path) = self
                    .current()
                    .filter(|e| !e.is_dir && !e.is_up)
                    .map(|e| e.path.clone())
                {
                    return Action::Launch(Launch::Hex(Some(path)));
                }
            }
            FileMenuAction::Copy => self.copy_sel(false),
            FileMenuAction::Cut => self.copy_sel(true),
            FileMenuAction::Paste => self.paste(),
            FileMenuAction::Rename => {
                if let Some((name, is_up)) = self.current().map(|e| (e.name.clone(), e.is_up)) {
                    if !is_up {
                        self.start_prompt(Prompt::Rename, name);
                    }
                }
            }
            FileMenuAction::Delete => {
                if let Some((name, is_up)) = self.current().map(|e| (e.name.clone(), e.is_up)) {
                    if !is_up {
                        self.start_prompt(Prompt::DeleteConfirm, String::new());
                        self.status = format!("delete {name}?");
                    }
                }
            }
            FileMenuAction::NewFolder => self.start_prompt(Prompt::Mkdir, String::new()),
            FileMenuAction::NewFile => self.start_prompt(Prompt::NewFile, String::new()),
            FileMenuAction::TerminalHere => {
                if let Some(dir) = self.cwd.clone() {
                    return Action::Launch(Launch::Terminal {
                        program: None,
                        args: Vec::new(),
                        cwd: Some(dir),
                    });
                }
            }
            FileMenuAction::RunHere => {
                if let Some(dir) = self.cwd.clone() {
                    return Action::Launch(Launch::RunHere {
                        dir,
                        files: self.selected_files(),
                    });
                }
            }
            FileMenuAction::Refresh => self.refresh(),
            FileMenuAction::TogglePreview => {
                self.preview = !self.preview;
                self.update_preview();
            }
            FileMenuAction::ToggleHidden => {
                self.show_hidden = !self.show_hidden;
                self.refresh();
            }
            FileMenuAction::GotoPath => {
                let initial = self
                    .cwd
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                self.start_prompt(Prompt::Goto, initial);
            }
            FileMenuAction::DragCopy | FileMenuAction::DragCut => {
                if let Some(e) = self.current().filter(|e| !e.is_up) {
                    return Action::BeginFileDrag {
                        path: e.path.clone(),
                        cut: matches!(action, FileMenuAction::DragCut),
                    };
                }
            }
            FileMenuAction::PinToDesktop => {
                if let Some(e) = self.current().filter(|e| !e.is_up) {
                    return Action::PinToDesktop(e.path.clone());
                }
            }
            FileMenuAction::SetAsWallpaper => {
                if let Some(e) = self.current().filter(|e| !e.is_up) {
                    return if self.pick_wallpaper {
                        Action::SetWallpaperImageAndClose(e.path.clone())
                    } else {
                        Action::SetWallpaperImage(e.path.clone())
                    };
                }
            }
            FileMenuAction::GitStage => {
                if let (Some(root), Some(e)) = (
                    self.git_root.clone(),
                    self.current().filter(|e| !e.is_up),
                ) {
                    match gitutil::stage(&root, &e.path) {
                        Ok(()) => {
                            self.status = format!("staged {}", e.name);
                            self.refresh();
                        }
                        Err(err) => self.status = err,
                    }
                }
            }
            FileMenuAction::GitUnstage => {
                if let (Some(root), Some(e)) = (
                    self.git_root.clone(),
                    self.current().filter(|e| !e.is_up),
                ) {
                    match gitutil::unstage(&root, &e.path) {
                        Ok(()) => {
                            self.status = format!("unstaged {}", e.name);
                            self.refresh();
                        }
                        Err(err) => self.status = err,
                    }
                }
            }
            FileMenuAction::GitDiff | FileMenuAction::GitOpen => {
                return Action::Launch(Launch::Git(self.git_root.clone().or(self.cwd.clone())));
            }
            FileMenuAction::SnapshotCreate => {
                let paths = self.selected_files();
                if paths.is_empty() {
                    self.status = "select a file first".into();
                } else {
                    let (_ok, _fail, msg) = snaputil::create_many(&paths);
                    self.status = msg;
                }
            }
            FileMenuAction::SnapshotHistory => {
                if let Some(path) = self
                    .current()
                    .filter(|e| !e.is_dir && !e.is_up)
                    .map(|e| e.path.clone())
                {
                    return Action::Launch(Launch::Snapshots(path));
                }
            }
            FileMenuAction::Compare => {
                let paths = self.selected_files();
                if paths.len() < 2 {
                    self.status = "tick 2+ files (click the ○ left of a name) then Compare".into();
                } else {
                    return Action::Launch(Launch::Compare(paths));
                }
            }
        }
        Action::None
    }

    fn render_context(&mut self, f: &mut Frame, area: Rect) {
        let Some(ctx) = &self.context else {
            return;
        };
        use unicode_width::UnicodeWidthStr;
        let w = ctx
            .items
            .iter()
            .map(|(l, _)| l.width() + 2)
            .max()
            .unwrap_or(12)
            .clamp(12, 36) as u16;
        let h = (ctx.items.len() as u16 + 2).min(area.height.max(3));
        let mut x = ctx.x.min(area.x + area.width.saturating_sub(w));
        let mut y = ctx.y.min(area.y + area.height.saturating_sub(h));
        if x < area.x {
            x = area.x;
        }
        if y < area.y {
            y = area.y;
        }
        let rect = Rect::new(x, y, w, h);
        f.render_widget(ratatui::widgets::Clear, rect);
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(160, 170, 200)))
            .style(Style::default().bg(Color::Rgb(18, 22, 38)));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let buf = f.buffer_mut();
        for (i, (label, _)) in ctx.items.iter().enumerate() {
            let iy = inner.y + i as u16;
            if iy >= inner.y + inner.height {
                break;
            }
            let style = if i == ctx.sel {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(0, 110, 170))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Rgb(210, 214, 228))
                    .bg(Color::Rgb(18, 22, 38))
            };
            let pad = " ".repeat(inner.width as usize);
            buf.set_string(inner.x, iy, &pad, style);
            buf.set_string(inner.x, iy, &format!(" {label}"), style);
        }
        // store adjusted rect via rewriting x/y for hit tests
        if let Some(c) = &mut self.context {
            c.x = rect.x;
            c.y = rect.y;
        }
    }

    fn context_hit(&self, col: u16, row: u16) -> Option<usize> {
        let ctx = self.context.as_ref()?;
        use unicode_width::UnicodeWidthStr;
        let w = ctx
            .items
            .iter()
            .map(|(l, _)| l.width() + 2)
            .max()
            .unwrap_or(12)
            .clamp(12, 28) as u16;
        let h = ctx.items.len() as u16 + 2;
        if col >= ctx.x && col < ctx.x + w && row > ctx.y && row < ctx.y + h - 1 {
            let idx = (row - ctx.y - 1) as usize;
            if idx < ctx.items.len() {
                return Some(idx);
            }
        }
        None
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
                self.set_cwd(Some(p));
            } else {
                self.status = "bookmark path missing".into();
            }
        }
    }

    /// Bottom row: the common file actions, or the active prompt's field.
    fn render_bar(&mut self, f: &mut Frame, row: Rect) {
        self.bar.begin();
        let buf = f.buffer_mut();
        if let Some((kind, input)) = &self.prompt {
            if matches!(kind, Prompt::DeleteConfirm) {
                let name = self.current().map(|e| e.name.clone()).unwrap_or_default();
                self.bar.toolbar(
                    buf,
                    row,
                    &[Item::Label(format!(" Delete {name}? This cannot be undone."), Color::White)],
                    &[
                        Item::with("🗑 Delete", Bar::PromptOk, Look::Danger),
                        Item::with("Cancel", Bar::PromptCancel, Look::Raised),
                    ],
                );
                return;
            }
            let (label, ok) = match kind {
                Prompt::Filter => ("Filter", "Apply"),
                Prompt::Rename => ("Rename to", "Rename"),
                Prompt::Mkdir => ("New folder", "Create"),
                Prompt::NewFile => ("New file", "Create"),
                _ => ("Go to", "Go"),
            };
            self.bar
                .input_row(buf, row, label, input, (ok, Bar::PromptOk), Bar::PromptCancel);
            return;
        }
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
        let marks = if self.marked.is_empty() {
            String::new()
        } else {
            format!(" · {} sel", self.marked.len())
        };
        let status = Item::Label(format!("{}{clip}{marks}", self.status), Color::Rgb(150, 158, 185));
        if self.pick_path {
            self.bar.toolbar(
                buf,
                row,
                &[Item::Label(String::new(), Color::Reset), status],
                &[
                    Item::with(
                        "📂 Use this folder",
                        Bar::UseFolder,
                        if self.cwd.is_some() { Look::Primary } else { Look::Disabled },
                    ),
                    Item::with("Cancel", Bar::Close, Look::Raised),
                ],
            );
            return;
        }
        if self.pick_wallpaper {
            let is_img = self.current().is_some_and(|e| !e.is_dir && super::wallpaper::is_image_path(&e.path));
            self.bar.toolbar(
                buf,
                row,
                &[Item::Label(String::new(), Color::Reset), status],
                &[
                    Item::with("🖼 Use this image", Bar::UseImage, if is_img { Look::Primary } else { Look::Disabled }),
                    Item::with("Cancel", Bar::Close, Look::Raised),
                ],
            );
            return;
        }
        let item = self.current().filter(|e| !e.is_up).is_some();
        let has = |b: bool| Look::enabled_if(b);
        let in_dir = self.cwd.is_some();
        let filter = if self.filter.is_empty() {
            Item::btn("🔍 Filter", Bar::Filter)
        } else {
            Item::with(format!("🔍 {} ✕", self.filter), Bar::ClearFilter, Look::On)
        };
        self.bar.toolbar(
            buf,
            row,
            &[
                Item::with("+ Folder", Bar::NewFolder, has(in_dir)),
                Item::with("+ File", Bar::NewFile, has(in_dir)),
                Item::Sep,
                // clipboard trio as icons: they are universal and the bar is tight
                Item::with("⧉", Bar::Copy, has(item || !self.marked.is_empty())),
                Item::with("✂", Bar::Cut, has(item || !self.marked.is_empty())),
                Item::with("📋", Bar::Paste, has(in_dir && self.clipboard.is_some())),
                Item::Sep,
                Item::with("✎ Rename", Bar::Rename, has(item)),
                Item::with("🗑 Delete", Bar::Delete, has(item)),
                Item::Sep,
                Item::btn("⋯ More", Bar::More),
                filter,
            ],
            &[status],
        );
    }

    fn run_bar(&mut self, b: Bar, col: u16, row: u16) -> Action {
        match b {
            Bar::NewFolder => return self.run_file_menu(FileMenuAction::NewFolder),
            Bar::NewFile => return self.run_file_menu(FileMenuAction::NewFile),
            Bar::Copy => return self.run_file_menu(FileMenuAction::Copy),
            Bar::Cut => return self.run_file_menu(FileMenuAction::Cut),
            Bar::Paste => return self.run_file_menu(FileMenuAction::Paste),
            Bar::Rename => return self.run_file_menu(FileMenuAction::Rename),
            Bar::Delete => return self.run_file_menu(FileMenuAction::Delete),
            Bar::Filter => self.start_prompt(Prompt::Filter, self.filter.clone()),
            Bar::ClearFilter => {
                self.filter.clear();
                self.apply_filter();
                self.status.clear();
            }
            Bar::More => self.open_file_context(col, row),
            Bar::UseImage => return self.open_sel(),
            Bar::UseFolder => {
                if let Some(dir) = self.cwd.clone() {
                    return Action::PickPath(dir);
                }
            }
            Bar::Close => return Action::Close,
            Bar::PromptOk => {
                if let Some((Prompt::DeleteConfirm, input)) = &mut self.prompt {
                    *input = "y".into();
                }
                return self.commit_prompt();
            }
            Bar::PromptCancel => {
                self.prompt = None;
                self.status = "cancelled".into();
            }
        }
        Action::None
    }

    /// Toolbar: nav buttons, clickable breadcrumb path, hidden/preview toggles.
    fn render_toolbar(&mut self, f: &mut Frame, area: Rect) {
        const BAR_BG: Color = Color::Rgb(28, 36, 62);
        let y = area.y;
        let buf = f.buffer_mut();
        for x in area.x..area.x + area.width {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(" ");
                c.set_bg(BAR_BG);
            }
        }
        self.toolbar_hits.clear();
        self.crumb_hits.clear();

        let on = Style::default()
            .fg(Color::White)
            .bg(Color::Rgb(0, 110, 170))
            .add_modifier(Modifier::BOLD);
        let live = Style::default().fg(Color::Rgb(210, 218, 240)).bg(BAR_BG);
        let dead = Style::default().fg(Color::Rgb(90, 100, 130)).bg(BAR_BG);
        let dim = Style::default().fg(Color::Rgb(90, 100, 130)).bg(BAR_BG);

        // right-aligned toggles first so we know how much room crumbs get
        let hid_label = " 👁.* ";
        let pre_label = " ▤ ";
        let hid_w = hid_label.width() as u16;
        let pre_w = pre_label.width() as u16;
        let pre_x = area.x + area.width.saturating_sub(pre_w);
        let hid_x = pre_x.saturating_sub(hid_w);
        buf.set_string(hid_x, y, hid_label, if self.show_hidden { on } else { live });
        buf.set_string(pre_x, y, pre_label, if self.preview { on } else { live });
        self.toolbar_hits.push((hid_x, hid_x + hid_w, ToolBtn::Hidden));
        self.toolbar_hits.push((pre_x, pre_x + pre_w, ToolBtn::Preview));

        let mut x = area.x;
        let buttons: [(&str, ToolBtn, bool); 5] = [
            (" ◂ ", ToolBtn::Back, !self.back.is_empty()),
            (" ▸ ", ToolBtn::Fwd, !self.fwd.is_empty()),
            (" ▴ ", ToolBtn::Up, self.cwd.is_some()),
            (" ⌂ ", ToolBtn::Home, true),
            (" ⟳ ", ToolBtn::Refresh, true),
        ];
        for (label, b, enabled) in buttons {
            let w = label.width() as u16;
            if x + w >= hid_x {
                break;
            }
            buf.set_string(x, y, label, if enabled { live } else { dead });
            self.toolbar_hits.push((x, x + w, b));
            x += w;
        }
        buf.set_string(x, y, "│", dim);
        x += 1;

        // breadcrumbs: (label, target); target None = drive list ("This PC")
        let mut crumbs: Vec<(String, Option<PathBuf>)> = Vec::new();
        #[cfg(windows)]
        crumbs.push(("💻".into(), None));
        match &self.cwd {
            None => {}
            Some(p) => {
                let mut acc = PathBuf::new();
                for comp in p.components() {
                    use std::path::Component;
                    acc.push(comp.as_os_str());
                    let label = match comp {
                        Component::Prefix(pr) => pr.as_os_str().to_string_lossy().to_string(),
                        Component::RootDir => {
                            if cfg!(windows) {
                                continue; // "C:" crumb already targets C:\
                            }
                            "/".to_string()
                        }
                        c => c.as_os_str().to_string_lossy().to_string(),
                    };
                    // make bare "C:" open "C:\" (bare prefix means cwd-relative)
                    let mut target = acc.clone();
                    if matches!(comp, Component::Prefix(_)) {
                        target.push("\\");
                    }
                    crumbs.push((label, Some(target)));
                }
            }
        }
        // drop leading crumbs (except the last) until the trail fits
        let avail = hid_x.saturating_sub(x + 1) as usize;
        let width_of = |c: &[(String, Option<PathBuf>)]| -> usize {
            c.iter().map(|(l, _)| l.width() + 3).sum::<usize>()
        };
        let mut start = 0usize;
        while crumbs.len() - start > 1 && width_of(&crumbs[start..]) + 2 > avail {
            start += 1;
        }
        if start > 0 {
            buf.set_string(x, y, " …", dim);
            x += 2;
        }
        for (i, (label, target)) in crumbs.iter().enumerate().skip(start) {
            let sep_w: u16 = if i > start { 3 } else { 1 };
            let w = label.width() as u16;
            if x + sep_w + w >= hid_x {
                break;
            }
            if i > start {
                buf.set_string(x, y, " ▸ ", dim);
            }
            x += sep_w;
            let is_last = i + 1 == crumbs.len();
            buf.set_string(
                x,
                y,
                label,
                if is_last {
                    Style::default()
                        .fg(Color::White)
                        .bg(BAR_BG)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Rgb(160, 190, 235)).bg(BAR_BG)
                },
            );
            self.crumb_hits.push((x, x + w, target.clone()));
            x += w;
        }
        if !self.filter.is_empty() {
            let chip = format!("  🔍{}", self.filter);
            if x + (chip.width() as u16) < hid_x {
                buf.set_string(
                    x,
                    y,
                    &chip,
                    Style::default().fg(Color::Rgb(255, 220, 120)).bg(BAR_BG),
                );
            }
        }
    }

    /// Clickable column headers with the active sort marked ▲ / ▼.
    fn render_header(&mut self, f: &mut Frame, area: Rect) {
        const HDR_BG: Color = Color::Rgb(20, 26, 44);
        let list = Self::list_area(area, self.preview);
        let y = area.y + 1;
        let buf = f.buffer_mut();
        for x in area.x..area.x + area.width {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(" ");
                c.set_bg(HDR_BG);
            }
        }
        self.header_hits.clear();
        let arrow = if self.sort_asc { "▲" } else { "▼" };
        let base = Style::default().fg(Color::Rgb(130, 145, 180)).bg(HDR_BG);
        let active = Style::default()
            .fg(Color::Rgb(120, 200, 255))
            .bg(HDR_BG)
            .add_modifier(Modifier::BOLD);
        let style_for = |b: SortBy, cur: SortBy| if b == cur { active } else { base };
        let label_for = |name: &str, b: SortBy, cur: SortBy| {
            if b == cur {
                format!("{name} {arrow}")
            } else {
                name.to_string()
            }
        };

        let meta_w = meta_width(list.width);
        let name_w = list.width.saturating_sub(4 + meta_w as u16) as usize;
        let name_x = list.x + 6;
        let date_x = list.x + 6 + name_w.saturating_sub(1) as u16;
        // Without the date column, Size sits where Modified would.
        let size_x = if meta_w == META_FULL { date_x + 11 } else { date_x };
        let max_x = area.x + area.width;

        let nl = label_for("Name", SortBy::Name, self.sort);
        if name_x + nl.width() as u16 <= max_x {
            buf.set_string(name_x, y, &nl, style_for(SortBy::Name, self.sort));
            self.header_hits
                .push((name_x, date_x.saturating_sub(1).max(name_x), SortBy::Name));
        }
        // meta columns only when the name column has real room
        if name_w < 10 || meta_w == 0 {
            return;
        }
        if meta_w == META_FULL && date_x + 11 <= max_x {
            let dl = label_for("Modified", SortBy::Modified, self.sort);
            buf.set_string(date_x, y, &dl, style_for(SortBy::Modified, self.sort));
            self.header_hits.push((date_x, size_x, SortBy::Modified));
        }
        if size_x + 8 <= max_x {
            let sl = label_for("Size", SortBy::Size, self.sort);
            let sx = size_x + 8u16.saturating_sub(sl.width() as u16);
            buf.set_string(sx, y, &sl, style_for(SortBy::Size, self.sort));
            self.header_hits.push((size_x, size_x + 9, SortBy::Size));
        }
    }

    /// Right-edge scrollbar; clicking / dragging it jumps proportionally.
    fn render_scrollbar(&mut self, f: &mut Frame, list: Rect) {
        self.scroll_rect = None;
        let total = self.filtered.len();
        let vis = list.height as usize;
        if total <= vis || vis == 0 || list.width < 8 {
            return;
        }
        let sx = list.x + list.width - 1;
        let track = Rect::new(sx, list.y, 1, list.height);
        self.scroll_rect = Some(track);
        let h = list.height as usize;
        let thumb_h = ((vis * h + total - 1) / total).clamp(1, h);
        let max_scroll = total - vis;
        let thumb_y = if max_scroll == 0 {
            0
        } else {
            (self.scroll * (h - thumb_h)) / max_scroll
        };
        let buf = f.buffer_mut();
        for i in 0..h {
            let cy = list.y + i as u16;
            let (sym, fg) = if i >= thumb_y && i < thumb_y + thumb_h {
                ("┃", Color::Rgb(120, 160, 220))
            } else {
                ("│", Color::Rgb(50, 58, 84))
            };
            if let Some(c) = buf.cell_mut((sx, cy)) {
                c.set_symbol(sym);
                c.set_fg(fg);
            }
        }
    }

    /// Jump the selection to the spot on the scrollbar track that was hit.
    fn scrollbar_jump(&mut self, row: u16) {
        let Some(track) = self.scroll_rect else {
            return;
        };
        let n = self.filtered.len();
        if n == 0 || track.height < 2 {
            return;
        }
        let frac = row.saturating_sub(track.y) as f32 / (track.height - 1) as f32;
        self.sel = ((frac * (n - 1) as f32).round() as usize).min(n - 1);
        self.update_preview();
    }

    fn fmt_mtime(t: Option<SystemTime>) -> String {
        let Some(t) = t else {
            return "        ".into();
        };
        let dt: chrono::DateTime<chrono::Local> = t.into();
        dt.format("%Y-%m-%d").to_string()
    }
}

/// Dotfiles everywhere; on Windows also the filesystem hidden attribute.
/// Columns after the name: git + date + size when the list is wide, size
/// alone when it is narrow (a docked explorer), nothing when it is tiny.
const META_FULL: usize = 24;

fn meta_width(list_w: u16) -> usize {
    match list_w {
        52.. => META_FULL,
        30.. => 9,
        _ => 0,
    }
}

/// Whether a watcher event can change what the list shows. Reads (our own
/// listing triggers those) and git's bookkeeping inside `.git` are ignored,
/// so refreshing — which runs `git status` — cannot feed itself.
fn is_listing_change(ev: &notify::Event) -> bool {
    if matches!(ev.kind, EventKind::Access(_)) {
        return false;
    }
    let in_git = |p: &PathBuf| p.components().any(|c| c.as_os_str() == ".git");
    ev.paths.is_empty() || !ev.paths.iter().all(in_git)
}

fn is_hidden(name: &str, md: Option<&fs::Metadata>) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if let Some(md) = md {
            if md.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
                return true;
            }
        }
    }
    #[cfg(not(windows))]
    let _ = md;
    false
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

/// Decode `%XX` escapes. Only used for `file://` pastes, which is the one
/// clipboard flavor that arrives percent-encoded.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |j: usize| (b[j] as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(i + 1), hex(i + 2)) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Normalize a path off the clipboard. Depending on where it was copied from a
/// path arrives bare, wrapped in quotes, shell-escaped (dragging a file into a
/// terminal), or as a `file://` URL — the nav bar takes all four.
fn clean_pasted_path(text: &str) -> String {
    let mut s = text.lines().next().unwrap_or("").trim().to_string();
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            s = s[1..s.len() - 1].to_string();
            break;
        }
    }
    if let Some(rest) = s.strip_prefix("file://") {
        // file:///Users/… — the empty host leaves the leading slash in place.
        s = percent_decode(rest);
    }
    // A terminal drag escapes spaces with a backslash. On Windows a backslash
    // is a separator, so leave it alone there.
    #[cfg(not(windows))]
    {
        s = s.replace("\\ ", " ");
    }
    s.trim().to_string()
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
    fn on_tick(&mut self) {
        if let Some(rx) = &self.watch_rx {
            if rx.try_iter().count() > 0 {
                self.changed_at = Some(Instant::now());
            }
        }
        // Debounce: a copy or a build touches many files at once.
        // A rename or new-folder prompt keeps the change waiting.
        if self.prompt.is_none()
            && self
                .changed_at
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(250))
        {
            self.changed_at = None;
            self.auto_refresh();
        }
    }

    fn title(&self) -> String {
        if self.pick_wallpaper {
            return "Choose wallpaper".into();
        }
        if self.pick_path {
            return "Open folder or file".into();
        }
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
        if area.height < 4 {
            return;
        }
        self.render_toolbar(f, area);
        self.render_header(f, area);

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
            let (icon, mut color) = if e.is_up {
                ("↩", Color::Rgb(140, 150, 180))
            } else {
                file_icon(&e.name, e.is_dir)
            };
            if e.hidden {
                // wash hidden entries out so they read as secondary
                color = Color::Rgb(110, 120, 145);
            }
            let size = if e.is_dir {
                "<DIR>".to_string()
            } else {
                gfx::fmt_size(e.size)
            };
            let date = Self::fmt_mtime(e.modified);
            let meta_w = meta_width(list.width);
            let name_w = list.width.saturating_sub(4 + meta_w as u16) as usize;
            let mut name = e.name.clone();
            if name.chars().count() > name_w {
                name = name.chars().take(name_w.saturating_sub(1)).collect::<String>() + "…";
            }
            let pad = name_w.saturating_sub(name.chars().count());
            let selected = row == self.sel;
            let marked = self.marked.contains(&e.path);
            let base = if selected && focused {
                Style::default().fg(Color::White).bg(Color::Rgb(0, 96, 150))
            } else if selected {
                Style::default().fg(Color::White).bg(Color::Rgb(48, 52, 74))
            } else if marked {
                Style::default().fg(color).bg(Color::Rgb(40, 48, 28))
            } else {
                Style::default().fg(color)
            };
            let git_ch = e.git.map(|g| g.as_char()).unwrap_or(' ');
            let git_fg = e
                .git
                .map(|g| g.color())
                .unwrap_or(Color::Rgb(60, 70, 90));
            // the mark column is clickable; the selected row shows its box
            let mark_ch = if marked {
                '●'
            } else if selected && !e.is_up {
                '○'
            } else {
                ' '
            };
            let sel_bg = if selected {
                if focused {
                    Color::Rgb(0, 96, 150)
                } else {
                    Color::Rgb(48, 52, 74)
                }
            } else if marked {
                Color::Rgb(40, 48, 28)
            } else {
                Color::Reset
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{mark_ch}{icon} "), base),
                Span::styled(
                    format!("{git_ch} "),
                    Style::default().fg(git_fg).bg(sel_bg),
                ),
                Span::styled(
                    format!("{}{} ", name, " ".repeat(pad.saturating_sub(2))),
                    if selected {
                        base.add_modifier(Modifier::BOLD)
                    } else {
                        base
                    },
                ),
                Span::styled(
                    if meta_w == META_FULL { format!("{date} ") } else { String::new() },
                    base.fg(if selected {
                        Color::White
                    } else {
                        Color::Rgb(100, 110, 135)
                    }),
                ),
                Span::styled(
                    if meta_w == 0 { String::new() } else { format!("{size:>8}") },
                    base.fg(if selected {
                        Color::White
                    } else {
                        Color::Rgb(120, 130, 155)
                    }),
                ),
            ]));
        }
        f.render_widget(Paragraph::new(lines), list);
        self.render_scrollbar(f, list);

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

        self.render_bar(f, Rect::new(area.x, area.y + area.height - 1, area.width, 1));

        if self.context.is_some() {
            self.render_context(f, area);
        }

    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        if self.context.is_some() {
            match k.code {
                KeyCode::Esc => self.context = None,
                KeyCode::Up => {
                    if let Some(ctx) = &mut self.context {
                        ctx.sel = ctx.sel.checked_sub(1).unwrap_or(ctx.items.len() - 1);
                    }
                }
                KeyCode::Down => {
                    if let Some(ctx) = &mut self.context {
                        let n = ctx.items.len();
                        ctx.sel = (ctx.sel + 1) % n.max(1);
                    }
                }
                KeyCode::Enter => {
                    if let Some(ctx) = self.context.take() {
                        if let Some((_, action)) = ctx.items.get(ctx.sel) {
                            return self.run_file_menu(*action);
                        }
                    }
                }
                _ => {}
            }
            return Action::None;
        }

        if let Some((Prompt::DeleteConfirm, _)) = self.prompt {
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    return self.run_bar(Bar::PromptOk, 0, 0);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    return self.run_bar(Bar::PromptCancel, 0, 0);
                }
                _ => return Action::None,
            }
        }
        if let Some((_, ref mut input)) = self.prompt {
            match k.code {
                KeyCode::Esc => {
                    self.prompt = None;
                    self.status = "cancelled".into();
                }
                KeyCode::Enter => return self.commit_prompt(),
                _ => {
                    super::text_edit(input, k);
                }
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
            KeyCode::Left if alt => self.nav_back(),
            KeyCode::Right if alt => self.nav_fwd(),
            KeyCode::Left => self.go_up(),
            KeyCode::Right => {
                // only descend into folders; files need Enter/double-click
                if self.current().map(|e| e.is_dir && !e.is_up).unwrap_or(false) {
                    return self.open_sel();
                }
            }
            KeyCode::Enter => return self.open_sel(),
            KeyCode::Backspace => self.go_up(),
            KeyCode::Esc | KeyCode::Char('q') if self.pick_wallpaper || self.pick_path => {
                return Action::Close
            }
            KeyCode::Esc if !self.marked.is_empty() => self.clear_marks(),
            KeyCode::Char(' ') if !ctrl && !alt => self.toggle_mark_current(),
            KeyCode::Char('a') if ctrl => self.mark_all_filtered(),
            KeyCode::Tab => {
                let next = match self.sort {
                    SortBy::Name => SortBy::Size,
                    SortBy::Size => SortBy::Modified,
                    SortBy::Modified => SortBy::Name,
                };
                self.sort = next;
                self.sort_asc = matches!(next, SortBy::Name);
                self.refresh();
                self.status = format!("sort: {}", self.sort_label());
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
            KeyCode::Char('C') if ctrl => {
                match self.current().filter(|e| !e.is_up).map(|e| e.path.clone()) {
                    Some(p) => {
                        crate::clipboard::copy(&p.to_string_lossy());
                        self.status = format!("copied path: {}", p.display());
                    }
                    None => self.status = "nothing selected".into(),
                }
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
            KeyCode::Char('g') if ctrl => {
                return Action::Launch(Launch::Git(self.git_root.clone().or(self.cwd.clone())));
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
        // The bottom bar holds the prompt's own OK / Cancel, so it stays live.
        if self.context.is_none() {
            if let Some(hit) = self.bar.route(&m) {
                return hit.map_or(Action::None, |b| self.run_bar(b, m.column, m.row));
            }
        }
        if self.prompt.is_some() {
            return Action::None;
        }

        if self.context.is_some() {
            match m.kind {
                MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::Down(MouseButton::Right) => {
                    if let Some(idx) = self.context_hit(m.column, m.row) {
                        let action = self.context.as_ref().unwrap().items[idx].1;
                        self.context = None;
                        return self.run_file_menu(action);
                    }
                    self.context = None;
                    if !matches!(m.kind, MouseEventKind::Down(MouseButton::Right)) {
                        return Action::None;
                    }
                }
                MouseEventKind::Moved => {
                    if let Some(idx) = self.context_hit(m.column, m.row) {
                        if let Some(ctx) = &mut self.context {
                            ctx.sel = idx;
                        }
                    }
                    return Action::None;
                }
                _ => return Action::None,
            }
        }

        let list = Self::list_area(area, self.preview);
        match m.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                if m.row >= list.y && m.row < list.y + list.height {
                    let idx = self.scroll + (m.row - list.y) as usize;
                    if idx < self.filtered.len() {
                        self.sel = idx;
                        self.update_preview();
                    }
                }
                self.open_file_context(m.column, m.row);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // toolbar buttons + breadcrumbs
                if m.row == area.y {
                    for (x0, x1, b) in self.toolbar_hits.clone() {
                        if m.column >= x0 && m.column < x1 {
                            match b {
                                ToolBtn::Back => self.nav_back(),
                                ToolBtn::Fwd => self.nav_fwd(),
                                ToolBtn::Up => self.go_up(),
                                ToolBtn::Home => self.go_home(),
                                ToolBtn::Refresh => self.refresh(),
                                ToolBtn::Hidden => {
                                    self.show_hidden = !self.show_hidden;
                                    self.refresh();
                                }
                                ToolBtn::Preview => {
                                    self.preview = !self.preview;
                                    self.update_preview();
                                }
                            }
                            return Action::None;
                        }
                    }
                    for (x0, x1, target) in self.crumb_hits.clone() {
                        if m.column >= x0 && m.column < x1 {
                            self.set_cwd(target);
                            return Action::None;
                        }
                    }
                    return Action::None;
                }
                // sortable column headers
                if m.row == area.y + 1 {
                    for (x0, x1, by) in self.header_hits.clone() {
                        if m.column >= x0 && m.column < x1 {
                            self.sort_by(by);
                            return Action::None;
                        }
                    }
                    return Action::None;
                }
                // scrollbar track
                if let Some(track) = self.scroll_rect {
                    if m.column == track.x
                        && m.row >= track.y
                        && m.row < track.y + track.height
                    {
                        self.scroll_drag = true;
                        self.scrollbar_jump(m.row);
                        return Action::None;
                    }
                }
                if m.row >= list.y && m.row < list.y + list.height {
                    let idx = self.scroll + (m.row - list.y) as usize;
                    if idx < self.filtered.len() {
                        self.sel = idx;
                        self.update_preview();
                        if m.modifiers.contains(KeyModifiers::CONTROL) || m.column == list.x {
                            if let Some(e) = self.current().filter(|e| !e.is_up) {
                                let p = e.path.clone();
                                if !self.marked.remove(&p) {
                                    self.marked.insert(p);
                                }
                                self.status = format!("{} selected", self.marked.len());
                            }
                            self.drag_from = None;
                            return Action::None;
                        }
                        let dbl = matches!(self.last_click, Some((t, li))
                            if li == idx && t.elapsed().as_millis() < 450);
                        if dbl {
                            self.last_click = None;
                            self.drag_from = None;
                            return self.open_sel();
                        }
                        self.last_click = Some((Instant::now(), idx));
                        self.drag_from = Some((idx, Instant::now()));
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.scroll_drag {
                    self.scrollbar_jump(m.row);
                    return Action::None;
                }
                if let Some((idx, started)) = self.drag_from {
                    // require a short hold/move before committing to drag
                    if started.elapsed().as_millis() > 120 && idx == self.sel {
                        if let Some(e) = self.current().filter(|e| !e.is_up) {
                            let path = e.path.clone();
                            self.drag_from = None;
                            return Action::BeginFileDrag {
                                path,
                                cut: false,
                            };
                        }
                    }
                }
            }
            MouseEventKind::Up(_) => {
                self.drag_from = None;
                self.scroll_drag = false;
            }
            MouseEventKind::ScrollUp => self.move_sel(-3),
            MouseEventKind::ScrollDown => self.move_sel(3),
            _ => {}
        }
        Action::None
    }

    fn on_drop(&mut self, paths: &[PathBuf], cut: bool) -> Action {
        self.paste_paths(paths, cut);
        Action::None
    }

    fn accepts_drop(&self) -> bool {
        self.cwd.is_some()
    }

    fn on_paste(&mut self, text: &str) -> Action {
        // An open prompt is a text field — the paste is just typing.
        if let Some((_, input)) = &mut self.prompt {
            input.push_str(text.lines().next().unwrap_or("").trim());
            return Action::None;
        }
        // Otherwise drop it into the nav bar, pre-filled and waiting on Enter,
        // rather than navigating somewhere the user cannot see first.
        let path = clean_pasted_path(text);
        if !path.is_empty() {
            self.start_prompt(Prompt::Goto, path);
        }
        Action::None
    }

    fn workspace_path(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_every_clipboard_flavor_of_a_path() {
        let want = "/Users/dan/My Docs";
        for pasted in [
            "/Users/dan/My Docs",
            "  /Users/dan/My Docs\n",
            "\"/Users/dan/My Docs\"",
            "'/Users/dan/My Docs'",
            "file:///Users/dan/My%20Docs",
            // multi-line clipboards (a file list) take the first entry
            "/Users/dan/My Docs\n/Users/dan/other",
        ] {
            assert_eq!(clean_pasted_path(pasted), want, "input: {pasted:?}");
        }
    }

    #[test]
    fn picks_up_files_created_outside() {
        let dir = std::env::temp_dir().join(format!("tt_watch_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut fb = FileBrowser::new(dir.clone());
        assert!(fb.watched.is_some(), "folder is watched");
        fs::write(dir.join("new.txt"), "hi").unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        let seen = loop {
            fb.on_tick();
            if fb.entries.iter().any(|e| e.name == "new.txt") {
                break true;
            }
            if Instant::now() > end {
                break false;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let _ = fs::remove_dir_all(&dir);
        assert!(seen, "the new file showed up without a manual refresh");
    }

    #[test]
    fn ignores_git_bookkeeping() {
        let ev = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(PathBuf::from("repo/.git/index"));
        assert!(!is_listing_change(&ev));
        let ev = notify::Event::new(EventKind::Create(notify::event::CreateKind::File))
            .add_path(PathBuf::from("repo/main.rs"));
        assert!(is_listing_change(&ev));
    }

    #[cfg(not(windows))]
    #[test]
    fn unescapes_terminal_drag_spaces() {
        assert_eq!(clean_pasted_path("/Users/dan/My\\ Docs"), "/Users/dan/My Docs");
    }

    #[cfg(windows)]
    #[test]
    fn keeps_windows_backslashes() {
        assert_eq!(clean_pasted_path(r"C:\Users\dan\My Docs"), r"C:\Users\dan\My Docs");
    }

    #[test]
    fn leaves_a_plain_relative_path_alone() {
        assert_eq!(clean_pasted_path("src/apps"), "src/apps");
        assert_eq!(clean_pasted_path("   "), "");
    }
}
