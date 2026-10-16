//! App groups: "virtual apps" made of several frames. A group is a saved
//! arrangement of built-in apps docked around a tab area, all started in one
//! folder. The IDE is the built-in example: a file explorer on the left, a
//! terminal at the bottom, and every app opened from inside it lands in a tab.
//!
//! Definitions live in `<state home>/.texttop_groups.json` and are edited with
//! the App Groups settings window (`groupedit.rs`). A running group notices a
//! save through `generation()` and rearranges itself.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use ratatui::crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::pin::split_args;
use super::ui::{self, Controls, Item, Look};
use super::filebrowser::FileBrowser;
use super::{launch_for_path, state_home, Action, AppWindow, Launch};

// ── definitions ──────────────────────────────────────────────────────────────

/// Where a frame sits. Docked frames are carved off the window edges in list
/// order; `Tabs` frames open as the first tabs of the centre area.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dock {
    Left,
    Right,
    Top,
    Bottom,
    Tabs,
}

impl Dock {
    pub const ALL: [Dock; 5] = [Dock::Left, Dock::Right, Dock::Top, Dock::Bottom, Dock::Tabs];

    pub fn label(self) -> &'static str {
        match self {
            Dock::Left => "Left",
            Dock::Right => "Right",
            Dock::Top => "Top",
            Dock::Bottom => "Bottom",
            Dock::Tabs => "Tab",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Dock::Left => "left",
            Dock::Right => "right",
            Dock::Top => "top",
            Dock::Bottom => "bottom",
            Dock::Tabs => "tabs",
        }
    }

    fn from_key(s: &str) -> Dock {
        Dock::ALL
            .into_iter()
            .find(|d| d.key() == s)
            .unwrap_or(Dock::Tabs)
    }

    /// Whether the size counts columns (side docks) rather than rows.
    pub fn horizontal(self) -> bool {
        matches!(self, Dock::Left | Dock::Right)
    }
}

/// The apps a frame can hold.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameApp {
    Files,
    Terminal,
    Editor,
    Hex,
    Image,
    Audio,
    Video,
    Web,
    Git,
    Search,
    Notes,
    Processes,
    SysInfo,
    Calculator,
    Clock,
    Calendar,
    Studio,
}

/// (app, emoji, name, json key, what the parameter means — empty: none).
pub const FRAME_APPS: &[(FrameApp, &str, &str, &str, &str)] = &[
    (FrameApp::Files, "🗂", "File explorer", "files", "subfolder (optional)"),
    (FrameApp::Terminal, "🖥", "Terminal", "terminal", "command to run (optional)"),
    (FrameApp::Editor, "📝", "Text Editor", "editor", "file to open (optional)"),
    (FrameApp::Hex, "🔢", "Hex Viewer", "hex", "file to open (optional)"),
    (FrameApp::Image, "🖼", "Image Viewer", "image", "image file"),
    (FrameApp::Audio, "🎵", "Audio Player", "audio", "audio file"),
    (FrameApp::Video, "🎬", "Video Player", "video", "video file"),
    (FrameApp::Web, "🌐", "Web Browser", "web", "URL or file (optional)"),
    (FrameApp::Git, "🌿", "Git", "git", ""),
    (FrameApp::Search, "🔎", "Project Search", "search", ""),
    (FrameApp::Notes, "📌", "Notes", "notes", ""),
    (FrameApp::Processes, "⚙", "Processes", "processes", ""),
    (FrameApp::SysInfo, "💻", "System Info", "sysinfo", ""),
    (FrameApp::Calculator, "🧮", "Calculator", "calculator", ""),
    (FrameApp::Clock, "⏱", "Clock", "clock", ""),
    (FrameApp::Calendar, "📅", "Calendar", "calendar", ""),
    (FrameApp::Studio, "🎛", "Studio", "studio", "project file (optional)"),
];

