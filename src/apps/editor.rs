use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::scroll::ScrollState;
use super::snaputil::{self, CONFLICT_DISK, CONFLICT_MINE, CONFLICT_SPLIT};
use super::syntax::{self, HlState, Lang};
use super::ui::{self, Controls, Item, Look};
use super::{Action, AppWindow, Launch};

/// Everything the editor can do from its menus, toolbar and context menu.
#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    FileMenu,
    EditMenu,
    ViewMenu,
    New,
    Open,
    Save,
    SaveAs,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Highlight,
    PromptOk,
    PromptCancel,
    Find,
    Replace,
    FindNext,
    FindPrev,
    FindCase,
    FindToggleReplace,
    FindClose,
    FocusQuery,
    FocusReplace,
    ReplaceOne,
    ReplaceAll,
    ShowConflicts,
    UseDisk,
    KeepMine,
}

/// Which field of the find bar receives typing.
#[derive(Clone, Copy, PartialEq)]
enum FindField {
    Query,
    Replace,
}

/// The find / replace bar under the toolbar.
struct FindBar {
    query: String,
    replace: String,
    /// Show the second row with the replacement field.
    replacing: bool,
    /// Match case exactly.
    case: bool,
    field: FindField,
    /// Typing goes to the bar (false after a click into the text).
    focused: bool,
}

pub struct Editor {
    path: Option<PathBuf>,
    lines: Vec<String>,
    cx: usize,
    cy: usize,
    scroll: usize,
    modified: bool,
    status: String,
    lang: Lang,
    highlight: bool,
    /// Where a selection was started, as (line, column). The other end is the
    /// cursor, so moving the cursor grows or shrinks the selection.
    anchor: Option<(usize, usize)>,
    ui: Controls<Cmd>,
    /// Path being typed into the "Save as" field.
    save_as: Option<String>,
    /// Bound to one file (Notes): no New / Open / Save as.
    fixed: bool,
    /// Saved under a new path, so the session should reopen that one.
    renamed: bool,
    find: Option<FindBar>,
    /// Last left click (time, column, row, clicks in a row) for double and
    /// triple clicks.
    last_click: Option<(Instant, u16, u16, u8)>,
    /// Scroll the view to the cursor on the next frame. Cleared when the
    /// wheel or scrollbar moves the view on its own.
    follow: bool,
    bar: ScrollState,
    /// Horizontal scroll of the last frame, for mapping clicks to columns.
    hscroll: usize,
    /// The file as we last read or wrote it (newlines normalized), to tell our
    /// own saves from outside changes.
    disk_text: Option<String>,
    /// Outside change that clashes with unsaved edits: the version on disk.
    conflict: Option<String>,
    watcher: Option<notify::RecommendedWatcher>,
    watch_rx: Option<Receiver<()>>,
    changed_at: Option<Instant>,
}

fn bidx(s: &str, cx: usize) -> usize {
    s.char_indices().nth(cx).map(|(i, _)| i).unwrap_or(s.len())
}

