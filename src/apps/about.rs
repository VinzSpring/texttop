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
            kv("🗂 Files", "browse drives, Enter opens with the right app"),
            kv("🌐 Web", "real browser (JS+media) · F4 url · F3 crisp text"),
            kv("🖥 Terminal", "full shell inside a window"),
            kv("🧩 Programs", "run apps: terminal, args form, or detached"),
            kv("🎵 Audio", "mp3/wav/flac/ogg with visualizer"),
            kv("🖼 Images", "quadrant-pixel color rendering"),
            kv("🎬 Video", "built-in engine playback with sound"),
            kv("📝 Editor", "edit text, Ctrl+S saves"),
            Line::default(),
            Line::from(Span::styled("   Everything is built in — no external programs", dim)),
            Line::from(Span::styled("   needed. Open files from the file manager.", dim)),
        ];
        f.render_widget(Paragraph::new(lines).scroll((self.scroll, 0)), area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll = (self.scroll + 1).min(24),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }
}