impl FrameApp {
    fn row(self) -> &'static (FrameApp, &'static str, &'static str, &'static str, &'static str) {
        FRAME_APPS
            .iter()
            .find(|r| r.0 == self)
            .unwrap_or(&FRAME_APPS[0])
    }

    pub fn emoji(self) -> &'static str {
        self.row().1
    }

    pub fn name(self) -> &'static str {
        self.row().2
    }

    pub fn param_hint(self) -> &'static str {
        self.row().4
    }

    fn from_key(s: &str) -> Option<FrameApp> {
        FRAME_APPS.iter().find(|r| r.3 == s).map(|r| r.0)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct FrameDef {
    pub app: FrameApp,
    pub dock: Dock,
    /// Columns for left / right docks, rows for top / bottom.
    pub size: u16,
    /// Per-app parameter (see `FRAME_APPS`); relative paths resolve against
    /// the group's folder.
    pub param: String,
}

impl FrameDef {
    pub fn new(app: FrameApp, dock: Dock, size: u16) -> Self {
        FrameDef {
            app,
            dock,
            size,
            param: String::new(),
        }
    }

    /// The launch for this frame in `cwd`, or `None` when a required file
    /// parameter is missing.
    pub fn launch(&self, cwd: &Path) -> Option<Launch> {
        let param = self.param.trim();
        let file = || (!param.is_empty()).then(|| cwd.join(param));
        Some(match self.app {
            FrameApp::Files => Launch::Files(file().unwrap_or_else(|| cwd.to_path_buf())),
            FrameApp::Terminal => {
                let mut words = split_args(param).into_iter();
                Launch::Terminal {
                    program: words.next().map(PathBuf::from),
                    args: words.collect(),
                    cwd: Some(cwd.to_path_buf()),
                }
            }
            FrameApp::Editor => Launch::Editor(file()),
            FrameApp::Hex => Launch::Hex(file()),
            FrameApp::Image => Launch::Image(file()?),
            FrameApp::Audio => Launch::Audio(file()?),
            FrameApp::Video => Launch::Video(file()?),
            FrameApp::Web if param.contains("://") => Launch::Web(Some(param.to_string())),
            FrameApp::Web => Launch::Web(file().map(|p| p.to_string_lossy().to_string())),
            FrameApp::Git => Launch::Git(Some(cwd.to_path_buf())),
            FrameApp::Search => Launch::Search(Some(cwd.to_path_buf())),
            FrameApp::Notes => Launch::Notes,
            FrameApp::Processes => Launch::Processes,
            FrameApp::SysInfo => Launch::SysInfo,
            FrameApp::Calculator => Launch::Calculator,
            FrameApp::Clock => Launch::Clock,
            FrameApp::Calendar => Launch::Calendar,
            FrameApp::Studio => Launch::Studio(file()),
        })
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "app": self.app.row().3,
            "dock": self.dock.key(),
            "size": self.size,
            "param": self.param,
        })
    }

    fn from_json(v: &serde_json::Value) -> Option<Self> {
        Some(FrameDef {
            app: FrameApp::from_key(v.get("app")?.as_str()?)?,
            dock: Dock::from_key(v.get("dock").and_then(|d| d.as_str()).unwrap_or("tabs")),
            size: v.get("size").and_then(|s| s.as_u64()).unwrap_or(24).min(400) as u16,
            param: v
                .get("param")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct GroupDef {
    pub id: String,
    pub name: String,
    pub emoji: String,
    /// Start-menu section the group is listed under.
    pub section: String,
    /// Apps launched from inside the group open as tabs instead of windows.
    pub tabs: bool,
    pub frames: Vec<FrameDef>,
}

impl GroupDef {
    pub fn blank() -> Self {
        GroupDef {
            id: new_id(),
            name: "New group".into(),
            emoji: "🧱".into(),
            section: "App Groups".into(),
            tabs: true,
            frames: vec![FrameDef::new(FrameApp::Files, Dock::Left, 28)],
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "emoji": self.emoji,
            "section": self.section,
            "tabs": self.tabs,
            "frames": self.frames.iter().map(FrameDef::to_json).collect::<Vec<_>>(),
        })
    }

    fn from_json(v: &serde_json::Value) -> Option<Self> {
        let s = |k: &str, d: &str| {
            v.get(k)
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(d)
                .to_string()
        };
        Some(GroupDef {
            id: v.get("id")?.as_str()?.to_string(),
            name: s("name", "App group"),
            emoji: s("emoji", "🧱"),
            section: s("section", "App Groups"),
            tabs: v.get("tabs").and_then(|t| t.as_bool()).unwrap_or(true),
            frames: v
                .get("frames")
                .and_then(|f| f.as_array())
                .map(|a| a.iter().filter_map(FrameDef::from_json).collect())
                .unwrap_or_default(),
        })
    }
}

/// Id of the built-in IDE group.
pub const IDE: &str = "ide";

/// The IDE as shipped: explorer left, terminal bottom, tabs in the middle.
pub fn default_ide() -> GroupDef {
    GroupDef {
        id: IDE.into(),
        name: "IDE".into(),
        emoji: "🛠".into(),
        section: "Development".into(),
        tabs: true,
        frames: vec![
            FrameDef::new(FrameApp::Files, Dock::Left, 30),
            FrameDef::new(FrameApp::Terminal, Dock::Bottom, 10),
        ],
    }
}

pub fn new_id() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("group-{t}")
}

// ── storage ──────────────────────────────────────────────────────────────────

struct Store {
    home: PathBuf,
    groups: Vec<GroupDef>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn store_path(home: &Path) -> PathBuf {
    home.join(".texttop_groups.json")
}

fn load(home: &Path) -> Vec<GroupDef> {
    let Ok(text) = std::fs::read_to_string(store_path(home)) else {
        return vec![default_ide()];
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("groups")?
                .as_array()
                .map(|a| a.iter().filter_map(GroupDef::from_json).collect())
        })
        .unwrap_or_else(|| vec![default_ide()])
}

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let home = state_home();
    let mut guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.as_ref().is_none_or(|s| s.home != home) {
        let groups = load(&home);
        *guard = Some(Store { home, groups });
    }
    f(guard.as_mut().expect("store loaded"))
}