/// Non-overlapping occurrences of `q` in `line`, as char ranges.
fn find_in(line: &[char], q: &[char], case: bool) -> Vec<(usize, usize)> {
    let same = |a: char, b: char| a == b || (!case && a.to_lowercase().eq(b.to_lowercase()));
    let mut out = Vec::new();
    let mut i = 0;
    while !q.is_empty() && i + q.len() <= line.len() {
        if (0..q.len()).all(|k| same(line[i + k], q[k])) {
            out.push((i, i + q.len()));
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Word characters, spaces and punctuation group separately for double-click
/// and Ctrl+arrow word movement.
fn char_class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

impl Editor {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut status = String::new();
        let lines = match &path {
            Some(p) => match fs::read(p) {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    if bytes.contains(&0) {
                        status = "⚠ binary file — editing not recommended".into();
                    }
                    text.replace("\r\n", "\n").split('\n').map(|s| s.to_string()).collect()
                }
                Err(e) => {
                    status = format!("✗ {e}");
                    vec![String::new()]
                }
            },
            None => vec![String::new()],
        };
        Editor::with_lines(path, lines, status)
    }

    /// Open an in-memory buffer bound to `path`; nothing is read from disk.
    /// Used for files that do not exist yet (see [`super::notes::Notes`]).
    pub fn with_lines(path: Option<PathBuf>, mut lines: Vec<String>, status: String) -> Self {
        // Cursor movement indexes `lines` directly, so it must never be empty.
        if lines.is_empty() {
            lines.push(String::new());
        }
        let lang = Lang::from_path(path.as_deref());
        let mut e = Editor {
            path,
            lines,
            cx: 0,
            cy: 0,
            scroll: 0,
            modified: false,
            status,
            lang,
            highlight: lang != Lang::Plain,
            anchor: None,
            ui: Controls::new(),
            save_as: None,
            fixed: false,
            renamed: false,
            find: None,
            last_click: None,
            follow: true,
            bar: ScrollState::default(),
            hscroll: 0,
            disk_text: None,
            conflict: None,
            watcher: None,
            watch_rx: None,
            changed_at: None,
        };
        e.disk_text = Some(e.lines.join("\n"));
        e.watch_file();
        e
    }

    /// Lock the editor to its file (Notes): the File menu only saves.
    pub fn fixed(mut self) -> Self {
        self.fixed = true;
        self
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    fn line_len(&self, y: usize) -> usize {
        self.lines.get(y).map(|l| l.chars().count()).unwrap_or(0)
    }

    fn save(&mut self) {
        match &self.path {
            None => self.begin_save_as(),
            Some(p) => match fs::write(p, self.lines.join("\n")) {
                Ok(_) => {
                    self.modified = false;
                    self.disk_text = Some(self.lines.join("\n"));
                    self.conflict = None;
                    self.lang = Lang::from_path(Some(p));
                    self.status = format!("saved {} lines ✓ · {}", self.lines.len(), self.lang.label());
                }
                Err(e) => self.status = format!("✗ save failed: {e}"),
            },
        }
    }

    fn begin_save_as(&mut self) {
        let start = match &self.path {
            Some(p) => p.clone(),
            None => std::env::current_dir()
                .unwrap_or_else(|_| super::state_home())
                .join("untitled.txt"),
        };
        self.save_as = Some(start.to_string_lossy().to_string());
    }

    fn finish_save_as(&mut self) {
        let Some(input) = self.save_as.take() else {
            return;
        };
        let input = input.trim();
        if input.is_empty() {
            return;
        }
        let p = PathBuf::from(input);
        if self.path.as_ref() != Some(&p) {
            self.renamed = true;
        }
        self.path = Some(p);
        self.save();
        self.watch_file();
    }

    fn paste(&mut self) {
        let text = crate::clipboard::paste();
        if text.is_empty() {
            self.status = "clipboard empty".into();
        } else {
            self.on_paste(&text);
            self.status = "pasted".into();
        }
    }

    fn toggle_highlight(&mut self) {
        self.highlight = !self.highlight;
        self.status = if self.highlight {
            format!("highlight on ({})", self.lang.label())
        } else {
            "highlight off".into()
        };
    }

    fn edit_items(&self) -> Vec<(String, Option<Cmd>)> {
        let sel = self.sel_range().is_some();
        vec![
            (if sel { "Cut" } else { "Cut line" }.into(), Some(Cmd::Cut)),
            (if sel { "Copy" } else { "Copy line" }.into(), Some(Cmd::Copy)),
            ("Paste".into(), Some(Cmd::Paste)),
            (String::new(), None),
            ("Select all".into(), Some(Cmd::SelectAll)),
            (String::new(), None),
            ("Find…".into(), Some(Cmd::Find)),
            ("Replace…".into(), Some(Cmd::Replace)),
            ("Find next".into(), Some(Cmd::FindNext)),
            ("Find previous".into(), Some(Cmd::FindPrev)),
        ]
    }

    fn run(&mut self, c: Cmd) -> Action {
        match c {
            Cmd::FileMenu => {
                let mut items = Vec::new();
                if !self.fixed {
                    items.push(("New file".into(), Some(Cmd::New)));
                    items.push(("Open…".into(), Some(Cmd::Open)));
                    items.push((String::new(), None));
                }
                items.push(("Save".into(), Some(Cmd::Save)));
                if !self.fixed {
                    items.push(("Save as…".into(), Some(Cmd::SaveAs)));
                }
                self.ui.open_menu_under(Cmd::FileMenu, items);
            }
            Cmd::EditMenu => {
                let items = self.edit_items();
                self.ui.open_menu_under(Cmd::EditMenu, items);
            }
            Cmd::ViewMenu => {
                let mark = if self.highlight { "☑" } else { "☐" };
                self.ui.open_menu_under(
                    Cmd::ViewMenu,
                    vec![
                        (format!("{mark} Syntax colors"), Some(Cmd::Highlight)),
                        (format!("  {}", self.lang.label()), None),
                    ],
                );
            }
            Cmd::New => return Action::Launch(Launch::Editor(None)),
            Cmd::Open => {
                let dir = self
                    .path
                    .as_ref()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(super::state_home);
                return Action::Launch(Launch::Files(dir));
            }
            Cmd::Save => self.save(),
            Cmd::SaveAs => self.begin_save_as(),
            Cmd::Cut => self.cut(),
            Cmd::Copy => self.copy(),
            Cmd::Paste => self.paste(),
            Cmd::SelectAll => self.select_all(),
            Cmd::Highlight => self.toggle_highlight(),
            Cmd::PromptOk => self.finish_save_as(),
            Cmd::PromptCancel => self.save_as = None,
            Cmd::Find => self.open_find(false),
            Cmd::Replace => self.open_find(true),
            Cmd::FindNext => self.find_step(true),
            Cmd::FindPrev => self.find_step(false),
            Cmd::FindCase => {
                if let Some(f) = &mut self.find {
                    f.case = !f.case;
                }
                self.find_live();
            }
            Cmd::FindToggleReplace => {
                if let Some(f) = &mut self.find {
                    f.replacing = !f.replacing;
                    f.field = if f.replacing { FindField::Replace } else { FindField::Query };
                    f.focused = true;
                }
            }
            Cmd::FindClose => self.find = None,
            Cmd::FocusQuery | Cmd::FocusReplace => {
                if let Some(f) = &mut self.find {
                    f.field = if c == Cmd::FocusQuery { FindField::Query } else { FindField::Replace };
                    f.focused = true;
                }
            }
            Cmd::ReplaceOne => self.replace_one(),
            Cmd::ReplaceAll => self.replace_all(),
            Cmd::ShowConflicts => self.show_conflicts(),
            Cmd::UseDisk => {
                if let Some(text) = self.conflict.take() {
                    self.reload_from(text);
                    self.status = "loaded the version on disk".into();
                }
            }
            Cmd::KeepMine => {
                if let Some(text) = self.conflict.take() {
                    // Saving now overwrites the outside change on purpose.
                    self.disk_text = Some(text);
                    self.status = "kept your version — Save to overwrite the file".into();
                }
            }
        }
        Action::None
    }

    // ── live reload ──────────────────────────────────────────────────────

    /// Watch the file's folder (atomic saves replace the file itself).
    fn watch_file(&mut self) {
        self.watcher = None;
        self.watch_rx = None;
        let Some(path) = self.path.clone().filter(|p| p.is_file()) else {
            return;
        };
        let Some(dir) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        let name = path.file_name().map(|n| n.to_os_string());
        let (tx, rx) = mpsc::channel();
        let made = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return;
                }
                if ev.paths.iter().any(|p| p.file_name().map(|n| n.to_os_string()) == name) {
                    let _ = tx.send(());
                }
            }
        });
        if let Ok(mut w) = made {
            if w.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
                self.watcher = Some(w);
                self.watch_rx = Some(rx);
            }
        }
    }

    /// Compare the file on disk with what we know of it: reload when we have
    /// no edits of our own, raise a conflict when we do.
    fn check_disk(&mut self) {
        let Some(p) = &self.path else { return };
        let Ok(bytes) = fs::read(p) else { return };
        let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
        if self.disk_text.as_deref() == Some(text.as_str()) {
            return;
        }
        if self.modified {
            self.conflict = Some(text);
        } else {
            self.reload_from(text);
            self.status = "reloaded — changed on disk".into();
        }
    }

    fn reload_from(&mut self, text: String) {
        self.lines = text.split('\n').map(String::from).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cy = self.cy.min(self.lines.len() - 1);
        self.cx = self.cx.min(self.line_len(self.cy));
        self.anchor = None;
        self.modified = false;
        self.disk_text = Some(text);
        self.conflict = None;
    }

    /// Put both versions in the buffer with conflict markers around every
    /// place they differ, for the user to resolve and save.
    fn show_conflicts(&mut self) {
        let Some(text) = self.conflict.take() else { return };
        let disk: Vec<String> = text.split('\n').map(String::from).collect();
        let (merged, n) = snaputil::conflict_merge(&self.lines, &disk);
        self.lines = merged;
        self.disk_text = Some(text);
        self.modified = true;
        self.anchor = None;
        if let Some(y) = self.lines.iter().position(|l| l == CONFLICT_MINE) {
            self.cy = y;
            self.cx = 0;
            self.follow = true;
        }
        self.status = format!(
            "{n} conflict{} marked — keep the part you want, delete the markers, Save",
            if n == 1 { "" } else { "s" }
        );
    }

    fn render_conflict(&mut self, buf: &mut ratatui::buffer::Buffer, row: Rect) {
        let bg = Color::Rgb(110, 70, 20);
        ui::fill_row(buf, row, bg);
        buf.set_string(
            row.x,
            row.y,
            " ⚠ Changed on disk while you were editing",
            Style::default().fg(Color::White).bg(bg).add_modifier(Modifier::BOLD),
        );
        let mut x = row.x + 44;
        for (label, id, look) in [
            ("Show conflicts", Cmd::ShowConflicts, Look::Primary),
            ("Use disk version", Cmd::UseDisk, Look::Raised),
            ("Keep mine", Cmd::KeepMine, Look::Raised),
        ] {
            if x + label.chars().count() as u16 + 2 > row.x + row.width {
                break;
            }
            x += self.ui.button(buf, x, row.y, label, id, look) + 1;
        }
    }

    // ── find / replace ───────────────────────────────────────────────────

    /// Open the bar (or focus it), seeded with a one-line selection.
    fn open_find(&mut self, replacing: bool) {
        let seed = self.selected_text().filter(|t| !t.contains('\n'));
        let bar = self.find.get_or_insert(FindBar {
            query: String::new(),
            replace: String::new(),
            replacing,
            case: false,
            field: FindField::Query,
            focused: true,
        });
        if let Some(t) = seed {
            bar.query = t;
        }
        bar.replacing |= replacing;
        bar.focused = true;
        bar.field = if replacing && !bar.query.is_empty() {
            FindField::Replace
        } else {
            FindField::Query
        };
        self.find_live();
    }

    /// Every match as (line, start column, end column), in order.
    fn matches(&self) -> Vec<(usize, usize, usize)> {
        let Some(f) = &self.find else { return Vec::new() };
        let q: Vec<char> = f.query.chars().collect();
        if q.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (y, line) in self.lines.iter().enumerate() {
            let l: Vec<char> = line.chars().collect();
            for (a, b) in find_in(&l, &q, f.case) {
                out.push((y, a, b));
            }
        }
        out
    }

    fn select_match(&mut self, m: (usize, usize, usize)) {
        self.anchor = Some((m.0, m.1));
        self.cy = m.0;
        self.cx = m.2;
        self.follow = true;
    }

    /// The match the selection covers, as an index into `ms`.
    fn current_match(&self, ms: &[(usize, usize, usize)]) -> Option<usize> {
        let ((sy, sx), (ey, ex)) = self.sel_range()?;
        ms.iter().position(|&(y, a, b)| y == sy && y == ey && a == sx && b == ex)
    }

    /// While typing a query: select the first match at or after the start of
    /// the current selection (or the cursor).
    fn find_live(&mut self) {
        let ms = self.matches();
        let from = self.sel_range().map(|r| r.0).unwrap_or((self.cy, self.cx));
        match ms.iter().find(|m| (m.0, m.1) >= from).or(ms.first()) {
            Some(&m) => {
                self.select_match(m);
                self.status.clear();
            }
            None => {
                if self.find.as_ref().is_some_and(|f| !f.query.is_empty()) {
                    self.status = "no matches".into();
                }
            }
        }
    }

    /// Next / previous match, wrapping around the file.
    fn find_step(&mut self, forward: bool) {
        if self.find.is_none() {
            self.open_find(false);
            return;
        }
        let ms = self.matches();
        if ms.is_empty() {
            self.status = "no matches".into();
            return;
        }
        let (start, end) = self
            .sel_range()
            .unwrap_or(((self.cy, self.cx), (self.cy, self.cx)));
        let pick = if forward {
            ms.iter()
                .find(|m| (m.0, m.1) >= end && (m.0, m.1) != start)
                .or(ms.first())
        } else {
            ms.iter().rev().find(|m| (m.0, m.1) < start).or(ms.last())
        };
        if let Some(&m) = pick {
            self.select_match(m);
            self.status.clear();
        }
    }

    /// Replace the selected match, then move on to the next one.
    fn replace_one(&mut self) {
        let ms = self.matches();
        let Some(i) = self.current_match(&ms) else {
            self.find_step(true);
            return;
        };
        let (y, a, b) = ms[i];
        let with = self.find.as_ref().map(|f| f.replace.clone()).unwrap_or_default();
        let line: Vec<char> = self.lines[y].chars().collect();
        let mut out: String = line[..a].iter().collect();
        out.push_str(&with);
        out.extend(&line[b..]);
        self.lines[y] = out;
        self.modified = true;
        self.anchor = None;
        self.cy = y;
        self.cx = a + with.chars().count();
        let rest = self.matches();
        match rest
            .iter()
            .find(|m| (m.0, m.1) >= (self.cy, self.cx))
            .or(rest.first())
        {
            Some(&m) => self.select_match(m),
            None => self.status = "replaced the last match".into(),
        }
    }

    fn replace_all(&mut self) {
        let Some(f) = &self.find else { return };
        let q: Vec<char> = f.query.chars().collect();
        if q.is_empty() {
            return;
        }
        let (case, with) = (f.case, f.replace.clone());
        let mut n = 0;
        for line in &mut self.lines {
            let l: Vec<char> = line.chars().collect();
            let hits = find_in(&l, &q, case);
            if hits.is_empty() {
                continue;
            }
            let mut out = String::new();
            let mut at = 0;
            for (a, b) in hits {
                out.extend(&l[at..a]);
                out.push_str(&with);
                at = b;
                n += 1;
            }
            out.extend(&l[at..]);
            *line = out;
        }
        self.anchor = None;
        self.cx = self.cx.min(self.line_len(self.cy));
        if n > 0 {
            self.modified = true;
        }
        self.status = format!("replaced {n}");
    }

    /// Rows between the toolbar and the text: conflict banner, find bar.
    fn extra_rows(&self) -> u16 {
        let find = match &self.find {
            Some(f) if f.replacing => 2,
            Some(_) => 1,
            None => 0,
        };
        find + u16::from(self.conflict.is_some())
    }

    /// The text area (line numbers + text + status line) inside `full`.
    fn text_area(&self, full: Rect) -> Rect {
        let top = 1 + self.extra_rows();
        Rect::new(full.x, full.y + top, full.width, full.height.saturating_sub(top))
    }

    fn render_find(&mut self, buf: &mut ratatui::buffer::Buffer, row: Rect) {
        let Some(f) = &self.find else { return };
        let ms = self.matches();
        let count = match (self.current_match(&ms), ms.len()) {
            (_, 0) if f.query.is_empty() => String::new(),
            (_, 0) => " no matches ".to_string(),
            (Some(i), n) => format!(" {} of {n} ", i + 1),
            (None, n) => format!(" {n} found "),
        };
        let (query, replace, replacing, case, field, focused) = (
            f.query.clone(),
            f.replace.clone(),
            f.replacing,
            f.case,
            f.field,
            f.focused,
        );
        let label_w = 10u16;
        let field_w = row.width.saturating_sub(label_w + 44).clamp(8, 40);
        let draw_field = |ui: &mut Controls<Cmd>,
                          buf: &mut ratatui::buffer::Buffer,
                          y: u16,
                          label: &str,
                          text: &str,
                          active: bool,
                          id: Cmd| {
            ui::fill_row(buf, Rect::new(row.x, y, row.width, 1), ui::BAR_BG);
            buf.set_string(row.x, y, format!(" {label}"), Style::default().fg(ui::DIM).bg(ui::BAR_BG));
            let r = Rect::new(row.x + label_w, y, field_w, 1);
            let bg = if active { Color::Rgb(8, 10, 18) } else { Color::Rgb(16, 20, 34) };
            ui::fill_row(buf, r, bg);
            let mut shown = text.to_string();
            if active {
                shown.push('▏');
            }
            let w = field_w as usize;
            let n = shown.chars().count();
            if n > w {
                shown = shown.chars().skip(n - w).collect();
            }
            buf.set_string(r.x, y, shown, Style::default().fg(Color::White).bg(bg));
            ui.add(r, id);
            row.x + label_w + field_w + 1
        };
        let x = draw_field(
            &mut self.ui,
            buf,
            row.y,
            "Find",
            &query,
            focused && field == FindField::Query,
            Cmd::FocusQuery,
        );
        buf.set_string(x, row.y, &count, Style::default().fg(ui::FG).bg(ui::BAR_BG));
        let mut bx = x + count.chars().count() as u16 + 1;
        for (label, id, look) in [
            ("Aa", Cmd::FindCase, Look::on_if(case)),
            ("◀", Cmd::FindPrev, Look::Raised),
            ("▶", Cmd::FindNext, Look::Raised),
            ("⇄ Replace", Cmd::FindToggleReplace, Look::on_if(replacing)),
            ("✕", Cmd::FindClose, Look::Plain),
        ] {
            if bx + label.chars().count() as u16 + 2 > row.x + row.width {
                break;
            }
            bx += self.ui.button(buf, bx, row.y, label, id, look) + 1;
        }
        if replacing {
            let y = row.y + 1;
            let x = draw_field(
                &mut self.ui,
                buf,
                y,
                "Replace",
                &replace,
                focused && field == FindField::Replace,
                Cmd::FocusReplace,
            );
            let mut bx = x;
            for (label, id) in [("Replace", Cmd::ReplaceOne), ("Replace all", Cmd::ReplaceAll)] {
                if bx + label.chars().count() as u16 + 2 > row.x + row.width {
                    break;
                }
                bx += self.ui.button(buf, bx, y, label, id, Look::Raised) + 1;
            }
        }
    }

    /// Keys while the find bar has focus. Returns false for keys the editor
    /// itself should handle.
    fn find_key(&mut self, k: KeyEvent) -> bool {
        let Some(f) = &mut self.find else { return false };
        if !f.focused {
            return false;
        }
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Esc => self.find = None,
            KeyCode::Enter if f.field == FindField::Replace => self.replace_one(),
            KeyCode::Enter => self.find_step(!shift),
            KeyCode::Tab | KeyCode::BackTab if f.replacing => {
                f.field = match f.field {
                    FindField::Query => FindField::Replace,
                    FindField::Replace => FindField::Query,
                };
            }
            _ => {
                let field = f.field;
                let buf = match field {
                    FindField::Query => &mut f.query,
                    FindField::Replace => &mut f.replace,
                };
                if !super::text_edit(buf, k) {
                    return false;
                }
                if field == FindField::Query {
                    self.find_live();
                }
            }
        }
        true
    }

    // ── selection helpers ────────────────────────────────────────────────

    /// Select the word (or run of spaces / punctuation) under the cursor.
    fn select_word(&mut self) {
        let l: Vec<char> = self.lines[self.cy].chars().collect();
        if l.is_empty() {
            return;
        }
        let at = self.cx.min(l.len() - 1);
        let class = char_class(l[at]);
        let mut a = at;
        while a > 0 && char_class(l[a - 1]) == class {
            a -= 1;
        }
        let mut b = at;
        while b < l.len() && char_class(l[b]) == class {
            b += 1;
        }
        self.anchor = Some((self.cy, a));
        self.cx = b;
    }

    /// Select the whole line, including its line break.
    fn select_line(&mut self) {
        self.anchor = Some((self.cy, 0));
        if self.cy + 1 < self.lines.len() {
            self.cy += 1;
            self.cx = 0;
        } else {
            self.cx = self.line_len(self.cy);
        }
    }

    /// Ctrl+Left: start of the previous word.
    fn word_left(&mut self) {
        if self.cx == 0 {
            if self.cy > 0 {
                self.cy -= 1;
                self.cx = self.line_len(self.cy);
            }
            return;
        }
        let l: Vec<char> = self.lines[self.cy].chars().collect();
        let mut x = self.cx.min(l.len());
        while x > 0 && l[x - 1].is_whitespace() {
            x -= 1;
        }
        if x > 0 {
            let class = char_class(l[x - 1]);
            while x > 0 && char_class(l[x - 1]) == class {
                x -= 1;
            }
        }
        self.cx = x;
    }

    /// Ctrl+Right: end of the current or next word.
    fn word_right(&mut self) {
        let l: Vec<char> = self.lines[self.cy].chars().collect();
        if self.cx >= l.len() {
            if self.cy + 1 < self.lines.len() {
                self.cy += 1;
                self.cx = 0;
            }
            return;
        }
        let mut x = self.cx;
        while x < l.len() && l[x].is_whitespace() {
            x += 1;
        }
        if x < l.len() {
            let class = char_class(l[x]);
            while x < l.len() && char_class(l[x]) == class {
                x += 1;
            }
        }
        self.cx = x;
    }

    /// Menu bar and quick buttons along the top row.
    fn render_bar(&mut self, f: &mut Frame, row: Rect) {
        let buf = f.buffer_mut();
        if let Some(input) = &self.save_as {
            self.ui
                .input_row(buf, row, "Save as", input, ("Save", Cmd::PromptOk), Cmd::PromptCancel);
            return;
        }
        let sel = self.sel_range().is_some();
        let save_look = if self.modified || self.path.is_none() {
            Look::Primary
        } else {
            Look::Plain
        };
        let left = [
            Item::btn("File", Cmd::FileMenu),
            Item::btn("Edit", Cmd::EditMenu),
            Item::btn("View", Cmd::ViewMenu),
            Item::Sep,
            Item::with("💾 Save", Cmd::Save, save_look),
            Item::Sep,
            Item::btn(if sel { "✂ Cut" } else { "✂" }, Cmd::Cut),
            Item::btn(if sel { "⧉ Copy" } else { "⧉" }, Cmd::Copy),
            Item::btn("📋", Cmd::Paste),
            Item::Sep,
            Item::with("🔍 Find", Cmd::Find, Look::on_if(self.find.is_some())),
        ];
        let right = [Item::with(
            if self.highlight { "🎨 Colors" } else { "🎨 Plain" },
            Cmd::Highlight,
            Look::on_if(self.highlight),
        )];
        self.ui.toolbar(buf, row, &left, &right);
    }

    /// Put the caret under the pointer. `gutter` wide line numbers sit left of
    /// the text, and the view is scrolled by `self.scroll` rows.
    fn move_to(&mut self, m: MouseEvent, area: Rect) {
        let row = if m.row < area.y {
            // Dragging above the text scrolls up a line at a time.
            self.scroll.saturating_sub(1)
        } else {
            (m.row - area.y) as usize + self.scroll
        };
        self.cy = row.min(self.lines.len().saturating_sub(1));
        let col = m.column.saturating_sub(area.x + 5) as usize + self.hscroll;
        self.cx = col.min(self.line_len(self.cy));
    }

    /// The selection as (start, end) with start <= end, or None when the
    /// anchor sits exactly on the cursor.
    fn sel_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.anchor?;
        let b = (self.cy, self.cx);
        if a == b {
            return None;
        }
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    fn selected_text(&self) -> Option<String> {
        let ((sy, sx), (ey, ex)) = self.sel_range()?;
        if sy == ey {
            let line = self.lines.get(sy)?;
            return Some(line.chars().skip(sx).take(ex - sx).collect());
        }
        let mut out: String = self.lines.get(sy)?.chars().skip(sx).collect();
        for line in self.lines.get(sy + 1..ey)? {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.extend(self.lines.get(ey)?.chars().take(ex));
        Some(out)
    }

    /// Remove the selected text and leave the cursor where it started.
    /// Returns false when there was no selection.
    fn delete_selection(&mut self) -> bool {
        let Some(((sy, sx), (ey, ex))) = self.sel_range() else {
            // An empty selection must not outlive the edit that follows: once
            // lines shift, a stale anchor can point past the end of the text.
            self.anchor = None;
            return false;
        };
        let head: String = self.lines[sy].chars().take(sx).collect();
        let tail: String = self.lines[ey].chars().skip(ex).collect();
        self.lines.splice(sy..=ey, [format!("{head}{tail}")]);
        self.cy = sy;
        self.cx = sx;
        self.anchor = None;
        self.modified = true;
        true
    }

    /// Copy the selection, or the whole cursor line when nothing is selected —
    /// the same convenience most editors give Ctrl+C.
    fn copy(&mut self) {
        let (text, what) = match self.selected_text() {
            Some(t) => {
                let n = t.lines().count();
                (t, if n > 1 { format!("{n} lines") } else { "selection".into() })
            }
            None => (
                format!("{}\n", self.lines[self.cy]),
                "line".to_string(),
            ),
        };
        crate::clipboard::copy(&text);
        self.status = format!("copied {what}");
    }

    fn cut(&mut self) {
        if self.sel_range().is_some() {
            self.copy();
            self.delete_selection();
            self.status = "cut selection".into();
        } else {
            crate::clipboard::copy(&format!("{}\n", self.lines[self.cy]));
            self.anchor = None;
            if self.lines.len() == 1 {
                self.lines[0].clear();
            } else {
                self.lines.remove(self.cy);
                self.cy = self.cy.min(self.lines.len() - 1);
            }
            self.cx = 0;
            self.modified = true;
            self.status = "cut line".into();
        }
    }

    fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.cy = self.lines.len() - 1;
        self.cx = self.line_len(self.cy);
    }

    /// Start, extend or drop the selection around a cursor move.
    /// Call before moving, with whether Shift was held.
    fn before_move(&mut self, extend: bool) {
        if extend {
            self.anchor.get_or_insert((self.cy, self.cx));
        } else {
            self.anchor = None;
        }
    }

    /// Paint the selection over an already-rendered text area.  Doing it on the
    /// buffer rather than in the spans keeps it independent of syntax
    /// highlighting, which has already split the line its own way.
    fn paint_selection(&self, f: &mut Frame, area: Rect, gutter: u16, hscroll: usize, text_h: usize) {
        let Some(((sy, sx), (ey, ex))) = self.sel_range() else {
            return;
        };
        let bg = Color::Rgb(46, 70, 116);
        for y in sy.max(self.scroll)..=ey.min(self.scroll + text_h.saturating_sub(1)) {
            let row = area.y + (y - self.scroll) as u16;
            let from = if y == sy { sx } else { 0 };
            // A selection running past a line's end shows one extra cell, so
            // the newline it swallowed is visible.
            let to = if y == ey { ex } else { self.line_len(y) + 1 };
            for col in from..to {
                let Some(vx) = col.checked_sub(hscroll) else {
                    continue;
                };
                let x = area.x + gutter + vx as u16;
                if x >= area.x + area.width {
                    break;
                }
                if let Some(c) = f.buffer_mut().cell_mut((x, row)) {
                    c.set_bg(bg);
                }
            }
        }
    }

    /// Tint every find match in view; the current one shows as the selection.
    fn paint_matches(&self, f: &mut Frame, area: Rect, gutter: u16, hscroll: usize, text_h: usize, text_w: u16) {
        if self.find.is_none() {
            return;
        }
        let bg = Color::Rgb(96, 82, 24);
        for (y, a, b) in self.matches() {
            if y < self.scroll || y >= self.scroll + text_h {
                continue;
            }
            let row = area.y + (y - self.scroll) as u16;
            for col in a..b {
                let Some(vx) = col.checked_sub(hscroll) else { continue };
                let x = area.x + gutter + vx as u16;
                if x >= area.x + text_w {
                    break;
                }
                if let Some(c) = f.buffer_mut().cell_mut((x, row)) {
                    c.set_bg(bg);
                }
            }
        }
    }

    /// Color conflict blocks: markers bold, your side green, disk side blue.
    fn paint_conflicts(&self, f: &mut Frame, area: Rect, text_h: usize, text_w: u16) {
        let mut side = 0u8; // 0 outside, 1 yours, 2 on disk
        for (y, line) in self.lines.iter().enumerate().take(self.scroll + text_h) {
            let marker = line == CONFLICT_MINE || line == CONFLICT_SPLIT || line == CONFLICT_DISK;
            let bg = if marker {
                Some(Color::Rgb(120, 60, 30))
            } else if side == 1 {
                Some(Color::Rgb(24, 52, 34))
            } else if side == 2 {
                Some(Color::Rgb(24, 38, 70))
            } else {
                None
            };
            if line == CONFLICT_MINE {
                side = 1;
            } else if line == CONFLICT_SPLIT && side == 1 {
                side = 2;
            } else if line == CONFLICT_DISK {
                side = 0;
            }
            let (Some(bg), true) = (bg, y >= self.scroll) else { continue };
            let row = area.y + (y - self.scroll) as u16;
            for x in area.x..area.x + text_w {
                if let Some(c) = f.buffer_mut().cell_mut((x, row)) {
                    c.set_bg(bg);
                }
            }
        }
    }

    fn paint_line(&self, line: &str, state: &mut HlState, hscroll: usize, content_w: usize) -> Vec<Span<'static>> {
        if !self.highlight || self.lang == Lang::Plain {
            let visible: String = line.chars().skip(hscroll).take(content_w).collect();
            return vec![Span::styled(
                visible,
                Style::default().fg(Color::Rgb(220, 222, 232)),
            )];
        }
        let spans = syntax::highlight_line(self.lang, line, state);
        syntax::slice_spans(spans, hscroll, content_w)
    }
}

