use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::apps::{
    about::About, audioplayer::AudioPlayer, calculator::Calculator, calendar::Calendar,
    clock::ClockApp, compare::CompareApp, editor::Editor, filebrowser::FileBrowser,
    game2048::Game2048, git::GitApp, hexviewer::HexViewer, imageviewer::ImageViewer,
    launcher::AppLauncher, minesweeper::Minesweeper, notes::Notes, processes::Processes,
    search::SearchApp, snake::Snake, snapshots::SnapshotsApp, sysinfo::SysInfo,
    terminal::Terminal, videoplayer::VideoPlayer, webbrowser::WebBrowser, Action, AppWindow,
    Launch,
};
use crate::apps::gitutil;
use crate::gfx;

const TASKBAR_BG: Color = Color::Rgb(14, 16, 28);
const ACCENT: Color = Color::Rgb(0, 190, 255);
const MENU_BG: Color = Color::Rgb(18, 22, 38);
const WIN_BG: Color = Color::Rgb(12, 14, 22);

const THEMES: &[((u8, u8, u8), (u8, u8, u8), &str)] = &[
    ((10, 14, 44), (66, 14, 78), "midnight"),
    ((3, 30, 52), (0, 96, 116), "ocean"),
    ((34, 8, 10), (140, 66, 12), "sunset"),
    ((6, 30, 18), (22, 88, 54), "forest"),
    ((14, 6, 34), (0, 110, 100), "aurora"),
    ((44, 10, 64), (150, 40, 80), "vapor"),
    ((30, 30, 38), (8, 8, 12), "graphite"),
];

const ICON_W: u16 = 12;
const ICON_H: u16 = 3;
const ICON_GAP: u16 = 1;

struct Win {
    id: u64,
    rect: Rect,
    minimized: bool,
    restore: Option<Rect>,
    icon: &'static str,
    /// What spawned this window — lets us restore the session on next start.
    launch: Launch,
    app: Box<dyn AppWindow>,
}

enum Drag {
    Move { id: u64, dx: u16 },
    Resize { id: u64 },
    Icon {
        idx: usize,
        grab_x: u16,
        grab_y: u16,
        start_x: u16,
        start_y: u16,
    },
    File {
        path: PathBuf,
        cut: bool,
        from_win: Option<u64>,
        ghost_x: u16,
        ghost_y: u16,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum MenuAction {
    Files,
    Web,
    Terminal,
    Editor,
    Git,
    Search,
    Processes,
    Calculator,
    Clock,
    Calendar,
    Notes,
    Hex,
    SysInfo,
    Snake,
    Minesweeper,
    Game2048,
    Wallpaper,
    Keys,
    About,
    Quit,
}

impl MenuAction {
    fn as_str(self) -> &'static str {
        match self {
            MenuAction::Files => "Files",
            MenuAction::Web => "Web",
            MenuAction::Terminal => "Terminal",
            MenuAction::Editor => "Editor",
            MenuAction::Git => "Git",
            MenuAction::Search => "Search",
            MenuAction::Processes => "Processes",
            MenuAction::Calculator => "Calculator",
            MenuAction::Clock => "Clock",
            MenuAction::Calendar => "Calendar",
            MenuAction::Notes => "Notes",
            MenuAction::Hex => "Hex",
            MenuAction::SysInfo => "SysInfo",
            MenuAction::Snake => "Snake",
            MenuAction::Minesweeper => "Minesweeper",
            MenuAction::Game2048 => "Game2048",
            MenuAction::Wallpaper => "Wallpaper",
            MenuAction::Keys => "Keys",
            MenuAction::About => "About",
            MenuAction::Quit => "Quit",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "Files" => MenuAction::Files,
            "Web" => MenuAction::Web,
            "Terminal" => MenuAction::Terminal,
            "Editor" => MenuAction::Editor,
            "Git" => MenuAction::Git,
            "Search" => MenuAction::Search,
            "Processes" => MenuAction::Processes,
            "Calculator" => MenuAction::Calculator,
            "Clock" => MenuAction::Clock,
            "Calendar" => MenuAction::Calendar,
            "Notes" => MenuAction::Notes,
            "Hex" => MenuAction::Hex,
            "SysInfo" => MenuAction::SysInfo,
            "Snake" => MenuAction::Snake,
            "Minesweeper" => MenuAction::Minesweeper,
            "Game2048" => MenuAction::Game2048,
            "Wallpaper" => MenuAction::Wallpaper,
            "Keys" => MenuAction::Keys,
            "About" => MenuAction::About,
            "Quit" => MenuAction::Quit,
            _ => return None,
        })
    }
}

/// (label, action, group) — groups become section headers in the start menu.
const MENU: &[(&str, MenuAction, &str)] = &[
    ("🗂  File Manager", MenuAction::Files, "System"),
    ("🖥  Terminal", MenuAction::Terminal, "System"),
    ("⚙  Processes", MenuAction::Processes, "System"),
    ("💻  System Info", MenuAction::SysInfo, "System"),
    ("🌿  Git", MenuAction::Git, "Development"),
    ("🔎  Project Search", MenuAction::Search, "Development"),
    ("📝  Text Editor", MenuAction::Editor, "Development"),
    ("🔢  Hex Viewer", MenuAction::Hex, "Development"),
    ("🌐  Web Browser", MenuAction::Web, "Internet"),
    ("🧮  Calculator", MenuAction::Calculator, "Tools"),
    ("⏱  Clock & Timer", MenuAction::Clock, "Tools"),
    ("📅  Calendar", MenuAction::Calendar, "Tools"),
    ("📌  Notes", MenuAction::Notes, "Tools"),
    ("🐍  Snake", MenuAction::Snake, "Games"),
    ("💣  Minesweeper", MenuAction::Minesweeper, "Games"),
    ("🎮  2048", MenuAction::Game2048, "Games"),
    ("🎨  Wallpaper", MenuAction::Wallpaper, "Settings"),
    ("⌨  Shortcut Keys", MenuAction::Keys, "Settings"),
    ("✨  About & Help", MenuAction::About, "Settings"),
    ("⏻  Exit texttop", MenuAction::Quit, "Settings"),
];

const DEFAULT_ICONS: &[(&str, &str, MenuAction)] = &[
    ("🗂", "Files", MenuAction::Files),
    ("🌐", "Web", MenuAction::Web),
    ("🖥", "Terminal", MenuAction::Terminal),
    ("📝", "Editor", MenuAction::Editor),
    ("🌿", "Git", MenuAction::Git),
    ("🔎", "Search", MenuAction::Search),
    ("⚙", "Procs", MenuAction::Processes),
    ("📌", "Notes", MenuAction::Notes),
    ("🧮", "Calc", MenuAction::Calculator),
    ("⏱", "Clock", MenuAction::Clock),
    ("📅", "Calendar", MenuAction::Calendar),
    ("💻", "System", MenuAction::SysInfo),
    ("🔢", "Hex", MenuAction::Hex),
    ("🐍", "Snake", MenuAction::Snake),
    ("💣", "Mines", MenuAction::Minesweeper),
    ("🎮", "2048", MenuAction::Game2048),
    ("✨", "About", MenuAction::About),
];

/// Desktop icons per column before wrapping to the next one.
const ICONS_PER_COL: usize = 8;

#[derive(Clone)]
enum DeskKind {
    App(MenuAction),
    Shortcut(PathBuf),
}

#[derive(Clone)]
struct DeskIcon {
    emoji: String,
    label: String,
    kind: DeskKind,
    x: u16,
    y: u16,
}

#[derive(Clone)]
enum CtxAction {
    Open,
    OpenFiles,
    OpenTerminal,
    OpenEditor,
    Wallpaper,
    ShowDesktop,
    TileWindows,
    CascadeWindows,
    CloseAll,
    CloseWindow(u64),
    MinimizeWindow(u64),
    MaximizeWindow(u64),
    SnapLeft(u64),
    SnapRight(u64),
    RestoreWindow(u64),
    RemoveShortcut(usize),
    ResetIcons,
}

struct ContextMenu {
    items: Vec<(String, CtxAction)>,
    sel: usize,
    rect: Rect,
}

struct Switcher {
    sel: usize,
    ids: Vec<u64>,
}

pub struct Desktop {
    pub should_quit: bool,
    windows: Vec<Win>,
    next_id: u64,
    start_open: bool,
    start_sel: usize,
    /// Live text filter typed while the start menu is open.
    start_filter: String,
    show_keys: bool,
    icons: Vec<DeskIcon>,
    icon_sel: usize,
    last_click: Option<(Instant, i32)>,
    last_title_click: Option<(Instant, u64)>,
    drag: Option<Drag>,
    /// Icon click that may become a drag once the cursor moves.
    pending_icon: Option<(usize, u16, u16, u16, u16)>,
    context: Option<ContextMenu>,
    switcher: Option<Switcher>,
    /// Window ids we minimized for Ctrl+D show-desktop (restore only these).
    desk_restore: Option<Vec<u64>>,
    screen: Rect,
    menu_rect: Rect,
    start_w: u16,
    taskbar_hits: Vec<(u16, u16, u64)>,
    overflow_ids: Vec<u64>,
    overflow_btn: Option<(u16, u16)>,
    overflow_open: bool,
    overflow_rect: Rect,
    theme: usize,
    status: String,
    status_until: Option<Instant>,
    snap_preview: Option<Rect>,
    active_git_root: Option<PathBuf>,
    git_btn: Option<(u16, u16)>,
    git_label: String,
}

impl Desktop {
    pub fn new() -> Self {
        let mut d = Desktop {
            should_quit: false,
            windows: Vec::new(),
            next_id: 0,
            start_open: false,
            start_sel: 0,
            start_filter: String::new(),
            show_keys: false,
            icons: default_icons(),
            icon_sel: 0,
            last_click: None,
            last_title_click: None,
            drag: None,
            pending_icon: None,
            context: None,
            switcher: None,
            desk_restore: None,
            screen: Rect::new(0, 0, 120, 36),
            menu_rect: Rect::default(),
            start_w: 9,
            taskbar_hits: Vec::new(),
            overflow_ids: Vec::new(),
            overflow_btn: None,
            overflow_open: false,
            overflow_rect: Rect::default(),
            theme: 0,
            status: String::new(),
            status_until: None,
            snap_preview: None,
            active_git_root: None,
            git_btn: None,
            git_label: String::new(),
        };
        d.load_desktop();
        d.add_missing_default_apps();
        if !d.load_session() {
            d.spawn(Launch::About);
        }
        d
    }

    /// New releases add app icons; place any the saved desktop doesn't have
    /// yet into free slots without disturbing the user's layout.
    fn add_missing_default_apps(&mut self) {
        for (emoji, label, action) in DEFAULT_ICONS {
            let exists = self
                .icons
                .iter()
                .any(|i| matches!(&i.kind, DeskKind::App(a) if a == action));
            if !exists {
                let (x, y) = self.next_icon_slot();
                self.icons.push(DeskIcon {
                    emoji: (*emoji).to_string(),
                    label: (*label).to_string(),
                    kind: DeskKind::App(*action),
                    x,
                    y,
                });
            }
        }
    }