/// Every saved group, in menu order.
pub fn all() -> Vec<GroupDef> {
    with_store(|s| s.groups.clone())
}

/// One group by id. The IDE is always available, even after being deleted,
/// because Contribute opens it.
pub fn get(id: &str) -> Option<GroupDef> {
    all()
        .into_iter()
        .find(|g| g.id == id)
        .or_else(|| (id == IDE).then(default_ide))
}

/// Replace all definitions and tell running groups to pick them up.
pub fn save_all(groups: Vec<GroupDef>) -> Result<(), String> {
    with_store(|s| {
        let json = serde_json::json!({
            "groups": groups.iter().map(GroupDef::to_json).collect::<Vec<_>>(),
        });
        let text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
        std::fs::write(store_path(&s.home), text).map_err(|e| e.to_string())?;
        s.groups = groups;
        GENERATION.fetch_add(1, Ordering::Relaxed);
        Ok(())
    })
}

/// Bumped on every save.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

// ── layout ───────────────────────────────────────────────────────────────────

/// Smallest centre area the docks leave behind.
const MIN_W: u16 = 16;
const MIN_H: u16 = 4;

/// Carve docked frames off `area` in order. Returns each frame's rectangle
/// and its separator (zero-sized for `Tabs` or a frame squeezed out), plus
/// what is left for the tab area.
pub fn arrange(area: Rect, docks: &[(Dock, u16)]) -> (Vec<(Rect, Rect)>, Rect) {
    let mut r = area;
    let mut out = Vec::with_capacity(docks.len());
    for &(dock, size) in docks {
        let none = (Rect::default(), Rect::default());
        let room = if dock.horizontal() {
            r.width.saturating_sub(MIN_W + 1)
        } else {
            r.height.saturating_sub(MIN_H + 1)
        };
        let s = size.min(room);
        if s == 0 || dock == Dock::Tabs {
            out.push(none);
            continue;
        }
        let (pane, sep) = match dock {
            Dock::Left => {
                let pane = Rect::new(r.x, r.y, s, r.height);
                let sep = Rect::new(r.x + s, r.y, 1, r.height);
                r.x += s + 1;
                r.width -= s + 1;
                (pane, sep)
            }
            Dock::Right => {
                let pane = Rect::new(r.x + r.width - s, r.y, s, r.height);
                let sep = Rect::new(pane.x - 1, r.y, 1, r.height);
                r.width -= s + 1;
                (pane, sep)
            }
            Dock::Top => {
                let pane = Rect::new(r.x, r.y, r.width, s);
                let sep = Rect::new(r.x, r.y + s, r.width, 1);
                r.y += s + 1;
                r.height -= s + 1;
                (pane, sep)
            }
            Dock::Bottom => {
                let pane = Rect::new(r.x, r.y + r.height - s, r.width, s);
                let sep = Rect::new(r.x, pane.y - 1, r.width, 1);
                r.height -= s + 1;
                (pane, sep)
            }
            Dock::Tabs => unreachable!(),
        };
        out.push((pane, sep));
    }
    (out, r)
}

// ── the group window ─────────────────────────────────────────────────────────

const BG: Color = Color::Rgb(12, 14, 22);
const ACCENT: Color = Color::Rgb(0, 190, 255);
const HEAD_BG: Color = Color::Rgb(22, 28, 48);
const SEP: Color = Color::Rgb(54, 62, 92);

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Settings,
    Open,
    Tab(usize),
    CloseTab(usize),
    Dock(usize),
}

/// Which child an event or action belongs to.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Slot {
    Dock(usize),
    Tab(usize),
}