impl AppWindow for Editor {
    fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".into());
        if self.modified {
            format!("{name} •")
        } else {
            name
        }
    }

    fn render(&mut self, f: &mut Frame, full: Rect, focused: bool) {
        if full.height < 3 {
            return;
        }
        self.ui.begin();
        self.render_bar(f, Rect::new(full.x, full.y, full.width, 1));
        let mut y = full.y + 1;
        if self.conflict.is_some() {
            self.render_conflict(f.buffer_mut(), Rect::new(full.x, y, full.width, 1));
            y += 1;
        }
        if self.find.is_some() {
            let rows = if self.find.as_ref().is_some_and(|f| f.replacing) { 2 } else { 1 };
            self.render_find(f.buffer_mut(), Rect::new(full.x, y, full.width, rows));
        }
        let area = self.text_area(full);
        if area.height < 2 {
            self.ui.render_menu(f.buffer_mut(), full);
            return;
        }
        let text_h = area.height as usize - 1;
        if self.follow {
            if self.cy < self.scroll {
                self.scroll = self.cy;
            }
            if self.cy >= self.scroll + text_h {
                self.scroll = self.cy + 1 - text_h;
            }
        }
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(1));
        // A scrollbar in the last column once the file is taller than the view.
        let overflow = self.lines.len() > text_h;
        let gutter = 5u16;
        let text_w = if overflow { area.width.saturating_sub(1) } else { area.width };
        let content_w = text_w.saturating_sub(gutter) as usize;
        let hscroll = self.cx.saturating_sub(content_w.saturating_sub(1));
        self.hscroll = hscroll;

        let mut state = if self.highlight {
            syntax::state_before(self.lang, &self.lines, self.scroll)
        } else {
            HlState::default()
        };

        let mut out: Vec<Line> = Vec::new();
        for (i, line) in self.lines.iter().enumerate().skip(self.scroll).take(text_h) {
            let num = Span::styled(
                format!("{:>4} ", i + 1),
                Style::default().fg(Color::Rgb(90, 95, 120)),
            );
            let mut body = self.paint_line(line, &mut state, hscroll, content_w);
            let mut spans = vec![num];
            spans.append(&mut body);
            out.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(out), Rect::new(area.x, area.y, text_w, text_h as u16));
        self.paint_conflicts(f, area, text_h, text_w);
        self.paint_matches(f, area, gutter, hscroll, text_h, text_w);
        self.paint_selection(f, Rect::new(area.x, area.y, text_w, area.height), gutter, hscroll, text_h);
        self.bar.offset = self.scroll.min(u16::MAX as usize) as u16;
        let bar = overflow.then(|| Rect::new(area.x + text_w, area.y, 1, text_h as u16));
        self.bar.show_bar(f.buffer_mut(), bar, self.lines.len(), text_h as u16);

        // cursor
        if focused {
            let cur_x = area.x + gutter + (self.cx - hscroll.min(self.cx)) as u16;
            let cur_y = area.y + self.cy.saturating_sub(self.scroll) as u16;
            let visible = self.cy >= self.scroll;
            if visible && cur_x < area.x + text_w && cur_y < area.y + text_h as u16 {
                if let Some(c) = f.buffer_mut().cell_mut((cur_x, cur_y)) {
                    c.modifier |= Modifier::REVERSED;
                }
            }
        }

        let hl = if self.highlight {
            self.lang.label()
        } else {
            "plain"
        };
        let status = format!(
            " {}  ·  Ln {}, Col {}  ·  {}{} ",
            if self.modified { "modified •" } else { "saved ✓" },
            self.cy + 1,
            self.cx + 1,
            hl,
            if self.status.is_empty() {
                String::new()
            } else {
                format!("  ·  {}", self.status)
            }
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                status,
                Style::default().fg(Color::Rgb(140, 150, 175)).bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
        self.ui.render_menu(f.buffer_mut(), full);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        // Terminals that forward the Command/Windows key report it as SUPER;
        // treat it as Ctrl so the platform-native chords work where they
        // survive the terminal emulator.
        let ctrl = k
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        // Shift with a movement key extends the selection instead of dropping it.
        let ext = k.modifiers.contains(KeyModifiers::SHIFT);
        if self.ui.menu_open() && k.code == KeyCode::Esc {
            self.ui.close_menu();
            return Action::None;
        }
        if let Some(input) = self.save_as.as_mut() {
            match k.code {
                KeyCode::Enter => self.finish_save_as(),
                KeyCode::Esc => self.save_as = None,
                _ => {
                    super::text_edit(input, k);
                }
            }
            return Action::None;
        }
        self.follow = true;
        match k.code {
            KeyCode::Char('f') if ctrl => {
                self.open_find(false);
                return Action::None;
            }
            KeyCode::Char('h') if ctrl => {
                self.open_find(true);
                return Action::None;
            }
            KeyCode::F(3) => {
                self.find_step(!ext);
                return Action::None;
            }
            _ => {}
        }
        if self.find_key(k) {
            return Action::None;
        }
        match k.code {
            KeyCode::Esc if self.find.is_some() => self.find = None,
            KeyCode::Left if ctrl => {
                self.before_move(ext);
                self.word_left();
            }
            KeyCode::Right if ctrl => {
                self.before_move(ext);
                self.word_right();
            }
            KeyCode::Home if ctrl => {
                self.before_move(ext);
                self.cy = 0;
                self.cx = 0;
            }
            KeyCode::End if ctrl => {
                self.before_move(ext);
                self.cy = self.lines.len() - 1;
                self.cx = self.line_len(self.cy);
            }
            KeyCode::Char('s') if ctrl => self.save(),
            KeyCode::Char('c') if ctrl => self.copy(),
            KeyCode::Char('x') if ctrl => self.cut(),
            KeyCode::Char('v') if ctrl => self.paste(),
            KeyCode::Char('a') if ctrl => self.select_all(),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.delete_selection();
                let i = bidx(&self.lines[self.cy], self.cx);
                self.lines[self.cy].insert(i, c);
                self.cx += 1;
                self.modified = true;
            }
            KeyCode::Tab => {
                self.delete_selection();
                let i = bidx(&self.lines[self.cy], self.cx);
                self.lines[self.cy].insert_str(i, "    ");
                self.cx += 4;
                self.modified = true;
            }
            KeyCode::Enter => {
                self.delete_selection();
                let i = bidx(&self.lines[self.cy], self.cx);
                let rest = self.lines[self.cy].split_off(i);
                self.lines.insert(self.cy + 1, rest);
                self.cy += 1;
                self.cx = 0;
                self.modified = true;
            }
            KeyCode::Backspace => {
                if self.delete_selection() {
                } else if self.cx > 0 {
                    let i = bidx(&self.lines[self.cy], self.cx - 1);
                    self.lines[self.cy].remove(i);
                    self.cx -= 1;
                    self.modified = true;
                } else if self.cy > 0 {
                    let cur = self.lines.remove(self.cy);
                    self.cy -= 1;
                    self.cx = self.line_len(self.cy);
                    self.lines[self.cy].push_str(&cur);
                    self.modified = true;
                }
            }
            KeyCode::Delete => {
                if self.delete_selection() {
                } else if self.cx < self.line_len(self.cy) {
                    let i = bidx(&self.lines[self.cy], self.cx);
                    self.lines[self.cy].remove(i);
                    self.modified = true;
                } else if self.cy + 1 < self.lines.len() {
                    let next = self.lines.remove(self.cy + 1);
                    self.lines[self.cy].push_str(&next);
                    self.modified = true;
                }
            }
            KeyCode::Left => {
                self.before_move(ext);
                if self.cx > 0 {
                    self.cx -= 1;
                } else if self.cy > 0 {
                    self.cy -= 1;
                    self.cx = self.line_len(self.cy);
                }
            }
            KeyCode::Right => {
                self.before_move(ext);
                if self.cx < self.line_len(self.cy) {
                    self.cx += 1;
                } else if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.cx = 0;
                }
            }
            KeyCode::Up => {
                self.before_move(ext);
                self.cy = self.cy.saturating_sub(1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::Down => {
                self.before_move(ext);
                self.cy = (self.cy + 1).min(self.lines.len() - 1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::PageUp => {
                self.before_move(ext);
                self.cy = self.cy.saturating_sub(20);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::PageDown => {
                self.before_move(ext);
                self.cy = (self.cy + 20).min(self.lines.len() - 1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::Home => {
                self.before_move(ext);
                self.cx = 0;
            }
            KeyCode::End => {
                self.before_move(ext);
                self.cx = self.line_len(self.cy);
            }
            _ => {}
        }
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        if text.is_empty() {
            return Action::None;
        }
        if let Some(input) = self.save_as.as_mut() {
            super::paste_line(input, text);
            return Action::None;
        }
        if let Some(f) = self.find.as_mut().filter(|f| f.focused) {
            let field = f.field;
            let buf = match field {
                FindField::Query => &mut f.query,
                FindField::Replace => &mut f.replace,
            };
            super::paste_line(buf, text);
            if field == FindField::Query {
                self.find_live();
            }
            return Action::None;
        }
        self.follow = true;
        self.delete_selection();
        // Split the current line at the cursor and stitch the paste in between.
        let at = bidx(&self.lines[self.cy], self.cx);
        let tail = self.lines[self.cy].split_off(at);
        let norm = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut parts = norm.split('\n');
        let first = parts.next().unwrap_or("");
        self.lines[self.cy].push_str(first);
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            self.cx += first.chars().count();
        } else {
            for (i, part) in rest.iter().enumerate() {
                self.lines.insert(self.cy + 1 + i, (*part).to_string());
            }
            self.cy += rest.len();
            self.cx = self.lines[self.cy].chars().count();
        }
        self.lines[self.cy].push_str(&tail);
        self.modified = true;
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, full: Rect) -> Action {
        if let Some(hit) = self.ui.route(&m) {
            return hit.map_or(Action::None, |c| self.run(c));
        }
        let area = self.text_area(full);
        if self.save_as.is_some() {
            return Action::None;
        }
        // Wheel and scrollbar move the view, not the cursor.
        let text_h = area.height.saturating_sub(1);
        self.bar.offset = self.scroll.min(u16::MAX as usize) as u16;
        if self.bar.on_mouse(m, self.lines.len(), text_h) {
            self.scroll = self.bar.offset as usize;
            self.follow = false;
            return Action::None;
        }
        // Presses start inside the text; a drag may wander above it.
        if m.row < area.y && matches!(m.kind, MouseEventKind::Down(_)) {
            return Action::None;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                // Keep an existing selection for Cut / Copy; otherwise move
                // the caret first, like a left click would.
                if self.sel_range().is_none() {
                    self.move_to(m, area);
                    self.anchor = None;
                }
                let items = self.edit_items();
                self.ui.open_menu(m.column, m.row, items);
            }
            MouseEventKind::Down(_) => {
                if let Some(f) = &mut self.find {
                    f.focused = false;
                }
                self.follow = true;
                let count = match self.last_click {
                    Some((t, x, y, n))
                        if t.elapsed() < Duration::from_millis(450) && x == m.column && y == m.row =>
                    {
                        n % 3 + 1
                    }
                    _ => 1,
                };
                self.last_click = Some((Instant::now(), m.column, m.row, count));
                if count == 1 && m.modifiers.contains(KeyModifiers::SHIFT) {
                    // Shift+click extends from the selection start, or the caret.
                    let keep = self.anchor.unwrap_or((self.cy, self.cx));
                    self.move_to(m, area);
                    self.anchor = Some(keep);
                    return Action::None;
                }
                self.anchor = None;
                self.move_to(m, area);
                match count {
                    2 => self.select_word(),
                    3 => self.select_line(),
                    // Anchor here so an immediate drag selects from the click.
                    _ => self.anchor = Some((self.cy, self.cx)),
                }
            }
            MouseEventKind::Drag(_) => {
                self.follow = true;
                self.move_to(m, area);
            }
            MouseEventKind::Up(_) => {
                // A click that never moved is a caret placement, not an empty
                // selection — clear the anchor so Ctrl+C still copies the line.
                if self.anchor == Some((self.cy, self.cx)) {
                    self.anchor = None;
                }
            }
            _ => {}
        }
        Action::None
    }

    fn on_tick(&mut self) {
        if let Some(rx) = &self.watch_rx {
            if rx.try_iter().count() > 0 {
                self.changed_at = Some(Instant::now());
            }
        }
        // Editors write in bursts (truncate, write, rename): wait for quiet.
        if self
            .changed_at
            .is_some_and(|t| t.elapsed() >= Duration::from_millis(200))
        {
            self.changed_at = None;
            self.check_disk();
        }
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    fn session_launch(&self) -> Option<Launch> {
        self.renamed.then(|| Launch::Editor(self.path.clone()))
    }

    fn on_drop(&mut self, paths: &[PathBuf], _cut: bool) -> Action {
        let Some(p) = paths.first() else {
            return Action::None;
        };
        if p.is_dir() {
            return Action::Launch(super::Launch::Files(p.clone()));
        }
        if self.modified {
            return Action::Launch(super::Launch::Editor(Some(p.clone())));
        }
        *self = Editor::new(Some(p.clone()));
        Action::None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn ed(lines: &[&str]) -> Editor {
        Editor::with_lines(
            None,
            lines.iter().map(|s| s.to_string()).collect(),
            String::new(),
        )
    }

    /// Select from (ay, ax) to (by, bx).
    fn select(e: &mut Editor, a: (usize, usize), b: (usize, usize)) {
        e.anchor = Some(a);
        e.cy = b.0;
        e.cx = b.1;
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn an_empty_selection_does_not_survive_an_edit() {
        // Shift+Down, Shift+Up leaves anchor == cursor on the last line; the
        // Backspace that joins it away must not leave the anchor dangling.
        let mut e = ed(&["one", ""]);
        select(&mut e, (1, 0), (1, 0));
        e.on_key(key(KeyCode::Backspace, KeyModifiers::NONE));
        e.on_key(key(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(e.lines, ["onex"]);
    }

    #[test]
    fn selection_reads_across_lines() {
        let mut e = ed(&["hello world", "second", "third line"]);
        select(&mut e, (0, 6), (0, 11));
        assert_eq!(e.selected_text().unwrap(), "world");

        select(&mut e, (0, 6), (2, 5));
        assert_eq!(e.selected_text().unwrap(), "world\nsecond\nthird");
    }

    #[test]
    fn a_backwards_selection_reads_the_same() {
        let mut e = ed(&["hello world", "second"]);
        select(&mut e, (1, 3), (0, 6));
        assert_eq!(e.selected_text().unwrap(), "world\nsec");
    }

    #[test]
    fn an_empty_selection_is_no_selection() {
        let mut e = ed(&["hello"]);
        select(&mut e, (0, 2), (0, 2));
        assert!(e.sel_range().is_none());
        assert!(e.selected_text().is_none());
    }

    #[test]
    fn deleting_a_selection_joins_the_ends() {
        let mut e = ed(&["hello world", "second", "third line"]);
        select(&mut e, (0, 6), (2, 6));
        assert!(e.delete_selection());
        assert_eq!(e.lines, vec!["hello line"]);
        assert_eq!((e.cy, e.cx), (0, 6));
        assert!(e.anchor.is_none());
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut e = ed(&["hello world"]);
        select(&mut e, (0, 6), (0, 11));
        e.on_key(key(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(e.lines, vec!["hello t"]);
    }

    #[test]
    fn pasting_over_a_selection_replaces_it() {
        let mut e = ed(&["hello world", "tail"]);
        select(&mut e, (0, 6), (1, 2));
        e.on_paste("there\nfriend");
        assert_eq!(e.lines, vec!["hello there", "friendil"]);
        assert_eq!((e.cy, e.cx), (1, 6));
    }

    #[test]
    fn pasting_a_single_line_stays_on_the_line() {
        let mut e = ed(&["ab"]);
        e.cx = 1;
        e.on_paste("XY");
        assert_eq!(e.lines, vec!["aXYb"]);
        assert_eq!((e.cy, e.cx), (0, 3));
    }

    #[test]
    fn shift_arrows_extend_and_plain_arrows_drop_the_selection() {
        let mut e = ed(&["hello"]);
        e.on_key(key(KeyCode::Right, KeyModifiers::SHIFT));
        e.on_key(key(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(e.selected_text().unwrap(), "he");
        e.on_key(key(KeyCode::Right, KeyModifiers::NONE));
        assert!(e.sel_range().is_none());
    }

    #[test]
    fn select_all_spans_the_buffer() {
        let mut e = ed(&["one", "two", "three"]);
        e.on_key(key(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(e.selected_text().unwrap(), "one\ntwo\nthree");
    }

    #[test]
    fn super_is_accepted_wherever_ctrl_is() {
        let mut e = ed(&["one", "two"]);
        e.on_key(key(KeyCode::Char('a'), KeyModifiers::SUPER));
        assert_eq!(e.selected_text().unwrap(), "one\ntwo");
    }

    #[test]
    fn backspace_with_a_selection_deletes_the_selection() {
        let mut e = ed(&["hello world"]);
        select(&mut e, (0, 5), (0, 11));
        e.on_key(key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(e.lines, vec!["hello"]);
    }

    fn ctrl(c: char) -> KeyEvent {
        key(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typ(e: &mut Editor, text: &str) {
        for c in text.chars() {
            e.on_key(key(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn ctrl_f_finds_and_steps_through_matches() {
        let mut e = ed(&["one two", "Two three two"]);
        e.on_key(ctrl('f'));
        typ(&mut e, "two");
        assert_eq!(e.selected_text().as_deref(), Some("two"));
        assert_eq!((e.cy, e.cx), (0, 7));
        e.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!((e.cy, e.cx), (1, 3), "case-insensitive by default");
        e.on_key(key(KeyCode::F(3), KeyModifiers::NONE));
        assert_eq!((e.cy, e.cx), (1, 13));
        e.on_key(key(KeyCode::F(3), KeyModifiers::NONE));
        assert_eq!((e.cy, e.cx), (0, 7), "wraps around");
        e.on_key(key(KeyCode::F(3), KeyModifiers::SHIFT));
        assert_eq!((e.cy, e.cx), (1, 13), "Shift+F3 goes back");
        assert_eq!(e.lines, ["one two", "Two three two"], "typing went to the bar");
    }

    #[test]
    fn match_case_skips_other_cases() {
        let mut e = ed(&["Two two"]);
        e.on_key(ctrl('f'));
        typ(&mut e, "two");
        e.run(Cmd::FindCase);
        assert_eq!(e.matches(), vec![(0, 4, 7)]);
    }

    #[test]
    fn replace_one_and_replace_all() {
        let mut e = ed(&["a cat, a cat", "cat"]);
        e.on_key(ctrl('h'));
        typ(&mut e, "cat");
        e.on_key(key(KeyCode::Tab, KeyModifiers::NONE));
        typ(&mut e, "dog");
        e.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(e.lines[0], "a dog, a cat");
        assert_eq!(e.selected_text().as_deref(), Some("cat"), "moves to the next match");
        e.run(Cmd::ReplaceAll);
        assert_eq!(e.lines, ["a dog, a dog", "dog"]);
        assert!(e.is_modified());
    }

    #[test]
    fn ctrl_arrows_jump_words_and_shift_extends() {
        let mut e = ed(&["let foo_bar = 1;"]);
        e.on_key(key(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(e.cx, 3);
        e.on_key(key(KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT));
        assert_eq!(e.selected_text().as_deref(), Some(" foo_bar"));
        e.on_key(key(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(e.cx, 4);
        assert!(e.sel_range().is_none());
    }

    #[test]
    fn double_click_selects_a_word_and_triple_click_the_line() {
        let mut e = ed(&["hello big world", "next"]);
        e.cx = 7;
        e.select_word();
        assert_eq!(e.selected_text().as_deref(), Some("big"));
        e.anchor = None;
        e.cy = 0;
        e.select_line();
        assert_eq!(e.selected_text().as_deref(), Some("hello big world\n"));
    }

    fn temp_file(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tt_edit_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.txt");
        fs::write(&p, text).unwrap();
        p
    }

    fn tick_until(e: &mut Editor, done: impl Fn(&Editor) -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            e.on_tick();
            if done(e) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn an_unedited_file_reloads_when_it_changes_on_disk() {
        let p = temp_file("reload", "first\n");
        let mut e = Editor::new(Some(p.clone()));
        fs::write(&p, "changed\nelsewhere\n").unwrap();
        assert!(tick_until(&mut e, |e| e.lines[0] == "changed"), "reloaded");
        assert!(!e.is_modified());
        assert!(e.conflict.is_none());
        let _ = fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn our_own_save_is_not_an_outside_change() {
        let p = temp_file("own", "a\n");
        let mut e = Editor::new(Some(p.clone()));
        typ(&mut e, "x");
        e.save();
        std::thread::sleep(Duration::from_millis(400));
        e.on_tick();
        std::thread::sleep(Duration::from_millis(250));
        e.on_tick();
        assert!(e.conflict.is_none());
        assert_eq!(e.lines[0], "xa");
        let _ = fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn edits_against_an_outside_change_show_conflicts() {
        let p = temp_file("conflict", "keep\nsame\nend");
        let mut e = Editor::new(Some(p.clone()));
        e.cy = 1;
        e.cx = 4;
        typ(&mut e, " mine");
        fs::write(&p, "keep\nsame theirs\nend").unwrap();
        assert!(tick_until(&mut e, |e| e.conflict.is_some()), "conflict raised");
        assert_eq!(e.lines[1], "same mine", "nothing overwritten yet");
        e.run(Cmd::ShowConflicts);
        assert_eq!(
            e.lines,
            ["keep", CONFLICT_MINE, "same mine", CONFLICT_SPLIT, "same theirs", CONFLICT_DISK, "end"]
        );
        assert!(e.conflict.is_none());
        let _ = fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn use_disk_version_drops_local_edits() {
        let p = temp_file("usedisk", "a");
        let mut e = Editor::new(Some(p.clone()));
        typ(&mut e, "local ");
        fs::write(&p, "remote").unwrap();
        assert!(tick_until(&mut e, |e| e.conflict.is_some()));
        e.run(Cmd::UseDisk);
        assert_eq!(e.lines, ["remote"]);
        assert!(!e.is_modified());
        let _ = fs::remove_dir_all(p.parent().unwrap());
    }
}
