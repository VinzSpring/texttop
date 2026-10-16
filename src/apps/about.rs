use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::scroll::{self, ScrollState};
use super::{Action, AppWindow, Launch};

/// Welcome page: how to get around with the mouse, and every built-in app as
/// a clickable row that opens it.
pub struct About {
    scroll: ScrollState,
    line_count: usize,
    view_h: u16,
    area: Rect,
    /// Line under the pointer, highlighted when it opens an app.
    hover: Option<usize>,
}

impl About {
    pub fn new() -> Self {
        About {
            scroll: ScrollState::default(),
            line_count: 0,
            view_h: 20,
            area: Rect::default(),
            hover: None,
        }
    }

    /// The page, with the app each line opens (if any).
    fn lines(&self) -> Vec<(Line<'static>, Option<Launch>)> {
        let accent = Style::default()
            .fg(Color::Rgb(0, 190, 255))
            .add_modifier(Modifier::BOLD);
        let key = Style::default()
            .fg(Color::Rgb(255, 210, 100))
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::Rgb(150, 155, 175));
        let txt = Style::default().fg(Color::Rgb(215, 218, 230));

        // pad by display width: emoji are one or two cells
        let pad = |k: &str| format!("   {k}{}", " ".repeat(20usize.saturating_sub(k.width())));
        let kv = |k: &str, v: &str| {
            (
                Line::from(vec![
                    Span::styled(pad(k), key),
                    Span::styled(v.to_string(), txt),
                ]),
                None,
            )
        };
        let plain = |s: &str, st: Style| (Line::from(Span::styled(s.to_string(), st)), None);
        let blank = || (Line::default(), None);

        let home = dirs::home_dir().unwrap_or_else(|| ".".into());
        let apps: Vec<(&str, &str, Option<Launch>)> = vec![
            ("🗂 Files", "browse, preview, copy / move, snapshots, compare", Some(Launch::Files(home))),
            ("🌐 Web", "real browser with JavaScript and media", Some(Launch::Web(None))),
            ("🖥 Terminal", "full shell inside a window", Some(Launch::Terminal { program: None, args: Vec::new(), cwd: None })),
            ("📝 Editor", "syntax colors by file type", Some(Launch::Editor(None))),
            ("🛠 IDE", "explorer, terminal and tabs in one window", Some(Launch::Group { id: super::group::IDE.into(), cwd: None, open: Vec::new() })),
            ("📌 Notes", "a sticky pad that keeps what you write", Some(Launch::Notes)),
            ("🎛 Studio", "electronic music: clips, piano roll, export WAV", Some(Launch::Studio(None))),
            ("🧮 Calculator", "expressions and a history tape", Some(Launch::Calculator)),
            ("⏱ Clock", "clock, stopwatch and countdown timer", Some(Launch::Clock)),
            ("📅 Calendar", "month view", Some(Launch::Calendar)),
            ("🌿 Git", "status · log · diff · branches · commit", Some(Launch::Git(None))),
            ("🔎 Search", "find text across a folder", Some(Launch::Search(None))),
            ("⚙ Processes", "list, filter and end processes", Some(Launch::Processes)),
            ("💻 System", "OS, paths, folders, environment", Some(Launch::SysInfo)),
            ("🔢 Hex", "binary view of any file", Some(Launch::Hex(None))),
            ("🪟 App Host", "show a real desktop app inside a window", Some(Launch::AppHost { cmd: None })),
            ("🤝 Contribute", "get texttop's source and change it in the IDE", Some(Launch::Contribute)),
            ("🧩 Detected apps", "find CLI tools on PATH and pin them", Some(Launch::DetectedApps)),
            ("🎨 Wallpaper", "themes or your own picture", Some(Launch::Wallpaper)),
            ("🐍 Snake", "click beside the head to turn", Some(Launch::Snake)),
            ("💣 Minesweeper", "three sizes, flag mode for one-button play", Some(Launch::Minesweeper)),
            ("🎮 2048", "swipe the board to slide tiles", Some(Launch::Game2048)),
            ("🎵 🖼 🎬 Media", "audio, images and video open from Files", None),
        ];

        let mut out = vec![
            blank(),
            plain("   ▀█▀ █▀▀ ▀▄▀ ▀█▀ ▀█▀ █▀█ █▀█", accent),
            plain("    █  ██▄ █ █  █   █  █▄█ █▀ ", accent),
            plain("   a desktop environment for your terminal", dim),
            blank(),
            plain("   Getting around", accent),
            kv("Start button", "every app, plus search"),
            kv("right-click", "menus on the desktop, icons, windows and files"),
            kv("drag a title bar", "move · drop on an edge to snap · top to maximize"),
            kv("double-click title", "maximize / restore"),
            kv("drag ◢ corner", "resize"),
            kv("─ □ ✕", "minimize · maximize · close"),
            kv("drag files", "onto the desktop or into apps"),
            kv("taskbar", "click to switch · middle-click to close"),
            kv("toolbars", "each app keeps its actions in buttons up top"),
            blank(),
            plain("   Apps — click one to open it", accent),
        ];
        for (name, desc, launch) in apps {
            out.push((
                Line::from(vec![
                    Span::styled(pad(name), key),
                    Span::styled(desc.to_string(), txt),
                ]),
                launch,
            ));
        }
        out.extend([
            blank(),
            plain("   Icons, wallpaper and open windows come back on the next start.", dim),
            plain("   Everything is built in — no external programs needed", dim),
            plain("   (Git and Search use git / ripgrep when installed).", dim),
        ]);
        out
    }

    fn line_at(&self, row: u16) -> Option<usize> {
        let a = self.area;
        (row >= a.y && row < a.y + a.height).then(|| self.scroll.offset as usize + (row - a.y) as usize)
    }
}

impl AppWindow for About {
    fn title(&self) -> String {
        "Welcome".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        self.area = area;
        let hover = self.hover;
        let lines: Vec<Line> = self
            .lines()
            .into_iter()
            .enumerate()
            .map(|(i, (line, launch))| {
                if launch.is_some() && hover == Some(i) {
                    line.style(Style::default().bg(Color::Rgb(0, 90, 140)))
                } else {
                    line
                }
            })
            .collect();
        self.line_count = lines.len();
        self.view_h = area.height;
        scroll::render_scrollable(f, area, &lines, &mut self.scroll);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        if self.scroll.on_key(k, self.line_count, self.view_h) {
            return Action::None;
        }
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => Action::Close,
            _ => Action::None,
        }
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        if self.scroll.on_mouse(m, self.line_count, self.view_h) {
            return Action::None;
        }
        match m.kind {
            MouseEventKind::Moved => self.hover = self.line_at(m.row),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(launch) = self
                    .line_at(m.row)
                    .and_then(|i| self.lines().into_iter().nth(i))
                    .and_then(|(_, l)| l)
                {
                    return Action::Launch(launch);
                }
            }
            _ => {}
        }
        Action::None
    }
}