struct Pane {
    app: Box<dyn AppWindow>,
    icon: &'static str,
    dock: Dock,
    size: u16,
    /// Opened by the group definition (rather than from inside the group).
    from_def: bool,
}

#[derive(Default)]
struct Lay {
    /// Per dock: (frame, header row, body, separator).
    docks: Vec<(Rect, Rect, Rect, Rect)>,
    strip: Rect,
    body: Rect,
}

pub struct AppGroup {
    def: GroupDef,
    cwd: PathBuf,
    gen: u64,
    docks: Vec<Pane>,
    tabs: Vec<Pane>,
    active: usize,
    /// Focused dock, or `None` for the tab area.
    focus: Option<usize>,
    /// Child that got the last button press, so drags and releases follow it.
    capture: Option<Slot>,
    drag_sep: Option<usize>,
    lay: Lay,
    ui: Controls<Btn>,
    /// "Open folder/file" picker, drawn over the frames while open.
    picker: Option<Box<FileBrowser>>,
    picker_rect: Rect,
    status: String,
    /// Actions for the desktop that could not be returned straight away.
    pending: VecDeque<Action>,
    /// The folder changed since launch, so a restored session should use it.
    moved: bool,
}

impl AppGroup {
    pub fn new(id: String, cwd: Option<PathBuf>, open: Vec<PathBuf>) -> Self {
        let (def, status) = match get(&id) {
            Some(d) => (d, String::new()),
            None => (
                GroupDef {
                    id: id.clone(),
                    name: "Missing group".into(),
                    frames: Vec::new(),
                    ..GroupDef::blank()
                },
                format!("app group '{id}' no longer exists — open ⚙ Settings to make it"),
            ),
        };
        let cwd = cwd
            .filter(|p| p.is_dir())
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let mut g = AppGroup {
            def,
            cwd,
            gen: generation(),
            docks: Vec::new(),
            tabs: Vec::new(),
            active: 0,
            focus: None,
            capture: None,
            drag_sep: None,
            lay: Lay::default(),
            ui: Controls::new(),
            picker: None,
            picker_rect: Rect::default(),
            status,
            pending: VecDeque::new(),
            moved: false,
        };
        g.rebuild(false);
        for f in open {
            let a = g.open(launch_for_path(&f));
            g.forward(a);
        }
        g
    }