    // ---------------------------------------------------------- rendering

    pub fn render(&mut self, f: &mut Frame) {
        let area = f.area();
        self.screen = area;
        self.sanitize();

        let taskbar_y = area.height.saturating_sub(1);
        let desk = Rect::new(0, 0, area.width, taskbar_y);
        let th = THEMES[self.theme % THEMES.len()];

        {
            let buf = f.buffer_mut();
            gfx::gradient(buf, desk, th.0, th.1);

            // watermark
            if desk.width > 46 && desk.height > 12 {
                let wm = "· t e x t t o p ·";
                let sub = "your terminal, but cozier";
                let x = desk.width.saturating_sub(wm.len() as u16) / 2;
                let y = desk.height / 2;
                buf.set_string(
                    x,
                    y,
                    wm,
                    Style::default()
                        .fg(Color::Rgb(
                            gfx::lerp(th.0 .0, 255, 0.30),
                            gfx::lerp(th.0 .1, 255, 0.30),
                            gfx::lerp(th.0 .2, 255, 0.30),
                        ))
                        .add_modifier(Modifier::BOLD),
                );
                let x2 = desk.width.saturating_sub(sub.len() as u16) / 2;
                buf.set_string(
                    x2,
                    y + 1,
                    sub,
                    Style::default().fg(Color::Rgb(
                        gfx::lerp(th.0 .0, 255, 0.18),
                        gfx::lerp(th.0 .1, 255, 0.18),
                        gfx::lerp(th.0 .2, 255, 0.18),
                    )),
                );
            }

            // desktop icons
            for (i, icon) in self.icons.iter().enumerate() {
                let r = icon_rect(icon);
                if r.y + r.height > desk.height || r.x + r.width > desk.width {
                    continue;
                }
                let selected = i == self.icon_sel && self.top_idx().is_none() && self.context.is_none();
                if selected {
                    for y in r.y..r.y + r.height {
                        for x in r.x..r.x + r.width {
                            if let Some(c) = buf.cell_mut((x, y)) {
                                c.set_bg(Color::Rgb(50, 70, 110));
                            }
                        }
                    }
                }
                let ex = r.x + r.width.saturating_sub(icon.emoji.width() as u16) / 2;
                buf.set_string(ex, r.y, &icon.emoji, Style::default().fg(Color::White));
                let label = truncate_label(&icon.label, r.width as usize);
                let lx = r.x + (r.width.saturating_sub(label.width() as u16)) / 2;
                buf.set_string(
                    lx,
                    r.y + 2,
                    &label,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(if selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                );
            }
        }

        // windows, bottom to top
        let top = self.top_idx();
        for i in 0..self.windows.len() {
            if self.windows[i].minimized {
                continue;
            }
            let rect = self.windows[i].rect;
            let focused = Some(i) == top;
            gfx::shadow(f.buffer_mut(), rect, desk);
            f.render_widget(Clear, rect);

            let border_style = if focused {
                Style::default().fg(ACCENT)
            } else {
                Style::default().fg(Color::Rgb(82, 86, 112))
            };
            let title_style = if focused {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(150, 152, 172))
            };
            let title = Line::from(vec![Span::styled(
                format!(" {} {} ", self.windows[i].icon, self.windows[i].app.title()),
                title_style,
            )]);
            let buttons = Line::from(vec![
                Span::styled("─", Style::default().fg(Color::Rgb(255, 200, 80))),
                Span::raw(" "),
                Span::styled("□", Style::default().fg(Color::Rgb(120, 220, 130))),
                Span::raw(" "),
                Span::styled("✕", Style::default().fg(Color::Rgb(255, 95, 95))),
                Span::raw(" "),
            ])
            .right_aligned();
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .title(title)
                .title_top(buttons)
                .style(Style::default().bg(WIN_BG));
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            self.windows[i].app.render(f, inner, focused);

            // resize grip
            let gx = rect.x + rect.width - 1;
            let gy = rect.y + rect.height - 1;
            if let Some(c) = f.buffer_mut().cell_mut((gx, gy)) {
                c.set_symbol("◢");
                c.set_fg(if focused {
                    ACCENT
                } else {
                    Color::Rgb(82, 86, 112)
                });
            }
        }

        // drag ghost for files / moving icons
        self.render_drag_ghost(f, desk);

        // snap zone preview while moving a window near an edge
        if let Some(preview) = self.snap_preview {
            let buf = f.buffer_mut();
            for y in preview.y..preview.y + preview.height {
                for x in preview.x..preview.x + preview.width {
                    if let Some(c) = buf.cell_mut((x, y)) {
                        let bg = c.bg;
                        // tint cells lightly
                        let _ = bg;
                        c.set_bg(Color::Rgb(20, 60, 100));
                    }
                }
            }
        }

        self.render_taskbar(f, taskbar_y);

        if self.overflow_open {
            self.render_overflow_menu(f, taskbar_y);
        }
        if self.start_open {
            self.render_start_menu(f, taskbar_y);
        }
        if let Some(sw) = &self.switcher {
            self.render_switcher(f, sw);
        }
        if self.context.is_some() {
            self.render_context_menu(f);
        }
        if self.show_keys {
            self.render_keys_overlay(f);
        }

        // transient status toast above taskbar
        if let Some(until) = self.status_until {
            if until.elapsed().as_millis() < 2500 && !self.status.is_empty() {
                let msg = truncate_label(&format!(" {} ", self.status), desk.width as usize);
                let w = msg.width() as u16;
                let x = desk.width.saturating_sub(w) / 2;
                let y = taskbar_y.saturating_sub(1);
                f.buffer_mut().set_string(
                    x,
                    y,
                    &msg,
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(30, 50, 80))
                        .add_modifier(Modifier::BOLD),
                );
            } else {
                self.status.clear();
                self.status_until = None;
            }
        }
    }

    fn render_drag_ghost(&self, f: &mut Frame, desk: Rect) {
        let buf = f.buffer_mut();
        match &self.drag {
            Some(Drag::File {
                path,
                cut,
                ghost_x,
                ghost_y,
                ..
            }) => {
                // tint windows that accept drops
                for w in &self.windows {
                    if w.minimized || !w.app.accepts_drop() {
                        continue;
                    }
                    let r = w.rect;
                    for y in r.y..r.y + r.height.min(desk.height.saturating_sub(r.y)) {
                        if let Some(c) = buf.cell_mut((r.x, y)) {
                            c.set_fg(Color::Rgb(80, 200, 140));
                        }
                        if let Some(c) = buf.cell_mut((r.x + r.width.saturating_sub(1), y)) {
                            c.set_fg(Color::Rgb(80, 200, 140));
                        }
                    }
                }
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());
                let icon = if path.is_dir() { "📁" } else { "📄" };
                let prefix = if *cut { "✂" } else { "📋" };
                let text = format!(" {prefix}{icon} {name} ");
                let w = text.width().min(desk.width.saturating_sub(2) as usize) as u16;
                let x = (*ghost_x).min(desk.width.saturating_sub(w));
                let y = (*ghost_y).min(desk.height.saturating_sub(1));
                buf.set_string(
                    x,
                    y,
                    &truncate_label(&text, w as usize),
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(40, 80, 120))
                        .add_modifier(Modifier::BOLD),
                );
            }
            Some(Drag::Icon { idx, .. }) => {
                if let Some(icon) = self.icons.get(*idx) {
                    let r = icon_rect(icon);
                    // highlight destination cell
                    for y in r.y..r.y + r.height {
                        for x in r.x..(r.x + r.width).min(desk.width) {
                            if y < desk.height {
                                if let Some(c) = buf.cell_mut((x, y)) {
                                    c.set_bg(Color::Rgb(0, 100, 140));
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn render_taskbar(&mut self, f: &mut Frame, y: u16) {
        let area = self.screen;
        let buf = f.buffer_mut();
        for x in 0..area.width {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(" ");
                c.set_bg(TASKBAR_BG);
            }
        }
        let start_label = " ⊞ Start ";
        self.start_w = Span::raw(start_label).width() as u16;
        let start_style = if self.start_open {
            Style::default()
                .fg(Color::White)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(0, 96, 160))
                .add_modifier(Modifier::BOLD)
        };
        buf.set_string(0, y, start_label, start_style);

        let clock = chrono::Local::now().format(" %H:%M  %a %d %b ").to_string();
        let cw = clock.len() as u16;
        let clock_x = area.width.saturating_sub(cw);

        // git indicator just left of clock
        self.git_btn = None;
        let git_w = if self.git_label.is_empty() {
            0u16
        } else {
            Span::raw(&self.git_label).width() as u16
        };
        let git_x = clock_x.saturating_sub(git_w);

        self.taskbar_hits.clear();
        self.overflow_ids.clear();
        self.overflow_btn = None;
        let mut x = self.start_w + 1;
        let top = self.top_idx();
        let reserve = 6u16 + git_w; // »N + git
        let limit = clock_x.saturating_sub(reserve + 1).min(git_x.saturating_sub(1));

        let mut shown = 0usize;
        for (i, w) in self.windows.iter().enumerate() {
            let mut t = w.app.title();
            if t.chars().count() > 14 {
                t = t.chars().take(13).collect::<String>() + "…";
            }
            let label = format!(" {} {} ", w.icon, t);
            let wdt = Span::raw(&label).width() as u16;
            if x + wdt >= limit {
                for w2 in self.windows.iter().skip(i) {
                    self.overflow_ids.push(w2.id);
                }
                break;
            }
            let style = if Some(i) == top {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(42, 70, 112))
                    .add_modifier(Modifier::BOLD)
            } else if w.minimized {
                Style::default()
                    .fg(Color::Rgb(120, 122, 140))
                    .bg(TASKBAR_BG)
                    .add_modifier(Modifier::ITALIC)
            } else {
                Style::default()
                    .fg(Color::Rgb(190, 192, 205))
                    .bg(TASKBAR_BG)
            };
            buf.set_string(x, y, &label, style);
            self.taskbar_hits.push((x, x + wdt, w.id));
            x += wdt + 1;
            shown += 1;
        }
        let _ = shown;

        if !self.overflow_ids.is_empty() {
            let label = format!(" »{} ", self.overflow_ids.len());
            let wdt = Span::raw(&label).width() as u16;
            let style = if self.overflow_open {
                Style::default()
                    .fg(Color::White)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Rgb(220, 200, 120))
                    .bg(Color::Rgb(40, 44, 64))
                    .add_modifier(Modifier::BOLD)
            };
            buf.set_string(x, y, &label, style);
            self.overflow_btn = Some((x, x + wdt));
        }

        if git_w > 0 {
            let dirty = self.git_label.contains('*');
            buf.set_string(
                git_x,
                y,
                &self.git_label,
                Style::default()
                    .fg(if dirty {
                        Color::Rgb(255, 200, 90)
                    } else {
                        Color::Rgb(120, 220, 160)
                    })
                    .bg(Color::Rgb(22, 32, 48))
                    .add_modifier(Modifier::BOLD),
            );
            self.git_btn = Some((git_x, git_x + git_w));
        }

        if area.width > cw {
            buf.set_string(
                clock_x,
                y,
                &clock,
                Style::default()
                    .fg(Color::Rgb(200, 212, 235))
                    .bg(TASKBAR_BG),
            );
        }
    }

    fn render_overflow_menu(&mut self, f: &mut Frame, taskbar_y: u16) {
        let ids = self.overflow_ids.clone();
        if ids.is_empty() {
            return;
        }
        let items: Vec<(String, u64)> = ids
            .iter()
            .filter_map(|&id| {
                self.windows.iter().find(|w| w.id == id).map(|w| {
                    let mark = if w.minimized { "·" } else { "●" };
                    (format!(" {mark} {} {} ", w.icon, w.app.title()), id)
                })
            })
            .collect();
        if items.is_empty() {
            return;
        }
        let w = items
            .iter()
            .map(|(l, _)| l.width())
            .max()
            .unwrap_or(20)
            .clamp(16, 40) as u16;
        let h = (items.len() as u16 + 2).min(taskbar_y.max(3));
        let x = self
            .overflow_btn
            .map(|(x0, _)| x0.saturating_sub(2))
            .unwrap_or(1)
            .min(self.screen.width.saturating_sub(w));
        let y = taskbar_y.saturating_sub(h);
        let rect = Rect::new(x, y, w, h);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(160, 170, 200)))
            .title(Span::styled(
                " more ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(MENU_BG));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let buf = f.buffer_mut();
        for (i, (label, _)) in items.iter().enumerate() {
            let iy = inner.y + i as u16;
            if iy >= inner.y + inner.height {
                break;
            }
            let style = Style::default().fg(Color::Rgb(210, 214, 228)).bg(MENU_BG);
            let pad = " ".repeat(inner.width as usize);
            buf.set_string(inner.x, iy, &pad, style);
            buf.set_string(inner.x, iy, label, style);
        }
        // stash rect for hit testing
        self.overflow_rect = rect;
    }

    fn render_switcher(&self, f: &mut Frame, sw: &Switcher) {
        let n = sw.ids.len().max(1);
        let w = 42u16.min(self.screen.width.saturating_sub(4));
        let h = (n as u16 + 4).min(self.screen.height.saturating_sub(4));
        let x = self.screen.width.saturating_sub(w) / 2;
        let y = self.screen.height.saturating_sub(h) / 3;
        let rect = Rect::new(x, y, w, h);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(ACCENT))
            .title(Span::styled(
                " switch window · Tab/↑↓ · Enter · Esc ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(MENU_BG));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let buf = f.buffer_mut();
        for (i, id) in sw.ids.iter().enumerate() {
            let iy = inner.y + i as u16;
            if iy >= inner.y + inner.height {
                break;
            }
            let Some(win) = self.windows.iter().find(|w| w.id == *id) else {
                continue;
            };
            let mark = if win.minimized { "·" } else { "●" };
            let text = format!(" {mark} {} {} ", win.icon, win.app.title());
            let style = if i == sw.sel {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(0, 110, 170))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(210, 214, 228)).bg(MENU_BG)
            };
            let pad = " ".repeat(inner.width as usize);
            buf.set_string(inner.x, iy, &pad, style);
            buf.set_string(
                inner.x,
                iy,
                &truncate_label(&text, inner.width as usize),
                style,
            );
        }
    }

    /// Start-menu entries matching the current typed filter (label or group).
    fn filtered_menu(&self) -> Vec<usize> {
        if self.start_filter.is_empty() {
            return (0..MENU.len()).collect();
        }
        let q = self.start_filter.to_lowercase();
        (0..MENU.len())
            .filter(|&i| {
                MENU[i].0.to_lowercase().contains(&q) || MENU[i].2.to_lowercase().contains(&q)
            })
            .collect()
    }

    /// Display rows: group headers interleaved with items. `Some(fi)` indexes
    /// into `filtered`; `None` marks a header. Headers are dropped while a
    /// text filter is active so results stay compact.
    fn start_rows(&self, filtered: &[usize]) -> Vec<(Option<usize>, String)> {
        let show_headers = self.start_filter.is_empty();
        let mut rows = Vec::new();
        let mut last_group = "";
        for (fi, &mi) in filtered.iter().enumerate() {
            let (label, action, group) = MENU[mi];
            if show_headers && group != last_group {
                rows.push((None, format!(" {group}")));
                last_group = group;
            }
            let text = if action == MenuAction::Wallpaper {
                format!("  {label}  · {}", THEMES[self.theme % THEMES.len()].2)
            } else {
                format!("  {label}")
            };
            rows.push((Some(fi), text));
        }
        rows
    }

    fn render_start_menu(&mut self, f: &mut Frame, taskbar_y: u16) {
        let filtered = self.filtered_menu();
        let rows = self.start_rows(&filtered);
        let want = rows.len().max(1) as u16 + 2;
        let h = want.min(taskbar_y.max(1));
        let w = 36u16.min(self.screen.width.saturating_sub(2));
        let rect = Rect::new(1, taskbar_y.saturating_sub(h), w, h);
        self.menu_rect = rect;
        f.render_widget(Clear, rect);
        let title = if self.start_filter.is_empty() {
            " ⊞ texttop · type to search ".to_string()
        } else {
            format!(" 🔎 {}_ ", self.start_filter)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(ACCENT))
            .title(Span::styled(
                title,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(MENU_BG));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let buf = f.buffer_mut();
        if filtered.is_empty() {
            buf.set_string(
                inner.x,
                inner.y,
                " no match — Esc clears ",
                Style::default().fg(Color::Rgb(150, 155, 175)).bg(MENU_BG),
            );
            return;
        }
        for (row, (item, text)) in rows.iter().enumerate() {
            let iy = inner.y + row as u16;
            if iy >= inner.y + inner.height {
                break;
            }
            let style = match item {
                None => Style::default()
                    .fg(Color::Rgb(110, 125, 165))
                    .bg(MENU_BG)
                    .add_modifier(Modifier::BOLD | Modifier::ITALIC),
                Some(fi) if *fi == self.start_sel => Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(0, 110, 170))
                    .add_modifier(Modifier::BOLD),
                Some(_) => Style::default().fg(Color::Rgb(210, 214, 228)).bg(MENU_BG),
            };
            let pad = " ".repeat(inner.width as usize);
            buf.set_string(inner.x, iy, &pad, style);
            buf.set_string(inner.x, iy, text, style);
        }
    }

    fn render_keys_overlay(&self, f: &mut Frame) {
        const KEYS: &[(&str, &str)] = &[
            ("F1", "start menu (type to search)"),
            ("Alt+Tab · F6", "window switcher"),
            ("Ctrl+W", "close window"),
            ("F11 · Ctrl+Shift+↑", "maximize / restore"),
            ("Ctrl+Shift+←/→", "snap left / right"),
            ("Ctrl+arrows", "move window"),
            ("Ctrl+D", "show desktop"),
            ("drag to edge", "snap · top edge maximizes"),
            ("double-click title", "maximize"),
            ("middle-click taskbar", "close that window"),
            ("right-click", "context menus everywhere"),
            ("F5", "clean dead desktop shortcuts"),
            ("F12", "this overlay"),
        ];
        let w = 54u16.min(self.screen.width.saturating_sub(2));
        let h = (KEYS.len() as u16 + 4).min(self.screen.height.saturating_sub(2));
        let x = self.screen.width.saturating_sub(w) / 2;
        let y = self.screen.height.saturating_sub(h) / 3;
        let rect = Rect::new(x, y, w, h);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(ACCENT))
            .title(Span::styled(
                " ⌨ desktop shortcuts · any key closes ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(MENU_BG));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let buf = f.buffer_mut();
        for (i, (k, d)) in KEYS.iter().enumerate() {
            let iy = inner.y + 1 + i as u16;
            if iy >= inner.y + inner.height {
                break;
            }
            buf.set_string(
                inner.x + 1,
                iy,
                format!("{k:>20}"),
                Style::default()
                    .fg(Color::Rgb(150, 200, 255))
                    .bg(MENU_BG)
                    .add_modifier(Modifier::BOLD),
            );
            buf.set_string(
                inner.x + 23,
                iy,
                *d,
                Style::default().fg(Color::Rgb(200, 205, 222)).bg(MENU_BG),
            );
        }
    }

    fn render_context_menu(&mut self, f: &mut Frame) {
        let Some(ctx) = self.context.as_mut() else {
            return;
        };
        let w = ctx
            .items
            .iter()
            .map(|(l, _)| l.width() + 2)
            .max()
            .unwrap_or(12)
            .clamp(12, 36) as u16;
        let h = (ctx.items.len() as u16 + 2).min(self.screen.height.saturating_sub(1).max(3));
        let mut x = ctx.rect.x;
        let mut y = ctx.rect.y;
        if x + w > self.screen.width {
            x = self.screen.width.saturating_sub(w);
        }
        let taskbar_y = self.screen.height.saturating_sub(1);
        if y + h > taskbar_y {
            y = taskbar_y.saturating_sub(h);
        }
        let rect = Rect::new(x, y, w, h);
        ctx.rect = rect;
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(160, 170, 200)))
            .style(Style::default().bg(MENU_BG));
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
                Style::default().fg(Color::Rgb(210, 214, 228)).bg(MENU_BG)
            };
            let pad = " ".repeat(inner.width as usize);
            buf.set_string(inner.x, iy, &pad, style);
            buf.set_string(inner.x, iy, &format!(" {label}"), style);
        }
    }

    // ---------------------------------------------------------- input

    pub fn on_key(&mut self, k: KeyEvent) {
        // the shortcut overlay swallows the next key press
        if self.show_keys {
            self.show_keys = false;
            return;
        }

        // Esc cancels file drag / menus first
        if matches!(k.code, KeyCode::Esc) {
            if self.drag.take().is_some() {
                self.snap_preview = None;
                self.toast("drag cancelled");
                return;
            }
            if self.switcher.take().is_some() {
                return;
            }
            if self.overflow_open {
                self.overflow_open = false;
                return;
            }
            if self.context.take().is_some() {
                return;
            }
            if self.start_open {
                if !self.start_filter.is_empty() {
                    self.start_filter.clear();
                    self.start_sel = 0;
                } else {
                    self.start_open = false;
                }
                return;
            }
        }

        if let Some(sw) = &mut self.switcher {
            let n = sw.ids.len().max(1);
            match k.code {
                KeyCode::Tab | KeyCode::Down | KeyCode::Right => {
                    sw.sel = (sw.sel + 1) % n;
                }
                KeyCode::BackTab | KeyCode::Up | KeyCode::Left => {
                    sw.sel = sw.sel.checked_sub(1).unwrap_or(n - 1);
                }
                KeyCode::Enter => {
                    let id = sw.ids.get(sw.sel).copied();
                    self.switcher = None;
                    if let Some(id) = id {
                        self.focus_id(id);
                    }
                }
                KeyCode::Char('w') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(&id) = sw.ids.get(sw.sel) {
                        self.close_id(id);
                        self.refresh_switcher();
                    }
                }
                _ => {}
            }
            return;
        }

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
                            self.run_ctx(action.clone());
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        if self.start_open {
            let filtered = self.filtered_menu();
            let n = filtered.len();
            match k.code {
                KeyCode::Esc | KeyCode::F(1) => self.start_open = false,
                KeyCode::Up => {
                    if n > 0 {
                        self.start_sel = self.start_sel.checked_sub(1).unwrap_or(n - 1);
                    }
                }
                KeyCode::Down => {
                    if n > 0 {
                        self.start_sel = (self.start_sel + 1) % n;
                    }
                }
                KeyCode::Enter => {
                    if let Some(&mi) = filtered.get(self.start_sel) {
                        self.activate_menu(MENU[mi].1);
                    }
                }
                KeyCode::Backspace => {
                    self.start_filter.pop();
                    self.start_sel = 0;
                }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.start_filter.push(c);
                    self.start_sel = 0;
                }
                _ => {}
            }
            return;
        }

        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);

        // Alt+Tab / Ctrl+Tab → window switcher
        if (alt || ctrl) && matches!(k.code, KeyCode::Tab | KeyCode::BackTab) {
            self.open_switcher(matches!(k.code, KeyCode::BackTab) || shift);
            return;
        }

        match k.code {
            KeyCode::F(1) => {
                self.start_open = true;
                self.start_sel = 0;
                self.start_filter.clear();
                self.overflow_open = false;
                return;
            }
            KeyCode::F(12) => {
                self.show_keys = true;
                return;
            }
            KeyCode::F(5) => {
                let before = self.icons.len();
                self.icons.retain(|i| match &i.kind {
                    DeskKind::App(_) => true,
                    DeskKind::Shortcut(p) => p.exists(),
                });
                let removed = before.saturating_sub(self.icons.len());
                self.icon_sel = self.icon_sel.min(self.icons.len().saturating_sub(1));
                if removed > 0 {
                    self.save_desktop();
                    self.toast(format!("cleaned {removed} dead shortcut(s)"));
                } else {
                    self.toast("desktop ok");
                }
                return;
            }
            KeyCode::F(6) => {
                self.open_switcher(false);
                return;
            }
            KeyCode::F(11) => {
                if let Some(i) = self.top_idx() {
                    self.toggle_max(i);
                }
                return;
            }
            KeyCode::Char('d') if ctrl => {
                self.show_desktop();
                return;
            }
            KeyCode::Char('w') if ctrl => {
                if let Some(i) = self.top_idx() {
                    let id = self.windows[i].id;
                    self.close_id(id);
                }
                return;
            }
            KeyCode::Left | KeyCode::Right if ctrl && shift => {
                if let Some(i) = self.top_idx() {
                    let id = self.windows[i].id;
                    self.snap_window(id, matches!(k.code, KeyCode::Left));
                }
                return;
            }
            KeyCode::Up if ctrl && shift => {
                if let Some(i) = self.top_idx() {
                    self.toggle_max(i);
                }
                return;
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if ctrl && !shift => {
                if let Some(i) = self.top_idx() {
                    let taskbar_y = self.screen.height.saturating_sub(1);
                    let r = &mut self.windows[i].rect;
                    match k.code {
                        KeyCode::Left => r.x = r.x.saturating_sub(2),
                        KeyCode::Right => {
                            r.x = (r.x + 2).min(self.screen.width.saturating_sub(r.width))
                        }
                        KeyCode::Up => r.y = r.y.saturating_sub(1),
                        KeyCode::Down => r.y = (r.y + 1).min(taskbar_y.saturating_sub(r.height)),
                        _ => {}
                    }
                }
                return;
            }
            KeyCode::Delete if self.top_idx().is_none() => {
                if let Some(DeskKind::Shortcut(_)) = self.icons.get(self.icon_sel).map(|i| &i.kind)
                {
                    self.icons.remove(self.icon_sel);
                    self.icon_sel = self.icon_sel.min(self.icons.len().saturating_sub(1));
                    self.save_desktop();
                    self.toast("shortcut removed");
                }
                return;
            }
            _ => {}
        }

        if let Some(i) = self.top_idx() {
            let action = self.windows[i].app.on_key(k);
            let id = self.windows[i].id;
            self.process(id, action);
        } else {
            // desktop icon navigation
            match k.code {
                KeyCode::Up | KeyCode::Left => {
                    self.icon_sel = self
                        .icon_sel
                        .checked_sub(1)
                        .unwrap_or(self.icons.len().saturating_sub(1))
                }
                KeyCode::Down | KeyCode::Right => {
                    if !self.icons.is_empty() {
                        self.icon_sel = (self.icon_sel + 1) % self.icons.len();
                    }
                }
                KeyCode::Enter => {
                    if let Some(icon) = self.icons.get(self.icon_sel).cloned() {
                        self.activate_icon(&icon);
                    }
                }
                KeyCode::Char(' ') if shift => {
                    if let Some(icon) = self.icons.get(self.icon_sel) {
                        let r = icon_rect(icon);
                        self.open_icon_context(self.icon_sel, r.x + r.width, r.y);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        let (col, row) = (m.column, m.row);
        let taskbar_y = self.screen.height.saturating_sub(1);

        if self.show_keys {
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.show_keys = false;
            }
            return;
        }

        // context menu eats clicks first
        if let Some(ctx) = &self.context {
            let r = ctx.rect;
            match m.kind {
                MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::Down(MouseButton::Right) => {
                    if contains(r, col, row) {
                        if row > r.y && row < r.y + r.height - 1 {
                            let idx = (row - r.y - 1) as usize;
                            if idx < ctx.items.len() {
                                let action = ctx.items[idx].1.clone();
                                self.context = None;
                                self.run_ctx(action);
                            }
                        }
                    } else {
                        self.context = None;
                        // fall through for right-click to open new menu
                        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Right)) {
                            return;
                        }
                    }
                    if self.context.is_none()
                        && !matches!(m.kind, MouseEventKind::Down(MouseButton::Right))
                    {
                        return;
                    }
                    if matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
                        return;
                    }
                }
                MouseEventKind::Moved => {
                    if contains(r, col, row) && row > r.y && row < r.y + r.height - 1 {
                        let idx = (row - r.y - 1) as usize;
                        if let Some(ctx) = &mut self.context {
                            if idx < ctx.items.len() {
                                ctx.sel = idx;
                            }
                        }
                    }
                    return;
                }
                _ => return,
            }
        }

        match m.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                self.start_open = false;
                self.handle_right_click(col, row, taskbar_y);
            }
            MouseEventKind::Down(MouseButton::Middle) => {
                // middle-click taskbar button → close
                if row == taskbar_y {
                    let hits = self.taskbar_hits.clone();
                    for (x0, x1, id) in hits {
                        if col >= x0 && col < x1 {
                            self.close_id(id);
                            return;
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.switcher = None;
                if row == taskbar_y {
                    if col < self.start_w {
                        self.start_open = !self.start_open;
                        self.start_sel = 0;
                        self.start_filter.clear();
                        self.context = None;
                        self.overflow_open = false;
                        return;
                    }
                    if let Some((x0, x1)) = self.overflow_btn {
                        if col >= x0 && col < x1 {
                            self.overflow_open = !self.overflow_open;
                            self.start_open = false;
                            return;
                        }
                    }
                    if let Some((x0, x1)) = self.git_btn {
                        if col >= x0 && col < x1 {
                            self.spawn(Launch::Git(self.active_git_root.clone()));
                            self.start_open = false;
                            self.overflow_open = false;
                            return;
                        }
                    }
                    let hits = self.taskbar_hits.clone();
                    for (x0, x1, id) in hits {
                        if col >= x0 && col < x1 {
                            self.overflow_open = false;
                            self.focus_or_minimize(id);
                            return;
                        }
                    }
                    self.start_open = false;
                    self.overflow_open = false;
                    return;
                }
                if self.overflow_open {
                    let r = self.overflow_rect;
                    if contains(r, col, row) {
                        if row > r.y && row < r.y + r.height - 1 {
                            let idx = (row - r.y - 1) as usize;
                            if idx < self.overflow_ids.len() {
                                let id = self.overflow_ids[idx];
                                self.overflow_open = false;
                                self.focus_id(id);
                            }
                        }
                    } else {
                        self.overflow_open = false;
                    }
                    return;
                }
                if self.start_open {
                    let r = self.menu_rect;
                    if col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height {
                        if row > r.y && row < r.y + r.height - 1 {
                            let idx = (row - r.y - 1) as usize;
                            let filtered = self.filtered_menu();
                            let rows = self.start_rows(&filtered);
                            if let Some((Some(fi), _)) = rows.get(idx) {
                                if let Some(&mi) = filtered.get(*fi) {
                                    self.start_sel = *fi;
                                    self.activate_menu(MENU[mi].1);
                                }
                            }
                        }
                    } else {
                        self.start_open = false;
                    }
                    return;
                }
                if let Some(i) = self
                    .windows
                    .iter()
                    .rposition(|w| !w.minimized && contains(w.rect, col, row))
                {
                    let w = self.windows.remove(i);
                    self.windows.push(w);
                    let i = self.windows.len() - 1;
                    let rect = self.windows[i].rect;
                    let id = self.windows[i].id;
                    let right = rect.x + rect.width;
                    if row == rect.y {
                        if col >= right.saturating_sub(4) && col < right {
                            self.close_id(id);
                        } else if col >= right.saturating_sub(6) {
                            self.toggle_max(i);
                        } else if col >= right.saturating_sub(8) {
                            self.windows[i].minimized = true;
                        } else {
                            // double-click title → maximize
                            let dbl = matches!(self.last_title_click, Some((t, lid))
                                if lid == id && t.elapsed().as_millis() < 450);
                            if dbl {
                                self.toggle_max(i);
                                self.last_title_click = None;
                            } else {
                                self.last_title_click = Some((Instant::now(), id));
                                // Keep restore until the cursor actually moves (see Drag).
                                self.drag = Some(Drag::Move {
                                    id,
                                    dx: col.saturating_sub(rect.x),
                                });
                            }
                        }
                        return;
                    }
                    if col >= right.saturating_sub(2) && row == rect.y + rect.height - 1 {
                        self.drag = Some(Drag::Resize { id });
                        return;
                    }
                    let inner = Rect::new(
                        rect.x + 1,
                        rect.y + 1,
                        rect.width.saturating_sub(2),
                        rect.height.saturating_sub(2),
                    );
                    let action = self.windows[i].app.on_mouse(m, inner);
                    self.process(id, action);
                    return;
                }
                // desktop icons — select / double-open; drag starts after movement
                for i in 0..self.icons.len() {
                    let r = icon_rect(&self.icons[i]);
                    if contains(r, col, row) {
                        let dbl = matches!(self.last_click, Some((t, li))
                            if li == i as i32 && t.elapsed().as_millis() < 450);
                        self.icon_sel = i;
                        if dbl {
                            let icon = self.icons[i].clone();
                            self.activate_icon(&icon);
                            self.last_click = None;
                            self.pending_icon = None;
                        } else {
                            self.last_click = Some((Instant::now(), i as i32));
                            self.pending_icon = Some((
                                i,
                                col.saturating_sub(self.icons[i].x),
                                row.saturating_sub(self.icons[i].y),
                                self.icons[i].x,
                                self.icons[i].y,
                            ));
                        }
                        return;
                    }
                }
                self.pending_icon = None;
                self.last_click = Some((Instant::now(), -1));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let taskbar_y = self.screen.height.saturating_sub(1);
                if self.drag.is_none() {
                    if let Some((idx, gx, gy, sx, sy)) = self.pending_icon {
                        let moved = col.abs_diff(sx.saturating_add(gx)) > 1
                            || row.abs_diff(sy.saturating_add(gy)) > 1;
                        if moved {
                            self.pending_icon = None;
                            self.drag = Some(Drag::Icon {
                                idx,
                                grab_x: gx,
                                grab_y: gy,
                                start_x: sx,
                                start_y: sy,
                            });
                        }
                    }
                }
                match &self.drag {
                    Some(Drag::Move { id, dx }) => {
                        let (id, dx) = (*id, *dx);
                        // Unsnap on first movement if still maximized/snapped
                        self.unsnap_if_needed(id, col);
                        let dx = self
                            .drag
                            .as_ref()
                            .and_then(|d| match d {
                                Drag::Move { dx, .. } => Some(*dx),
                                _ => None,
                            })
                            .unwrap_or(dx);
                        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                            let max_x = self.screen.width.saturating_sub(w.rect.width);
                            w.rect.x = col.saturating_sub(dx).min(max_x);
                            w.rect.y = row.min(taskbar_y.saturating_sub(w.rect.height));
                        }
                        self.snap_preview = self.preview_snap_at(col, row);
                    }
                    Some(Drag::Resize { id }) => {
                        let id = *id;
                        self.snap_preview = None;
                        // Leave snap/maximize state when the user resizes
                        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                            w.restore = None;
                            let max_w = self.screen.width.saturating_sub(w.rect.x);
                            let max_h = taskbar_y.saturating_sub(w.rect.y);
                            w.rect.width =
                                (col.saturating_sub(w.rect.x) + 1).clamp(24, max_w.max(24));
                            w.rect.height =
                                (row.saturating_sub(w.rect.y) + 1).clamp(6, max_h.max(6));
                        }
                    }
                    Some(Drag::Icon {
                        idx,
                        grab_x,
                        grab_y,
                        ..
                    }) => {
                        self.snap_preview = None;
                        let (idx, gx, gy) = (*idx, *grab_x, *grab_y);
                        if let Some(icon) = self.icons.get_mut(idx) {
                            let max_x = self.screen.width.saturating_sub(ICON_W + 1);
                            let max_y = taskbar_y.saturating_sub(ICON_H + 1);
                            icon.x = col.saturating_sub(gx).min(max_x);
                            icon.y = row.saturating_sub(gy).min(max_y);
                        }
                    }
                    Some(Drag::File { .. }) => {
                        let shift = m.modifiers.contains(KeyModifiers::SHIFT);
                        if let Some(Drag::File {
                            ghost_x,
                            ghost_y,
                            cut,
                            ..
                        }) = &mut self.drag
                        {
                            *ghost_x = col;
                            *ghost_y = row.min(taskbar_y.saturating_sub(1));
                            if shift {
                                *cut = true;
                            }
                        }
                    }
                    None => self.forward_mouse_to_content(m, col, row),
                }
            }
            MouseEventKind::Up(_) => {
                self.pending_icon = None;
                self.snap_preview = None;
                let finished = self.drag.take();
                match finished {
                    Some(Drag::Move { id, .. }) => {
                        if let Some(w) = self.windows.iter().find(|w| w.id == id) {
                            let edge = 2u16;
                            let near_left = w.rect.x <= edge;
                            let near_right =
                                w.rect.x + w.rect.width + edge >= self.screen.width;
                            let near_top = w.rect.y <= edge;
                            let id = w.id;
                            if near_top {
                                self.snap_window_max(id);
                            } else if near_left {
                                self.snap_window(id, true);
                            } else if near_right {
                                self.snap_window(id, false);
                            }
                        }
                    }
                    Some(Drag::Icon {
                        idx,
                        start_x,
                        start_y,
                        ..
                    }) => {
                        let moved = self
                            .icons
                            .get(idx)
                            .map(|i| i.x != start_x || i.y != start_y)
                            .unwrap_or(false);
                        if moved {
                            self.snap_one_icon(idx);
                            self.save_desktop();
                            self.toast("icon moved");
                        }
                    }
                    Some(Drag::File {
                        path,
                        cut,
                        from_win,
                        ..
                    }) => {
                        self.handle_file_drop(path, cut, from_win, col, row);
                    }
                    Some(Drag::Resize { .. }) | None => {
                        if finished.is_none() {
                            self.forward_mouse_to_content(m, col, row);
                        }
                    }
                }
            }
            MouseEventKind::Moved
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown => {
                self.forward_mouse_to_content(m, col, row);
            }
            _ => {}
        }
    }

    fn handle_right_click(&mut self, col: u16, row: u16, taskbar_y: u16) {
        if row == taskbar_y {
            if col < self.start_w {
                self.open_context(
                    col,
                    row.saturating_sub(1),
                    vec![
                        ("Show desktop".into(), CtxAction::ShowDesktop),
                        ("Tile windows".into(), CtxAction::TileWindows),
                        ("Cascade windows".into(), CtxAction::CascadeWindows),
                        ("Close all".into(), CtxAction::CloseAll),
                        ("Wallpaper".into(), CtxAction::Wallpaper),
                    ],
                );
                return;
            }
            let hits = self.taskbar_hits.clone();
            for (x0, x1, id) in hits {
                if col >= x0 && col < x1 {
                    self.open_context(
                        col,
                        row.saturating_sub(1),
                        vec![
                            ("Restore / Focus".into(), CtxAction::RestoreWindow(id)),
                            ("Minimize".into(), CtxAction::MinimizeWindow(id)),
                            ("Maximize".into(), CtxAction::MaximizeWindow(id)),
                            ("Snap left".into(), CtxAction::SnapLeft(id)),
                            ("Snap right".into(), CtxAction::SnapRight(id)),
                            ("Close".into(), CtxAction::CloseWindow(id)),
                        ],
                    );
                    return;
                }
            }
            self.open_context(
                col,
                row.saturating_sub(1),
                vec![
                    ("Show desktop".into(), CtxAction::ShowDesktop),
                    ("Tile windows".into(), CtxAction::TileWindows),
                    ("Cascade windows".into(), CtxAction::CascadeWindows),
                ],
            );
            return;
        }

        if let Some(i) = self
            .windows
            .iter()
            .rposition(|w| !w.minimized && contains(w.rect, col, row))
        {
            let w = self.windows.remove(i);
            self.windows.push(w);
            let i = self.windows.len() - 1;
            let rect = self.windows[i].rect;
            let id = self.windows[i].id;
            if row == rect.y {
                // title-bar context
                self.open_context(
                    col,
                    row + 1,
                    vec![
                        ("Minimize".into(), CtxAction::MinimizeWindow(id)),
                        ("Maximize / Restore".into(), CtxAction::MaximizeWindow(id)),
                        ("Snap left".into(), CtxAction::SnapLeft(id)),
                        ("Snap right".into(), CtxAction::SnapRight(id)),
                        ("Close".into(), CtxAction::CloseWindow(id)),
                    ],
                );
                return;
            }
            // forward right-click into app content (file browser menus, etc.)
            let inner = Rect::new(
                rect.x + 1,
                rect.y + 1,
                rect.width.saturating_sub(2),
                rect.height.saturating_sub(2),
            );
            if contains(inner, col, row) {
                let m = MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Right),
                    column: col,
                    row,
                    modifiers: KeyModifiers::NONE,
                };
                let action = self.windows[i].app.on_mouse(m, inner);
                self.process(id, action);
            }
            return;
        }

        for i in 0..self.icons.len() {
            let r = icon_rect(&self.icons[i]);
            if contains(r, col, row) {
                self.icon_sel = i;
                self.open_icon_context(i, col, row);
                return;
            }
        }

        // empty desktop
        self.open_context(
            col,
            row,
            vec![
                ("Open Files".into(), CtxAction::OpenFiles),
                ("Open Terminal".into(), CtxAction::OpenTerminal),
                ("Open Editor".into(), CtxAction::OpenEditor),
                ("Show desktop".into(), CtxAction::ShowDesktop),
                ("Tile windows".into(), CtxAction::TileWindows),
                ("Cascade windows".into(), CtxAction::CascadeWindows),
                ("Reset icons".into(), CtxAction::ResetIcons),
                ("Wallpaper".into(), CtxAction::Wallpaper),
            ],
        );
    }

    fn open_icon_context(&mut self, idx: usize, col: u16, row: u16) {
        let mut items = vec![("Open".into(), CtxAction::Open)];
        if matches!(self.icons.get(idx).map(|i| &i.kind), Some(DeskKind::Shortcut(_))) {
            items.push(("Remove shortcut".into(), CtxAction::RemoveShortcut(idx)));
        }
        items.push(("Reset icons".into(), CtxAction::ResetIcons));
        self.open_context(col, row, items);
    }

    fn open_context(&mut self, x: u16, y: u16, items: Vec<(String, CtxAction)>) {
        self.context = Some(ContextMenu {
            items,
            sel: 0,
            rect: Rect::new(x, y, 1, 1), // sized on render
        });
    }

    fn run_ctx(&mut self, action: CtxAction) {
        match action {
            CtxAction::Open => {
                if let Some(icon) = self.icons.get(self.icon_sel).cloned() {
                    self.activate_icon(&icon);
                }
            }
            CtxAction::OpenFiles => self.activate_menu(MenuAction::Files),
            CtxAction::OpenTerminal => self.activate_menu(MenuAction::Terminal),
            CtxAction::OpenEditor => self.activate_menu(MenuAction::Editor),
            CtxAction::Wallpaper => {
                self.theme = (self.theme + 1) % THEMES.len();
                self.save_desktop();
                self.toast(format!("wallpaper · {}", THEMES[self.theme].2));
            }
            CtxAction::ShowDesktop => self.show_desktop(),
            CtxAction::TileWindows => self.tile_windows(),
            CtxAction::CascadeWindows => self.cascade_windows(),
            CtxAction::CloseAll => {
                self.windows.clear();
                self.toast("all windows closed");
            }
            CtxAction::CloseWindow(id) => self.close_id(id),
            CtxAction::MinimizeWindow(id) => {
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                    w.minimized = true;
                }
            }
            CtxAction::MaximizeWindow(id) => {
                if let Some(i) = self.windows.iter().position(|w| w.id == id) {
                    self.windows[i].minimized = false;
                    let w = self.windows.remove(i);
                    self.windows.push(w);
                    let i = self.windows.len() - 1;
                    self.toggle_max(i);
                }
            }
            CtxAction::SnapLeft(id) => self.snap_window(id, true),
            CtxAction::SnapRight(id) => self.snap_window(id, false),
            CtxAction::RestoreWindow(id) => {
                if let Some(i) = self.windows.iter().position(|w| w.id == id) {
                    self.windows[i].minimized = false;
                    let w = self.windows.remove(i);
                    self.windows.push(w);
                }
            }
            CtxAction::RemoveShortcut(idx) => {
                if idx < self.icons.len()
                    && matches!(self.icons[idx].kind, DeskKind::Shortcut(_))
                {
                    self.icons.remove(idx);
                    self.icon_sel = self.icon_sel.min(self.icons.len().saturating_sub(1));
                    self.save_desktop();
                    self.toast("shortcut removed");
                }
            }
            CtxAction::ResetIcons => {
                self.icons = default_icons();
                self.icon_sel = 0;
                self.save_desktop();
                self.toast("icons reset");
            }
        }
    }

    fn handle_file_drop(
        &mut self,
        path: PathBuf,
        cut: bool,
        from_win: Option<u64>,
        col: u16,
        row: u16,
    ) {
        // drop onto a window content?
        if let Some(i) = self
            .windows
            .iter()
            .rposition(|w| !w.minimized && contains(w.rect, col, row))
        {
            let id = self.windows[i].id;
            if Some(id) == from_win {
                self.toast("drop cancelled");
                return;
            }
            let rect = self.windows[i].rect;
            let inner = Rect::new(
                rect.x + 1,
                rect.y + 1,
                rect.width.saturating_sub(2),
                rect.height.saturating_sub(2),
            );
            if contains(inner, col, row) {
                if self.windows[i].app.accepts_drop() {
                    let action = self.windows[i].app.on_drop(&[path], cut);
                    self.process(id, action);
                    self.toast(if cut { "moved into window" } else { "dropped into window" });
                } else {
                    // open with the right app instead
                    self.open_path(path);
                    self.toast("opened");
                }
                return;
            }
        }
        // drop on desktop → shortcut (never a filesystem move)
        self.add_shortcut(path, col, row);
    }

    fn add_shortcut(&mut self, path: PathBuf, col: u16, row: u16) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let (emoji, _) = crate::apps::file_icon(&name, path.is_dir());
        let taskbar_y = self.screen.height.saturating_sub(1);
        let x = col
            .saturating_sub(ICON_W / 2)
            .min(self.screen.width.saturating_sub(ICON_W + 1));
        let y = row
            .saturating_sub(1)
            .min(taskbar_y.saturating_sub(ICON_H + 1));
        // avoid exact overlaps
        let mut x = x;
        let y = y;
        for _ in 0..8 {
            let clash = self
                .icons
                .iter()
                .any(|i| i.x.abs_diff(x) < ICON_W && i.y.abs_diff(y) < ICON_H);
            if !clash {
                break;
            }
            x = (x + ICON_W + ICON_GAP).min(self.screen.width.saturating_sub(ICON_W + 1));
        }
        self.icons.push(DeskIcon {
            emoji: emoji.to_string(),
            label: truncate_label(&name, ICON_W as usize),
            kind: DeskKind::Shortcut(path),
            x,
            y,
        });
        self.icon_sel = self.icons.len() - 1;
        self.save_desktop();
        self.toast("shortcut added");
    }

    /// Deliver a mouse event to the topmost window's content area under the
    /// cursor (title-bar / border chrome is ignored so hover doesn't steal
    /// window-management gestures).
    fn forward_mouse_to_content(&mut self, m: MouseEvent, col: u16, row: u16) {
        if let Some(i) = self
            .windows
            .iter()
            .rposition(|w| !w.minimized && contains(w.rect, col, row))
        {
            let rect = self.windows[i].rect;
            if row <= rect.y || row >= rect.y + rect.height.saturating_sub(1) {
                return;
            }
            if col <= rect.x || col >= rect.x + rect.width.saturating_sub(1) {
                return;
            }
            let inner = Rect::new(
                rect.x + 1,
                rect.y + 1,
                rect.width.saturating_sub(2),
                rect.height.saturating_sub(2),
            );
            let id = self.windows[i].id;
            let action = self.windows[i].app.on_mouse(m, inner);
            self.process(id, action);
        }
    }

    pub fn on_tick(&mut self) {
        for w in &mut self.windows {
            w.app.on_tick();
        }
        self.sync_git_indicator();
    }

    fn sync_git_indicator(&mut self) {
        // Prefer focused window workspace
        if let Some(i) = self.top_idx() {
            if let Some(p) = self.windows[i].app.workspace_path() {
                if let Some(root) = gitutil::find_root(&p) {
                    self.active_git_root = Some(root);
                }
            }
        }
        let root = self
            .active_git_root
            .clone()
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|p| gitutil::find_root(&p))
            })
            .or_else(|| dirs::home_dir().and_then(|p| gitutil::find_root(&p)));
        if let Some(root) = root {
            self.active_git_root = Some(root.clone());
            let st = gitutil::short_stat(&root);
            self.git_label = st.taskbar();
        } else {
            self.git_label.clear();
        }
    }

    // ---------------------------------------------------------- helpers

    fn top_idx(&self) -> Option<usize> {
        self.windows.iter().rposition(|w| !w.minimized)
    }

    fn activate_icon(&mut self, icon: &DeskIcon) {
        match &icon.kind {
            DeskKind::App(a) => self.activate_menu(*a),
            DeskKind::Shortcut(p) => {
                let p = p.clone();
                self.open_path(p);
            }
        }
    }

    fn activate_menu(&mut self, action: MenuAction) {
        match action {
            MenuAction::Files => {
                let root = if cfg!(windows) { "C:\\" } else { "/" };
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from(root));
                self.spawn(Launch::Files(home));
                self.start_open = false;
            }
            MenuAction::Web => {
                self.spawn(Launch::Web(None));
                self.start_open = false;
            }
            MenuAction::Terminal => {
                self.spawn(Launch::Terminal {
                    program: None,
                    args: Vec::new(),
                    cwd: None,
                });
                self.start_open = false;
            }
            MenuAction::Editor => {
                self.spawn(Launch::Editor(None));
                self.start_open = false;
            }
            MenuAction::Git => {
                self.spawn(Launch::Git(self.active_git_root.clone()));
                self.start_open = false;
            }
            MenuAction::Search => {
                self.spawn(Launch::Search(self.active_git_root.clone()));
                self.start_open = false;
            }
            MenuAction::Processes => {
                self.spawn(Launch::Processes);
                self.start_open = false;
            }
            MenuAction::Calculator => {
                self.spawn(Launch::Calculator);
                self.start_open = false;
            }
            MenuAction::Clock => {
                self.spawn(Launch::Clock);
                self.start_open = false;
            }
            MenuAction::Calendar => {
                self.spawn(Launch::Calendar);
                self.start_open = false;
            }
            MenuAction::Notes => {
                self.spawn(Launch::Notes);
                self.start_open = false;
            }
            MenuAction::Hex => {
                self.spawn(Launch::Hex(None));
                self.start_open = false;
            }
            MenuAction::SysInfo => {
                self.spawn(Launch::SysInfo);
                self.start_open = false;
            }
            MenuAction::Snake => {
                self.spawn(Launch::Snake);
                self.start_open = false;
            }
            MenuAction::Minesweeper => {
                self.spawn(Launch::Minesweeper);
                self.start_open = false;
            }
            MenuAction::Game2048 => {
                self.spawn(Launch::Game2048);
                self.start_open = false;
            }
            MenuAction::Wallpaper => {
                self.theme = (self.theme + 1) % THEMES.len();
                self.save_desktop();
            }
            MenuAction::Keys => {
                self.show_keys = true;
                self.start_open = false;
            }
            MenuAction::About => {
                self.spawn(Launch::About);
                self.start_open = false;
            }
            MenuAction::Quit => {
                self.save_session();
                self.should_quit = true;
            }
        }
    }

    fn spawn(&mut self, launch: Launch) {
        let saved = launch.clone();
        let (app, icon, prefer_w, prefer_h, min_w, min_h): (
            Box<dyn AppWindow>,
            &'static str,
            u16,
            u16,
            u16,
            u16,
        ) = match launch {
            Launch::Files(p) => (Box::new(FileBrowser::new(p)), "🗂", 78, 28, 40, 12),
            Launch::Web(u) => (Box::new(WebBrowser::new(u)), "🌐", 96, 34, 48, 14),
            Launch::Audio(p) => (Box::new(AudioPlayer::new(p)), "🎵", 60, 18, 36, 10),
            Launch::Image(p) => (Box::new(ImageViewer::new(p)), "🖼", 80, 32, 40, 12),
            Launch::Video(p) => (Box::new(VideoPlayer::new(p)), "🎬", 88, 32, 48, 12),
            Launch::Editor(p) => (Box::new(Editor::new(p)), "📝", 80, 28, 40, 12),
            Launch::Terminal { program, args, cwd } => {
                (Box::new(Terminal::new(program, args, cwd)), "🖥", 86, 28, 48, 12)
            }
            Launch::AppLauncher(p) => (Box::new(AppLauncher::new(p)), "🧩", 72, 22, 40, 12),
            Launch::About => (Box::new(About::new()), "✨", 78, 34, 48, 14),
            Launch::Calculator => (Box::new(Calculator::new()), "🧮", 32, 26, 24, 14),
            Launch::Clock => (Box::new(ClockApp::new()), "⏱", 58, 18, 36, 10),
            Launch::Hex(p) => (Box::new(HexViewer::new(p)), "🔢", 84, 28, 48, 12),
            Launch::Calendar => (Box::new(Calendar::new()), "📅", 52, 20, 36, 12),
            Launch::SysInfo => (Box::new(SysInfo::new()), "💻", 76, 28, 40, 12),
            Launch::Notes => (Box::new(Notes::new()), "📌", 68, 24, 36, 10),
            Launch::Git(p) => (Box::new(GitApp::new(p)), "🌿", 84, 28, 48, 12),
            Launch::Search(p) => (Box::new(SearchApp::new(p)), "🔎", 80, 26, 40, 12),
            Launch::Processes => (Box::new(Processes::new()), "⚙", 72, 26, 40, 12),
            Launch::Snapshots(p) => (Box::new(SnapshotsApp::new(p)), "⏱", 84, 28, 48, 12),
            Launch::Compare(paths) => (Box::new(CompareApp::new(paths)), "↔", 90, 30, 48, 12),
            Launch::Snake => (Box::new(Snake::new()), "🐍", 64, 24, 30, 10),
            Launch::Minesweeper => (Box::new(Minesweeper::new()), "💣", 56, 22, 30, 12),
            Launch::Game2048 => (Box::new(Game2048::new()), "🎮", 44, 22, 28, 12),
        };
        let scr = self.screen;
        let taskbar = 1u16;
        // Leave a little margin so windows don't cover the whole desk by default,
        // but still grow when the terminal is small so content stays usable.
        let avail_w = scr.width.saturating_sub(2).max(24);
        let avail_h = scr.height.saturating_sub(taskbar + 1).max(6);
        let w = prefer_w.clamp(min_w.min(avail_w), avail_w);
        let h = prefer_h.clamp(min_h.min(avail_h), avail_h);
        let n = self.next_id as u16;
        let max_x = scr.width.saturating_sub(w).max(1);
        let max_y = scr.height.saturating_sub(h + taskbar).max(1);
        let x = (16 + n * 4) % max_x;
        let y = (1 + n * 2) % max_y;
        self.windows.push(Win {
            id: self.next_id,
            rect: Rect::new(x.max(0), y.max(0), w, h),
            minimized: false,
            restore: None,
            icon,
            launch: saved,
            app,
        });
        self.next_id += 1;
    }

    fn toggle_max(&mut self, i: usize) {
        let taskbar_y = self.screen.height.saturating_sub(1);
        if let Some(prev) = self.windows[i].restore.take() {
            self.windows[i].rect = prev;
        } else {
            self.windows[i].restore = Some(self.windows[i].rect);
            self.windows[i].rect = Rect::new(0, 0, self.screen.width, taskbar_y);
        }
    }

    /// Maximize via edge-snap (always stores restore if missing).
    fn snap_window_max(&mut self, id: u64) {
        let taskbar_y = self.screen.height.saturating_sub(1);
        if let Some(i) = self.windows.iter().position(|w| w.id == id) {
            if self.windows[i].restore.is_none() {
                self.windows[i].restore = Some(self.windows[i].rect);
            }
            self.windows[i].minimized = false;
            self.windows[i].rect = Rect::new(0, 0, self.screen.width, taskbar_y);
            let w = self.windows.remove(i);
            self.windows.push(w);
            self.toast("maximized");
        }
    }

    /// If the window is maximized, snapped, or tiled, shrink it back to a
    /// floating size under the cursor so it can be dragged normally.
    fn unsnap_if_needed(&mut self, id: u64, col: u16) {
        let taskbar_y = self.screen.height.saturating_sub(1);
        let Some(i) = self.windows.iter().position(|w| w.id == id) else {
            return;
        };
        let old = self.windows[i].rect;
        let prev = match self.windows[i].restore.take() {
            Some(r) => r,
            None if self.is_arranged(&old, taskbar_y) => {
                // No saved size (e.g. old tile) — invent a comfortable float size.
                let fw = (self.screen.width * 2 / 3).clamp(40, self.screen.width.max(40));
                let fh = (taskbar_y * 2 / 3).clamp(12, taskbar_y.max(12));
                Rect::new(0, 0, fw, fh)
            }
            None => return,
        };
        // Preserve horizontal grab ratio so the cursor stays on the title.
        let grab = col.saturating_sub(old.x);
        let ratio = if old.width > 0 {
            (grab as f32 / old.width as f32).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let mut r = prev;
        r.width = r.width.clamp(24, self.screen.width.max(24));
        r.height = r.height.clamp(6, taskbar_y.max(6));
        let dx = ((r.width as f32) * ratio) as u16;
        r.x = col.saturating_sub(dx).min(self.screen.width.saturating_sub(r.width));
        r.y = 1u16.min(taskbar_y.saturating_sub(r.height));
        self.windows[i].rect = r;
        if let Some(Drag::Move { dx, .. }) = &mut self.drag {
            *dx = col.saturating_sub(r.x).min(r.width.saturating_sub(1));
        }
    }

    /// True when a window looks layout-managed (max / snap / tile), so a drag
    /// should pull it back to a floating size.
    fn is_arranged(&self, r: &Rect, taskbar_y: u16) -> bool {
        let touches_top = r.y <= 1;
        let tall = r.height + r.y >= taskbar_y.saturating_sub(1);
        let wide = r.width >= self.screen.width.saturating_sub(2)
            || r.width >= self.screen.width / 2;
        let flush_left = r.x <= 1;
        let flush_right = r.x + r.width + 1 >= self.screen.width;
        touches_top && tall && (wide || flush_left || flush_right)
    }

    fn snap_window(&mut self, id: u64, left: bool) {
        let taskbar_y = self.screen.height.saturating_sub(1);
        if let Some(i) = self.windows.iter().position(|w| w.id == id) {
            self.windows[i].minimized = false;
            if self.windows[i].restore.is_none() {
                self.windows[i].restore = Some(self.windows[i].rect);
            }
            let half = (self.screen.width / 2).max(24);
            self.windows[i].rect = if left {
                Rect::new(0, 0, half, taskbar_y)
            } else {
                Rect::new(self.screen.width.saturating_sub(half), 0, half, taskbar_y)
            };
            let w = self.windows.remove(i);
            self.windows.push(w);
            self.toast(if left { "snapped left" } else { "snapped right" });
        }
    }

    fn show_desktop(&mut self) {
        if let Some(ids) = self.desk_restore.take() {
            for id in ids {
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                    w.minimized = false;
                }
            }
            self.toast("restore windows");
        } else {
            let mut ids = Vec::new();
            for w in &mut self.windows {
                if !w.minimized {
                    w.minimized = true;
                    ids.push(w.id);
                }
            }
            if ids.is_empty() {
                self.toast("desktop already clear");
            } else {
                self.desk_restore = Some(ids);
                self.toast("show desktop");
            }
        }
    }

    fn tile_windows(&mut self) {
        let visible: Vec<usize> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.minimized)
            .map(|(i, _)| i)
            .collect();
        if visible.is_empty() {
            self.toast("no windows to tile");
            return;
        }
        let taskbar_y = self.screen.height.saturating_sub(1);
        let n = visible.len();
        let cols = (n as f64).sqrt().ceil() as usize;
        let rows = (n + cols - 1) / cols;
        let cell_w = (self.screen.width / cols as u16).max(24);
        let cell_h = (taskbar_y / rows as u16).max(6);
        for (k, &i) in visible.iter().enumerate() {
            let c = (k % cols) as u16;
            let r = (k / cols) as u16;
            // Keep pre-tile size so title-bar drag can unsnap later
            if self.windows[i].restore.is_none() {
                self.windows[i].restore = Some(self.windows[i].rect);
            }
            self.windows[i].rect = Rect::new(
                c * cell_w,
                r * cell_h,
                if c + 1 == cols as u16 {
                    self.screen.width.saturating_sub(c * cell_w)
                } else {
                    cell_w
                }
                .max(24),
                if r + 1 == rows as u16 {
                    taskbar_y.saturating_sub(r * cell_h)
                } else {
                    cell_h
                }
                .max(6),
            );
        }
        self.toast("tiled");
    }

    fn cascade_windows(&mut self) {
        let taskbar_y = self.screen.height.saturating_sub(1);
        let mut n = 0u16;
        for w in &mut self.windows {
            if w.minimized {
                continue;
            }
            if w.restore.is_none() {
                w.restore = Some(w.rect);
            }
            let ww = (self.screen.width * 2 / 3).max(40).min(self.screen.width);
            let hh = (taskbar_y * 2 / 3).max(12).min(taskbar_y);
            w.rect = Rect::new(
                (4 + n * 3).min(self.screen.width.saturating_sub(ww)),
                (1 + n * 2).min(taskbar_y.saturating_sub(hh)),
                ww,
                hh,
            );
            n += 1;
        }
        self.toast("cascaded");
    }

    fn snap_one_icon(&mut self, idx: usize) {
        let step_x = ICON_W + ICON_GAP;
        let step_y = ICON_H + ICON_GAP;
        if let Some(icon) = self.icons.get_mut(idx) {
            icon.x = ((icon.x + step_x / 2) / step_x) * step_x;
            icon.y = ((icon.y.max(1) + step_y / 2) / step_y) * step_y;
            if icon.y == 0 {
                icon.y = 1;
            }
        }
    }

    fn preview_snap_at(&self, col: u16, _row: u16) -> Option<Rect> {
        let edge = 3u16;
        let taskbar_y = self.screen.height.saturating_sub(1);
        let half = (self.screen.width / 2).max(24);
        if col <= edge {
            Some(Rect::new(0, 0, half, taskbar_y))
        } else if col + edge >= self.screen.width {
            Some(Rect::new(
                self.screen.width.saturating_sub(half),
                0,
                half,
                taskbar_y,
            ))
        } else if _row <= edge {
            Some(Rect::new(0, 0, self.screen.width, taskbar_y))
        } else {
            None
        }
    }

    fn focus_id(&mut self, id: u64) {
        if let Some(i) = self.windows.iter().position(|w| w.id == id) {
            self.windows[i].minimized = false;
            let w = self.windows.remove(i);
            self.windows.push(w);
            self.desk_restore = None;
        }
    }

    fn focus_or_minimize(&mut self, id: u64) {
        if let Some(i) = self.windows.iter().position(|w| w.id == id) {
            if self.windows[i].minimized {
                self.windows[i].minimized = false;
                let w = self.windows.remove(i);
                self.windows.push(w);
            } else if Some(i) == self.top_idx() {
                self.windows[i].minimized = true;
            } else {
                let w = self.windows.remove(i);
                self.windows.push(w);
            }
            self.desk_restore = None;
        }
    }

    fn open_switcher(&mut self, reverse: bool) {
        if self.windows.is_empty() {
            self.toast("no windows");
            return;
        }
        if let Some(sw) = &mut self.switcher {
            let n = sw.ids.len().max(1);
            if reverse {
                sw.sel = sw.sel.checked_sub(1).unwrap_or(n - 1);
            } else {
                sw.sel = (sw.sel + 1) % n;
            }
            return;
        }
        // most-recently-used: reverse z-order (top first)
        let mut ids: Vec<u64> = self.windows.iter().rev().map(|w| w.id).collect();
        if ids.len() > 1 {
            // start on the window under the current top (next)
            ids.rotate_left(1);
        }
        let sel = if reverse {
            ids.len().saturating_sub(1)
        } else {
            0
        };
        self.switcher = Some(Switcher { sel, ids });
        self.start_open = false;
        self.context = None;
        self.overflow_open = false;
    }

    fn refresh_switcher(&mut self) {
        let Some(sw) = &mut self.switcher else {
            return;
        };
        sw.ids.retain(|id| self.windows.iter().any(|w| w.id == *id));
        if sw.ids.is_empty() {
            self.switcher = None;
        } else {
            sw.sel = sw.sel.min(sw.ids.len() - 1);
        }
    }

    fn close_id(&mut self, id: u64) {
        self.windows.retain(|w| w.id != id);
    }

    pub fn open_path(&mut self, p: PathBuf) {
        let launch = if p.is_dir() {
            Launch::Files(p)
        } else {
            crate::apps::launch_for_path(&p)
        };
        self.spawn(launch);
    }

    fn process(&mut self, id: u64, action: Action) {
        match action {
            Action::None => {}
            Action::Close => self.close_id(id),
            Action::Launch(l) => self.spawn(l),
            Action::BeginFileDrag { path, cut } => {
                // seed ghost near the focused window rather than screen center
                let (gx, gy) = self
                    .windows
                    .iter()
                    .find(|w| w.id == id)
                    .map(|w| (w.rect.x + w.rect.width / 2, w.rect.y + w.rect.height / 2))
                    .unwrap_or((self.screen.width / 2, self.screen.height / 2));
                self.drag = Some(Drag::File {
                    path,
                    cut,
                    from_win: Some(id),
                    ghost_x: gx,
                    ghost_y: gy,
                });
                self.toast(if cut {
                    "dragging (cut)… Esc cancel · Shift=cut"
                } else {
                    "dragging… Esc cancel · hold Shift to cut"
                });
            }
            Action::PinToDesktop(path) => {
                let (x, y) = self.next_icon_slot();
                self.add_shortcut(path, x + ICON_W / 2, y + 1);
            }
        }
    }

    fn next_icon_slot(&self) -> (u16, u16) {
        let taskbar_y = self.screen.height.saturating_sub(1);
        let mut x = 2u16;
        let mut y = 1u16;
        for _ in 0..64 {
            let clash = self
                .icons
                .iter()
                .any(|i| i.x.abs_diff(x) < ICON_W && i.y.abs_diff(y) < ICON_H);
            if !clash {
                return (x, y);
            }
            y += ICON_H + ICON_GAP;
            if y + ICON_H >= taskbar_y {
                y = 1;
                x += ICON_W + ICON_GAP;
            }
        }
        (2, 1)
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_until = Some(Instant::now());
    }

    fn desktop_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".texttop_desktop.json")
    }

    fn save_desktop(&self) {
        let icons: Vec<serde_json::Value> = self
            .icons
            .iter()
            .filter_map(|i| match &i.kind {
                DeskKind::App(a) => Some(serde_json::json!({
                    "kind": "app",
                    "action": a.as_str(),
                    "emoji": i.emoji,
                    "label": i.label,
                    "x": i.x,
                    "y": i.y,
                })),
                DeskKind::Shortcut(p) => Some(serde_json::json!({
                    "kind": "shortcut",
                    "path": p.to_string_lossy(),
                    "emoji": i.emoji,
                    "label": i.label,
                    "x": i.x,
                    "y": i.y,
                })),
            })
            .collect();
        let doc = serde_json::json!({
            "theme": self.theme,
            "icons": icons,
        });
        let _ = fs::write(Self::desktop_path(), doc.to_string());
    }

    fn load_desktop(&mut self) {
        let Ok(data) = fs::read_to_string(Self::desktop_path()) else {
            return;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
            return;
        };
        if let Some(t) = v.get("theme").and_then(|t| t.as_u64()) {
            self.theme = t as usize % THEMES.len();
        }
        let Some(arr) = v.get("icons").and_then(|a| a.as_array()) else {
            return;
        };
        let mut icons = Vec::new();
        for item in arr {
            let x = item.get("x").and_then(|x| x.as_u64()).unwrap_or(2) as u16;
            let y = item.get("y").and_then(|y| y.as_u64()).unwrap_or(1) as u16;
            let emoji = item
                .get("emoji")
                .and_then(|e| e.as_str())
                .unwrap_or("📄")
                .to_string();
            let label = item
                .get("label")
                .and_then(|l| l.as_str())
                .unwrap_or("icon")
                .to_string();
            let kind = item.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            match kind {
                "app" => {
                    if let Some(a) = item
                        .get("action")
                        .and_then(|a| a.as_str())
                        .and_then(MenuAction::from_str)
                    {
                        icons.push(DeskIcon {
                            emoji,
                            label,
                            kind: DeskKind::App(a),
                            x,
                            y,
                        });
                    }
                }
                "shortcut" => {
                    if let Some(p) = item.get("path").and_then(|p| p.as_str()) {
                        icons.push(DeskIcon {
                            emoji,
                            label,
                            kind: DeskKind::Shortcut(PathBuf::from(p)),
                            x,
                            y,
                        });
                    }
                }
                _ => {}
            }
        }
        if !icons.is_empty() {
            self.icons = icons;
        }
    }

    fn session_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".texttop_session.json")
    }

    /// Persist open windows (what they are + where) so the next start
    /// brings the whole desktop back.
    pub fn save_session(&self) {
        let wins: Vec<serde_json::Value> = self
            .windows
            .iter()
            .filter_map(|w| {
                launch_json(&w.launch).map(|l| {
                    serde_json::json!({
                        "launch": l,
                        "x": w.rect.x, "y": w.rect.y,
                        "w": w.rect.width, "h": w.rect.height,
                        "min": w.minimized,
                    })
                })
            })
            .collect();
        let doc = serde_json::json!({ "windows": wins });
        let _ = fs::write(Self::session_path(), doc.to_string());
    }

    /// Reopen the previous session; true if at least one window came back.
    fn load_session(&mut self) -> bool {
        let Ok(data) = fs::read_to_string(Self::session_path()) else {
            return false;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
            return false;
        };
        let Some(arr) = v.get("windows").and_then(|a| a.as_array()) else {
            return false;
        };
        let mut restored = false;
        for item in arr {
            let Some(launch) = item.get("launch").and_then(launch_from_json) else {
                continue;
            };
            self.spawn(launch);
            restored = true;
            if let Some(w) = self.windows.last_mut() {
                let g = |k: &str| item.get(k).and_then(|n| n.as_u64());
                if let (Some(x), Some(y), Some(ww), Some(hh)) =
                    (g("x"), g("y"), g("w"), g("h"))
                {
                    w.rect = Rect::new(x as u16, y as u16, ww as u16, hh as u16);
                }
                w.minimized = item.get("min").and_then(|m| m.as_bool()).unwrap_or(false);
            }
        }
        restored
    }

    fn sanitize(&mut self) {
        let scr = self.screen;
        if scr.width < 30 || scr.height < 8 {
            return;
        }
        let taskbar_y = scr.height.saturating_sub(1);
        for w in &mut self.windows {
            w.rect.width = w.rect.width.clamp(24, scr.width);
            w.rect.height = w.rect.height.clamp(6, taskbar_y.max(6));
            if w.rect.x + w.rect.width > scr.width {
                w.rect.x = scr.width.saturating_sub(w.rect.width);
            }
            if w.rect.y + w.rect.height > taskbar_y {
                w.rect.y = taskbar_y.saturating_sub(w.rect.height);
            }
        }
        for icon in &mut self.icons {
            icon.x = icon.x.min(scr.width.saturating_sub(ICON_W + 1));
            icon.y = icon.y.min(taskbar_y.saturating_sub(ICON_H + 1).max(1));
        }
    }
}

