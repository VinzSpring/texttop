use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::scroll::{self, ScrollState};
use super::{Action, AppWindow};

pub struct About {
    scroll: ScrollState,
    line_count: usize,
    view_h: u16,
}

impl About {
    pub fn new() -> Self {
        About {
            scroll: ScrollState::default(),
            line_count: 0,
            view_h: 20,
        }
    }

    fn lines() -> Vec<Line<'static>> {
        let accent = Style::default()
            .fg(Color::Rgb(0, 190, 255))
            .add_modifier(Modifier::BOLD);
        let key = Style::default()
            .fg(Color::Rgb(255, 210, 100))
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::Rgb(150, 155, 175));
        let txt = Style::default().fg(Color::Rgb(215, 218, 230));

        let kv = |k: &str, v: &str| {
            Line::from(vec![
                Span::styled(format!("   {k:<18}"), key),
                Span::styled(v.to_string(), txt),
            ])
        };

        vec![
            Line::default(),
            Line::from(Span::styled("   ▀█▀ █▀▀ ▀▄▀ ▀█▀ ▀█▀ █▀█ █▀█", accent)),
            Line::from(Span::styled("    █  ██▄ █ █  █   █  █▄█ █▀ ", accent)),
            Line::from(Span::styled(
                "   a desktop environment for your terminal",
                dim,
            )),
            Line::default(),
            Line::from(Span::styled("   Desktop", accent)),
            kv("F1", "start menu — just type to search apps"),
            kv("F12", "shortcut key overlay"),
            kv("F6 / Alt+Tab", "window switcher"),
            kv("F11", "maximize / restore window"),
            kv("Ctrl+W", "close window"),
            kv("Ctrl+D", "show / restore desktop"),
            kv("Ctrl+Shift+←/→", "snap window left / right"),
            kv("Ctrl+Shift+↑", "maximize window"),
            kv("Ctrl+arrows", "nudge window"),
            kv("Ctrl+Q", "quit texttop"),
            kv("F5", "prune dead desktop shortcuts"),
            kv("Delete", "remove selected desktop shortcut"),
            kv("Esc", "cancel drag / close menus"),
            Line::default(),
            Line::from(Span::styled("   Mouse", accent)),
            kv(
                "right-click",
                "context menus (desktop · icons · windows · files)",
            ),
            kv("drag title bar", "move · edge snap · top = maximize"),
            kv("double-click title", "maximize / restore"),
            kv("drag ◢ corner", "resize window"),
            kv("─ □ ✕", "minimize · maximize · close"),
            kv("drag icons", "custom placement · right-click Arrange to pack"),
            kv("drag files", "drop on desktop / apps · Shift=cut"),
            kv("taskbar »N", "overflow window list"),
            kv("middle-click taskbar", "close window"),
            kv("double-click", "open desktop icons & files"),
            kv("mouse wheel", "scroll overflow content in apps"),
            Line::default(),
            Line::from(Span::styled("   Apps", accent)),
            kv(
                "🗂 Files",
                "filter, rename, copy/paste, preview, bookmarks",
            ),
            kv(
                "🌐 Web",
                "real browser (JS+media) · F4 url · F3 crisp text",
            ),
            kv("🖥 Terminal", "full shell inside a window"),
            kv(
                "🧩 Programs",
                "run apps: terminal, args form, or detached",
            ),
            kv("🎵 Audio", "mp3/wav/flac/ogg with visualizer"),
            kv("🖼 Images", "zoom · pan · gallery · slideshow · ? help"),
            kv("🎬 Video", "built-in engine playback with sound"),
            kv(
                "📝 Editor",
                "syntax colors by file type · Ctrl+S save · Ctrl+H toggle",
            ),
            kv(
                "🧮 Calculator",
                "expressions with + − × ÷ ^ % ( ) · history tape",
            ),
            kv("⏱ Clock", "clock, stopwatch, and countdown timer"),
            kv(
                "📅 Calendar",
                "month view — navigate days/months/years",
            ),
            kv("📌 Notes", "sticky pad saved to ~/.texttop_notes.txt"),
            kv("🔢 Hex", "binary dump — open from Files with F3"),
            kv("💻 System", "OS, paths, folder summaries, env"),
            kv("🌿 Git", "status · log · diff · branches · commit (needs git on PATH)"),
            kv("🔎 Search", "project search via ripgrep or builtin walk"),
            kv("⚙ Processes", "list / filter / kill processes"),
            kv(
                "⏱ Snapshots",
                "file history in ~/.texttop_snap — restore · version diffs",
            ),
            kv(
                "↔ Compare",
                "visual diff of 2+ marked files (pick A/B)",
            ),
            kv("🐍 Snake", "arrows steer · gets faster as you eat"),
            kv("💣 Minesweeper", "click/flag mines · 3 difficulties"),
            kv("🎮 2048", "slide & merge tiles to 2048"),
            kv(
                "🎨 Wallpaper",
                "themes + image… tile opens Files to pick a photo",
            ),
            Line::default(),
            Line::from(Span::styled("   File Manager", accent)),
            kv("/ or Ctrl+F", "filter · F2 rename · Del delete"),
            kv("F7 / F8", "new folder / new file"),
            kv("Ctrl+C/X/V", "copy · cut · paste"),
            kv("Space / Ctrl+A", "mark file · select all · Esc clear"),
            kv(
                "right-click",
                "snapshot · history · compare · git · pin",
            ),
            kv("Ctrl+G", "open Git app for this folder"),
            kv("Ctrl+P / Tab", "preview pane · cycle sort"),
            kv("Ctrl+L / Ctrl+T", "go to path · terminal here"),
            kv("Alt+H/D/O/L", "home · desktop · docs · downloads"),
            Line::default(),
            Line::from(Span::styled(
                "   Taskbar shows ⎇ branch* when you're in a git repo — click to open Git",
                dim,
            )),
            Line::from(Span::styled(
                "   Drop files onto Editor · Hex · Image · Audio · Video · Web · Terminal",
                dim,
            )),
            Line::from(Span::styled(
                "   Icons & theme persist in ~/.texttop_desktop.json;",
                dim,
            )),
            Line::from(Span::styled(
                "   open windows are restored on next start (Exit via menu).",
                dim,
            )),
            Line::from(Span::styled(
                "   Press ? in Files · Git · Images for per-app keymaps.",
                dim,
            )),
            Line::from(Span::styled(
                "   Everything is built in — no external programs",
                dim,
            )),
            Line::from(Span::styled(
                "   needed (except system git / optional rg).",
                dim,
            )),
            Line::default(),
            Line::from(Span::styled(
                "   ↑↓ / PgUp/PgDn / mouse wheel — scroll this help",
                dim,
            )),
        ]
    }
}

impl AppWindow for About {
    fn title(&self) -> String {
        "Welcome".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        let lines = Self::lines();
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
        self.scroll.on_mouse(m, self.line_count, self.view_h);
        Action::None
    }
}