    /// Taskbar icon for the group's emoji. Window icons are `'static`; a group
    /// is opened rarely, so leaking its few bytes is fine.
    pub fn icon(&self) -> &'static str {
        Box::leak(self.def.emoji.clone().into_boxed_str())
    }

    /// (Re)start the frames of the definition in the current folder.
    fn rebuild(&mut self, keep_opened: bool) {
        self.docks.clear();
        if keep_opened {
            self.tabs.retain(|t| !t.from_def);
        } else {
            self.tabs.clear();
        }
        let mut def_tabs = Vec::new();
        for fr in &self.def.frames {
            let Some(launch) = fr.launch(&self.cwd) else {
                continue;
            };
            let Ok((app, icon, ..)) = crate::app::build_app(launch) else {
                continue;
            };
            let pane = Pane {
                app,
                icon,
                dock: fr.dock,
                size: fr.size,
                from_def: true,
            };
            if fr.dock == Dock::Tabs {
                def_tabs.push(pane);
            } else {
                self.docks.push(pane);
            }
        }
        self.tabs.splice(0..0, def_tabs);
        if !keep_opened {
            self.active = 0;
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        self.focus = if self.tabs.is_empty() && !self.docks.is_empty() {
            Some(0)
        } else {
            None
        };
        self.capture = None;
        self.drag_sep = None;
    }

    /// Run the group in another folder, or open a file in it.
    fn relaunch(&mut self, path: PathBuf) {
        let path = if path.is_absolute() {
            path
        } else {
            self.cwd.join(path)
        };
        if path.is_dir() {
            self.cwd = path;
            self.moved = true;
            self.rebuild(false);
            self.status.clear();
        } else if path.is_file() {
            if let Some(parent) = path.parent().filter(|p| *p != self.cwd) {
                self.cwd = parent.to_path_buf();
                self.moved = true;
                self.rebuild(false);
            }
            self.status.clear();
            let a = self.open(launch_for_path(&path));
            self.forward(a);
        } else {
            self.status = format!("not found: {}", path.display());
        }
    }

    /// Open a launch as a tab when the group collects them; otherwise (or
    /// when only the desktop can build it) pass it on.
    fn open(&mut self, launch: Launch) -> Action {
        if !self.def.tabs || matches!(launch, Launch::Group { .. }) {
            return Action::Launch(launch);
        }
        match crate::app::build_app(launch) {
            Ok((app, icon, ..)) => {
                self.tabs.push(Pane {
                    app,
                    icon,
                    dock: Dock::Tabs,
                    size: 0,
                    from_def: false,
                });
                self.active = self.tabs.len() - 1;
                self.focus = None;
                Action::None
            }
            Err(launch) => Action::Launch(launch),
        }
    }

    fn forward(&mut self, a: Action) {
        if !matches!(a, Action::None) {
            self.pending.push_back(a);
        }
    }

    fn remove(&mut self, slot: Slot) {
        match slot {
            Slot::Dock(i) if i < self.docks.len() => {
                self.docks.remove(i);
                self.focus = match self.focus {
                    Some(f) if f == i => None,
                    Some(f) if f > i => Some(f - 1),
                    f => f,
                };
            }
            Slot::Tab(i) if i < self.tabs.len() => {
                self.tabs.remove(i);
                if self.active > i || self.active >= self.tabs.len() {
                    self.active = self.active.saturating_sub(1);
                }
            }
            _ => {}
        }
        self.capture = None;
    }

    /// Interpret what a child asked for. Window-level requests (close, launch)
    /// apply to the child's frame; the rest go to the desktop.
    fn handle(&mut self, slot: Slot, action: Action) -> Action {
        match action {
            Action::None => Action::None,
            Action::Close => {
                self.remove(slot);
                Action::None
            }
            Action::Launch(l) => self.open(l),
            Action::LaunchAndClose(l) => {
                self.remove(slot);
                self.open(l)
            }
            Action::LaunchManyAndClose(ls) => {
                self.remove(slot);
                for l in ls {
                    let a = self.open(l);
                    self.forward(a);
                }
                self.pending.pop_front().unwrap_or(Action::None)
            }
            Action::SetThemeAndClose(t) => {
                self.remove(slot);
                Action::SetTheme(t)
            }
            Action::SetWallpaperImageAndClose(p) => {
                self.remove(slot);
                Action::SetWallpaperImage(p)
            }
            other => other,
        }
    }

    fn pane_mut(&mut self, slot: Slot) -> Option<&mut Pane> {
        match slot {
            Slot::Dock(i) => self.docks.get_mut(i),
            Slot::Tab(i) => self.tabs.get_mut(i),
        }
    }

    fn focused(&self) -> Option<Slot> {
        match self.focus {
            Some(i) if i < self.docks.len() => Some(Slot::Dock(i)),
            _ if !self.tabs.is_empty() => Some(Slot::Tab(self.active)),
            _ => None,
        }
    }

    fn body(&self, slot: Slot) -> Rect {
        match slot {
            Slot::Dock(i) => self.lay.docks.get(i).map(|d| d.2).unwrap_or_default(),
            Slot::Tab(_) => self.lay.body,
        }
    }

    fn slot_at(&self, col: u16, row: u16) -> Option<Slot> {
        let inside = |r: Rect| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height;
        if let Some(i) = self.lay.docks.iter().position(|d| inside(d.2)) {
            return Some(Slot::Dock(i));
        }
        (inside(self.lay.body) && !self.tabs.is_empty()).then_some(Slot::Tab(self.active))
    }

    fn press(&mut self, b: Btn) -> Action {
        match b {
            Btn::Settings => return Action::Launch(Launch::GroupEditor(Some(self.def.id.clone()))),
            Btn::Open => self.picker = Some(Box::new(FileBrowser::path_picker(self.cwd.clone()))),
            Btn::Tab(i) => {
                self.active = i.min(self.tabs.len().saturating_sub(1));
                self.focus = None;
            }
            Btn::CloseTab(i) => self.remove(Slot::Tab(i)),
            Btn::Dock(i) => self.focus = Some(i),
        }
        Action::None
    }

    /// What the Open folder/file picker answered.
    fn picked(&mut self, a: Action) -> Action {
        match a {
            Action::PickPath(p) => {
                self.picker = None;
                self.relaunch(p);
                self.pending.pop_front().unwrap_or(Action::None)
            }
            Action::Close => {
                self.picker = None;
                Action::None
            }
            Action::Launch(l) => self.open(l),
            Action::None => Action::None,
            other => other,
        }
    }

    /// The picker as a framed panel over the middle of the group.
    fn render_picker(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        let Some(p) = &mut self.picker else {
            self.picker_rect = Rect::default();
            return;
        };
        let w = area.width.saturating_sub(4).min(96);
        let h = area.height.saturating_sub(4).min(30);
        let r = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + 1 + (area.height.saturating_sub(1) - h) / 2,
            w,
            h,
        );
        self.picker_rect = r;
        f.render_widget(ratatui::widgets::Clear, r);
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(ACCENT))
            .title(" 📂 Open folder or file ")
            .style(Style::default().bg(BG));
        let inner = block.inner(r);
        f.render_widget(block, r);
        p.render(f, inner, focused);
    }

    fn render_strip(&mut self, buf: &mut ratatui::buffer::Buffer, strip: Rect) {
        ui::fill_row(buf, strip, ui::BAR_BG);
        if self.tabs.is_empty() {
            buf.set_string(
                strip.x + 1,
                strip.y,
                "Tabs",
                Style::default().fg(ui::DIM).bg(ui::BAR_BG),
            );
            return;
        }
        let labels: Vec<String> = self
            .tabs
            .iter()
            .map(|t| {
                let mut title = t.app.title();
                while title.width() > 24 {
                    title.pop();
                }
                format!(" {} {title} ", t.icon)
            })
            .collect();
        let widths: Vec<u16> = labels.iter().map(|l| l.width() as u16 + 3).collect();
        // Scroll the strip so the active tab is visible.
        let mut first = 0;
        while first < self.active
            && widths[first..=self.active].iter().sum::<u16>() > strip.width
        {
            first += 1;
        }
        let end = strip.x + strip.width;
        let mut x = strip.x;
        for i in first..self.tabs.len() {
            if x + widths[i] > end {
                break;
            }
            let on = i == self.active;
            let look = if on && self.focus.is_none() {
                Look::On
            } else if on {
                Look::Raised
            } else {
                Look::Plain
            };
            let w = self.ui.button(buf, x, strip.y, labels[i].trim(), Btn::Tab(i), look);
            x += w;
            let cw = self.ui.button(buf, x, strip.y, "✕", Btn::CloseTab(i), Look::Plain);
            x += cw.saturating_sub(1).max(1);
            buf.set_string(x, strip.y, "│", Style::default().fg(SEP).bg(ui::BAR_BG));
            x += 1;
        }
    }
}

