use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};

pub struct About {
    scroll: u16,
}

impl About {
    pub fn new() -> Self {
        About { scroll: 0 }
    }
}

impl AppWindow for About {
    fn title(&self) -> String {
        "Welcome".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        let accent = Style::default().fg(Color::Rgb(0, 190, 255)).add_modifier(Modifier::BOLD);
        let key = Style::default().fg(Color::Rgb(255, 210, 100)).add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::Rgb(150, 155, 175));
        let txt = Style::default().fg(Color::Rgb(215, 218, 230));

        let kv = |k: &str, v: &str| {
            Line::from(vec![
                Span::styled(format!("   {k:<18}"), key),
                Span::styled(v.to_string(), txt),
            ])
        };

        let lines: Vec<Line> = vec![
            Line::default(),
            Line::from(Span::styled("   ▀█▀ █▀▀ ▀▄▀ ▀█▀ ▀█▀ █▀█ █▀█", accent)),
            Line::from(Span::styled("    █  ██▄ █ █  █   █  █▄█ █▀ ", accent)),
            Line::from(Span::styled("   a desktop environment for your terminal", dim)),
            Line::default(),
            Line::from(Span::styled("   Desktop", accent)),
            kv("F1", "open the start menu"),
            kv("F6", "cycle windows"),
            kv("F11", "maximize / restore window"),
            kv("Ctrl+W", "close window"),
            kv("Ctrl+arrows", "move window"),
            kv("Ctrl+Q", "quit texttop"),
            Line::default(),
            Line::from(Span::styled("   Mouse", accent)),
            kv("drag title bar", "move window"),
            kv("drag ◢ corner", "resize window"),
            kv("─ □ ✕", "minimize · maximize · close"),
            kv("double-click", "open desktop icons & files"),
            Line::default(),
            Line::from(Span::styled("   Apps", accent)),
            kv("🗂 Files", "filter, rename, copy/paste, preview, bookmarks"),
            kv("🌐 Web", "real browser (JS+media) · F4 url · F3 crisp text"),
            kv("🖥 Terminal", "full shell inside a window"),
            kv("🧩 Programs", "run apps: terminal, args form, or detached"),
            kv("🎵 Audio", "mp3/wav/flac/ogg with visualizer"),
            kv("🖼 Images", "quadrant-pixel color rendering"),
            kv("🎬 Video", "built-in engine playback with sound"),
            kv("📝 Editor", "syntax colors by file type · Ctrl+S save · Ctrl+H toggle"),
            kv("🧮 Calculator", "expressions with + − × ÷ ^ % and ( )"),
            kv("⏱ Clock", "clock, stopwatch, and countdown timer"),
            kv("📅 Calendar", "month view — navigate days/months/years"),
            kv("📌 Notes", "sticky pad saved to ~/.texttop_notes.txt"),
            kv("🔢 Hex", "binary dump — open from Files with F3"),
            kv("💻 System", "OS, paths, folder summaries, env"),
            Line::default(),
            Line::from(Span::styled("   File Manager", accent)),
            kv("/ or Ctrl+F", "filter · F2 rename · Del delete"),
            kv("F7 / F8", "new folder / new file"),
            kv("Ctrl+C/X/V", "copy · cut · paste"),
            kv("Ctrl+P / Tab", "preview pane · cycle sort"),
            kv("Ctrl+L / Ctrl+T", "go to path · terminal here"),
            kv("Alt+H/D/O/L", "home · desktop · docs · downloads"),
            Line::default(),
            Line::from(Span::styled("   Everything is built in — no external programs", dim)),
            Line::from(Span::styled("   needed. Open files from the file manager.", dim)),
        ];
        f.render_widget(Paragraph::new(lines).scroll((self.scroll, 0)), area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll = (self.scroll + 1).min(48),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }
}