fn default_icons() -> Vec<DeskIcon> {
    DEFAULT_ICONS
        .iter()
        .enumerate()
        .map(|(i, (emoji, label, action))| DeskIcon {
            emoji: (*emoji).to_string(),
            label: (*label).to_string(),
            kind: DeskKind::App(*action),
            x: 2 + (i / ICONS_PER_COL) as u16 * (ICON_W + ICON_GAP),
            y: 1 + (i % ICONS_PER_COL) as u16 * (ICON_H + ICON_GAP),
        })
        .collect()
}

fn icon_rect(icon: &DeskIcon) -> Rect {
    Rect::new(icon.x, icon.y, ICON_W, ICON_H)
}

fn truncate_label(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        if out.width() + ch.width().unwrap_or(1) > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// Serialize a Launch for session restore. One-shot launchers (external
/// programs) are intentionally skipped — re-running them on start would be
/// surprising.
fn launch_json(l: &Launch) -> Option<serde_json::Value> {
    use serde_json::json;
    let s = |p: &PathBuf| p.to_string_lossy().to_string();
    let so = |p: &Option<PathBuf>| p.as_ref().map(|p| p.to_string_lossy().to_string());
    Some(match l {
        Launch::Files(p) => json!({"t": "files", "p": s(p)}),
        Launch::Web(u) => json!({"t": "web", "u": u}),
        Launch::Audio(p) => json!({"t": "audio", "p": s(p)}),
        Launch::Image(p) => json!({"t": "image", "p": s(p)}),
        Launch::Video(p) => json!({"t": "video", "p": s(p)}),
        Launch::Editor(p) => json!({"t": "editor", "p": so(p)}),
        Launch::Terminal { cwd, .. } => json!({"t": "terminal", "cwd": so(cwd)}),
        Launch::About => json!({"t": "about"}),
        Launch::Calculator => json!({"t": "calculator"}),
        Launch::Clock => json!({"t": "clock"}),
        Launch::Hex(p) => json!({"t": "hex", "p": so(p)}),
        Launch::Calendar => json!({"t": "calendar"}),
        Launch::SysInfo => json!({"t": "sysinfo"}),
        Launch::Notes => json!({"t": "notes"}),
        Launch::Git(p) => json!({"t": "git", "p": so(p)}),
        Launch::Search(p) => json!({"t": "search", "p": so(p)}),
        Launch::Processes => json!({"t": "processes"}),
        Launch::Snapshots(p) => json!({"t": "snapshots", "p": s(p)}),
        Launch::Compare(ps) => {
            json!({"t": "compare", "ps": ps.iter().map(|p| s(p)).collect::<Vec<_>>()})
        }
        Launch::Snake => json!({"t": "snake"}),
        Launch::Minesweeper => json!({"t": "mines"}),
        Launch::Game2048 => json!({"t": "2048"}),
        Launch::AppLauncher(_) => return None,
    })
}

fn launch_from_json(v: &serde_json::Value) -> Option<Launch> {
    let t = v.get("t")?.as_str()?;
    let p = || v.get("p").and_then(|p| p.as_str()).map(PathBuf::from);
    // file-backed windows only come back if the file still exists
    let pf = || p().filter(|p| p.exists());
    Some(match t {
        "files" => Launch::Files(pf()?),
        "web" => Launch::Web(v.get("u").and_then(|u| u.as_str()).map(String::from)),
        "audio" => Launch::Audio(pf()?),
        "image" => Launch::Image(pf()?),
        "video" => Launch::Video(pf()?),
        "editor" => Launch::Editor(p()),
        "terminal" => Launch::Terminal {
            program: None,
            args: Vec::new(),
            cwd: v
                .get("cwd")
                .and_then(|c| c.as_str())
                .map(PathBuf::from)
                .filter(|p| p.exists()),
        },
        "about" => Launch::About,
        "calculator" => Launch::Calculator,
        "clock" => Launch::Clock,
        "hex" => Launch::Hex(p().filter(|p| p.exists())),
        "calendar" => Launch::Calendar,
        "sysinfo" => Launch::SysInfo,
        "notes" => Launch::Notes,
        "git" => Launch::Git(p().filter(|p| p.exists())),
        "search" => Launch::Search(p().filter(|p| p.exists())),
        "processes" => Launch::Processes,
        "snapshots" => Launch::Snapshots(pf()?),
        "compare" => Launch::Compare(
            v.get("ps")?
                .as_array()?
                .iter()
                .filter_map(|p| p.as_str())
                .map(PathBuf::from)
                .filter(|p| p.exists())
                .collect(),
        ),
        "snake" => Launch::Snake,
        "mines" => Launch::Minesweeper,
        "2048" => Launch::Game2048,
        _ => return None,
    })
}

fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}