impl AppWindow for AppGroup {
    fn title(&self) -> String {
        let folder = self
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.cwd.to_string_lossy().to_string());
        format!("{} · {folder}", self.def.name)
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        self.ui.begin();
        {
            let buf = f.buffer_mut();
            for y in area.y..area.y + area.height {
                ui::fill_row(buf, Rect::new(area.x, y, area.width, 1), BG);
            }
        }
        if area.height < 3 || area.width < 8 {
            return;
        }

        // Top bar: name and folder, then Open folder/file and Settings.
        let bar = Rect::new(area.x, area.y, area.width, 1);
        {
            let mut left = vec![Item::Label(
                format!(" {} {}", self.def.emoji, self.def.name),
                ACCENT,
            )];
            if self.status.is_empty() {
                left.push(Item::Label(self.cwd.to_string_lossy().to_string(), ui::DIM));
            } else {
                left.push(Item::Label(self.status.clone(), Color::Rgb(255, 170, 110)));
            }
            self.ui.toolbar(
                f.buffer_mut(),
                bar,
                &left,
                &[
                    Item::with("📂 Open folder/file", Btn::Open, Look::Raised),
                    Item::with("⚙ Settings", Btn::Settings, Look::Raised),
                ],
            );
        }

        // Frames.
        let work = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        let specs: Vec<(Dock, u16)> = self.docks.iter().map(|p| (p.dock, p.size)).collect();
        let (rects, rest) = arrange(work, &specs);
        self.lay.docks.clear();
        for (i, (pane, sep)) in rects.into_iter().enumerate() {
            let header = Rect::new(pane.x, pane.y, pane.width, pane.height.min(1));
            let body = Rect::new(
                pane.x,
                pane.y + 1,
                pane.width,
                pane.height.saturating_sub(1),
            );
            self.lay.docks.push((pane, header, body, sep));
            if pane.width == 0 {
                continue;
            }
            let is_focus = self.focus == Some(i);
            {
                let buf = f.buffer_mut();
                let bg = if is_focus { ui::ON_BG } else { HEAD_BG };
                ui::fill_row(buf, header, bg);
                let mut title = format!(" {} {}", self.docks[i].icon, self.docks[i].app.title());
                while title.width() > header.width as usize {
                    title.pop();
                }
                let mut st = Style::default().fg(ui::FG).bg(bg);
                if is_focus {
                    st = st.fg(Color::White).add_modifier(Modifier::BOLD);
                }
                buf.set_string(header.x, header.y, title, st);
                let hot = self.drag_sep == Some(i) || self.ui.hot(sep);
                let st = Style::default().fg(if hot { ACCENT } else { SEP }).bg(BG);
                let glyph = if self.docks[i].dock.horizontal() { "│" } else { "─" };
                for y in sep.y..sep.y + sep.height {
                    for x in sep.x..sep.x + sep.width {
                        buf.set_string(x, y, glyph, st);
                    }
                }
            }
            self.ui.add(header, Btn::Dock(i));
            if body.height > 0 {
                self.docks[i].app.render(f, body, focused && is_focus);
            }
        }

