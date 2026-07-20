use std::collections::HashSet;
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

use super::gitutil::{self, GitLetter};
use super::snaputil;
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
    Refresh,
    TogglePreview,
    DragCopy,
    DragCut,
    PinToDesktop,
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
    sort: SortBy,
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
            context: None,
            drag_from: None,
            git_root: None,
            git_summary: String::new(),
            marked: HashSet::new(),
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
                    size: 0,
                    modified: None,
                    git: None,
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
                                git: None,
                            });
                        }
                        self.sort_items(&mut items);
                        let n = items.len();
                        self.entries.extend(items);
                        self.status = format!("{n} items · sort:{}", self.sort_label());
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
            format!("{n} selected · Space toggle · Ctrl+A all · Esc clear")
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
        self.marked.clear();
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
            self.marked.clear();
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
        }
        if self.git_root.is_some() || self.cwd.is_some() {
            items.push(("Open Git here".into(), FileMenuAction::GitOpen));
        }
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
            FileMenuAction::Refresh => self.refresh(),
            FileMenuAction::TogglePreview => {
                self.preview = !self.preview;
                self.update_preview();
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
                    self.status = "mark 2+ files (Space) then Compare".into();
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
            let meta_w = 24usize; // git + date + size
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
            let mark_ch = if marked { '●' } else { ' ' };
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
                    format!("{date} "),
                    base.fg(if selected {
                        Color::White
                    } else {
                        Color::Rgb(100, 110, 135)
                    }),
                ),
                Span::styled(
                    format!("{size:>8}"),
                    base.fg(if selected {
                        Color::White
                    } else {
                        Color::Rgb(120, 130, 155)
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
            let marks = if self.marked.is_empty() {
                String::new()
            } else {
                format!(" · {} sel", self.marked.len())
            };
            format!(
                " {}{}{}  │ Space mark · right-click snapshot/compare · / filter · Ctrl+C/X/V ",
                self.status, clip, marks
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
            KeyCode::Esc if !self.marked.is_empty() => self.clear_marks(),
            KeyCode::Char(' ') if !ctrl && !alt => self.toggle_mark_current(),
            KeyCode::Char('a') if ctrl => self.mark_all_filtered(),
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
                if m.row >= list.y && m.row < list.y + list.height {
                    let idx = self.scroll + (m.row - list.y) as usize;
                    if idx < self.filtered.len() {
                        self.sel = idx;
                        self.update_preview();
                        if m.modifiers.contains(KeyModifiers::CONTROL) {
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

    fn workspace_path(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }
}