        // Tab area.
        let strip = Rect::new(rest.x, rest.y, rest.width, rest.height.min(1));
        let body = Rect::new(rest.x, rest.y + 1, rest.width, rest.height.saturating_sub(1));
        self.lay.strip = strip;
        self.lay.body = body;
        self.render_strip(f.buffer_mut(), strip);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if body.height > 0 {
                tab.app.render(f, body, focused && self.focus.is_none());
            }
        } else if body.height > 2 {
            let buf = f.buffer_mut();
            let lines = [
                "Nothing open yet".to_string(),
                String::new(),
                "Files and apps you open in here show up as tabs.".to_string(),
            ];
            let top = body.y + body.height / 2 - 1;
            for (k, l) in lines.iter().enumerate() {
                let x = body.x + body.width.saturating_sub(l.width() as u16) / 2;
                let fg = if k == 0 { ui::FG } else { ui::DIM };
                buf.set_string(x, top + k as u16, l, Style::default().fg(fg).bg(BG));
            }
        }
        self.render_picker(f, area, focused);
        self.ui.render_menu(f.buffer_mut(), area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        if let Some(p) = &mut self.picker {
            let a = p.on_key(k);
            return self.picked(a);
        }
        let Some(slot) = self.focused() else {
            return Action::None;
        };
        let a = match self.pane_mut(slot) {
            Some(p) => p.app.on_key(k),
            None => return Action::None,
        };
        self.handle(slot, a)
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        if let Some(p) = &mut self.picker {
            let r = self.picker_rect;
            let inside = m.column >= r.x
                && m.column < r.x + r.width
                && m.row >= r.y
                && m.row < r.y + r.height;
            if !inside {
                // A click beside the picker dismisses it.
                if matches!(m.kind, MouseEventKind::Down(_)) {
                    self.picker = None;
                }
                return Action::None;
            }
            let inner = Rect::new(r.x + 1, r.y + 1, r.width.saturating_sub(2), r.height.saturating_sub(2));
            let a = p.on_mouse(m, inner);
            return self.picked(a);
        }
        match self.ui.route(&m) {
            Some(Some(b)) => return self.press(b),
            Some(None) => return Action::None,
            None => {}
        }
        // Dragging a separator resizes its frame.
        if let Some(i) = self.drag_sep {
            match m.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    if let (Some(p), Some(&(pane, ..))) = (self.docks.get_mut(i), self.lay.docks.get(i)) {
                        let (c, r) = (m.column, m.row);
                        let s = match p.dock {
                            Dock::Left => c.saturating_sub(pane.x),
                            Dock::Right => (pane.x + pane.width).saturating_sub(c + 1),
                            Dock::Top => r.saturating_sub(pane.y),
                            Dock::Bottom => (pane.y + pane.height).saturating_sub(r + 1),
                            Dock::Tabs => p.size,
                        };
                        p.size = s.max(3);
                    }
                    return Action::None;
                }
                MouseEventKind::Up(_) => {
                    self.drag_sep = None;
                    return Action::None;
                }
                _ => {}
            }
        }
        if matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            let on_sep = self.lay.docks.iter().position(|d| {
                let s = d.3;
                m.column >= s.x && m.column < s.x + s.width && m.row >= s.y && m.row < s.y + s.height
            });
            if let Some(i) = on_sep {
                self.drag_sep = Some(i);
                return Action::None;
            }
        }
        let slot = match m.kind {
            MouseEventKind::Down(_) => {
                let s = self.slot_at(m.column, m.row);
                if let Some(s) = s {
                    self.capture = Some(s);
                    self.focus = match s {
                        Slot::Dock(i) => Some(i),
                        Slot::Tab(_) => None,
                    };
                }
                s
            }
            MouseEventKind::Drag(_) | MouseEventKind::Up(_) => {
                self.capture.or_else(|| self.slot_at(m.column, m.row))
            }
            _ => self.slot_at(m.column, m.row),
        };
        if matches!(m.kind, MouseEventKind::Up(_)) {
            self.capture = None;
        }
        let Some(slot) = slot else {
            return Action::None;
        };
        let rect = self.body(slot);
        let a = match self.pane_mut(slot) {
            Some(p) => p.app.on_mouse(m, rect),
            None => return Action::None,
        };
        self.handle(slot, a)
    }

    fn on_tick(&mut self) {
        for p in self.docks.iter_mut().chain(self.tabs.iter_mut()) {
            p.app.on_tick();
        }
        let gen = generation();
        if gen != self.gen {
            self.gen = gen;
            if let Some(def) = get(&self.def.id) {
                let rearrange = def.frames != self.def.frames;
                self.def = def;
                if rearrange {
                    self.rebuild(true);
                }
            }
        }
    }

    fn take_action(&mut self) -> Action {
        if let Some(a) = self.pending.pop_front() {
            return a;
        }
        for i in 0..self.docks.len() {
            let a = self.docks[i].app.take_action();
            if !matches!(a, Action::None) {
                return self.handle(Slot::Dock(i), a);
            }
        }
        for i in 0..self.tabs.len() {
            let a = self.tabs[i].app.take_action();
            if !matches!(a, Action::None) {
                return self.handle(Slot::Tab(i), a);
            }
        }
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        if let Some(p) = &mut self.picker {
            let a = p.on_paste(text);
            return self.picked(a);
        }
        let Some(slot) = self.focused() else {
            return Action::None;
        };
        let a = match self.pane_mut(slot) {
            Some(p) => p.app.on_paste(text),
            None => return Action::None,
        };
        self.handle(slot, a)
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    /// The focused frame takes the drop when it can; otherwise a folder
    /// relaunches the group there and files open as tabs.
    fn on_drop(&mut self, paths: &[PathBuf], cut: bool) -> Action {
        if let Some(slot) = self.focused() {
            if let Some(p) = self.pane_mut(slot).filter(|p| p.app.accepts_drop()) {
                let a = p.app.on_drop(paths, cut);
                return self.handle(slot, a);
            }
        }
        for p in paths {
            self.relaunch(p.clone());
        }
        self.pending.pop_front().unwrap_or(Action::None)
    }

    fn workspace_path(&self) -> Option<PathBuf> {
        Some(self.cwd.clone())
    }

    fn on_wallpaper_changed(&mut self, image: Option<PathBuf>, theme: usize) {
        for p in self.docks.iter_mut().chain(self.tabs.iter_mut()) {
            p.app.on_wallpaper_changed(image.clone(), theme);
        }
    }

    fn session_launch(&self) -> Option<Launch> {
        self.moved.then(|| Launch::Group {
            id: self.def.id.clone(),
            cwd: Some(self.cwd.clone()),
            open: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docks_are_carved_in_order_and_leave_the_centre() {
        let area = Rect::new(0, 0, 100, 40);
        let (r, rest) = arrange(area, &[(Dock::Left, 30), (Dock::Bottom, 10)]);
        assert_eq!(r[0].0, Rect::new(0, 0, 30, 40));
        assert_eq!(r[0].1, Rect::new(30, 0, 1, 40));
        assert_eq!(r[1].0, Rect::new(31, 30, 69, 10));
        assert_eq!(rest, Rect::new(31, 0, 69, 29));
    }

    #[test]
    fn oversized_docks_keep_a_usable_centre() {
        let (_, rest) = arrange(Rect::new(0, 0, 40, 20), &[(Dock::Left, 200)]);
        assert_eq!(rest.width, MIN_W);
    }

    #[test]
    fn frames_get_the_group_folder() {
        let cwd = Path::new("proj");
        let term = FrameDef {
            param: "cargo run".into(),
            ..FrameDef::new(FrameApp::Terminal, Dock::Bottom, 8)
        };
        match term.launch(cwd) {
            Some(Launch::Terminal { program, args, cwd }) => {
                assert_eq!(program, Some(PathBuf::from("cargo")));
                assert_eq!(args, vec!["run".to_string()]);
                assert_eq!(cwd.as_deref(), Some(Path::new("proj")));
            }
            _ => panic!("expected a terminal"),
        }
        let files = FrameDef::new(FrameApp::Files, Dock::Left, 20);
        assert!(matches!(files.launch(cwd), Some(Launch::Files(p)) if p == Path::new("proj")));
        assert!(FrameDef::new(FrameApp::Image, Dock::Tabs, 0).launch(cwd).is_none());
    }

    #[test]
    fn definitions_round_trip_through_json() {
        let mut g = default_ide();
        g.frames[1].param = "cargo watch".into();
        assert_eq!(GroupDef::from_json(&g.to_json()), Some(g));
    }
}
