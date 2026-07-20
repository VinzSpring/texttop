use chrono::{Datelike, Duration, Local, NaiveDate};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::scroll::{self, ScrollState};
use super::{Action, AppWindow};

pub struct Calendar {
    cursor: NaiveDate,
    scroll: ScrollState,
    line_count: usize,
    view_h: u16,
}

impl Calendar {
    pub fn new() -> Self {
        Calendar {
            cursor: Local::now().date_naive(),
            scroll: ScrollState::default(),
            line_count: 0,
            view_h: 16,
        }
    }

    fn month_matrix(year: i32, month: u32) -> Vec<Vec<Option<u32>>> {
        let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
        let start_wd = first.weekday().num_days_from_sunday(); // 0=Sun
        let days = days_in_month(year, month);
        let mut weeks = Vec::new();
        let mut week = vec![None; start_wd as usize];
        for d in 1..=days {
            week.push(Some(d));
            if week.len() == 7 {
                weeks.push(week);
                week = Vec::new();
            }
        }
        if !week.is_empty() {
            while week.len() < 7 {
                week.push(None);
            }
            weeks.push(week);
        }
        weeks
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap();
    let last = next - Duration::days(1);
    last.day()
}

impl AppWindow for Calendar {
    fn title(&self) -> String {
        self.cursor.format("%B %Y").to_string()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        let today = Local::now().date_naive();
        let year = self.cursor.year();
        let month = self.cursor.month();
        let weeks = Self::month_matrix(year, month);

        let accent = Style::default()
            .fg(Color::Rgb(120, 200, 255))
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::Rgb(120, 130, 155));
        let day_s = Style::default().fg(Color::Rgb(210, 215, 230));
        let today_s = Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(0, 190, 255))
            .add_modifier(Modifier::BOLD);
        let sel_s = Style::default()
            .fg(Color::White)
            .bg(Color::Rgb(60, 80, 140))
            .add_modifier(Modifier::BOLD);
        let weekend = Style::default().fg(Color::Rgb(255, 160, 140));

        let mut lines = vec![
            Line::from(Span::styled(
                format!("  {}", self.cursor.format("%B %Y")),
                accent,
            )),
            Line::default(),
            Line::from(Span::styled(
                "     Su  Mo  Tu  We  Th  Fr  Sa",
                dim,
            )),
        ];

        for week in &weeks {
            let mut spans = vec![Span::raw("    ")];
            for (i, day) in week.iter().enumerate() {
                match day {
                    None => spans.push(Span::raw("    ")),
                    Some(d) => {
                        let date = NaiveDate::from_ymd_opt(year, month, *d).unwrap();
                        let style = if date == today {
                            today_s
                        } else if date == self.cursor {
                            sel_s
                        } else if i == 0 || i == 6 {
                            weekend
                        } else {
                            day_s
                        };
                        spans.push(Span::styled(format!(" {d:2} "), style));
                    }
                }
            }
            lines.push(Line::from(spans));
        }

        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!("  Selected: {}", self.cursor.format("%A, %Y-%m-%d")),
            accent,
        )));
        if self.cursor == today {
            lines.push(Line::from(Span::styled("  (today)", dim)));
        } else {
            let delta = (self.cursor - today).num_days();
            let label = if delta > 0 {
                format!("  ({delta} days ahead)")
            } else {
                format!("  ({} days ago)", -delta)
            };
            lines.push(Line::from(Span::styled(label, dim)));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  ←→ day · ↑↓ week · [/] month · T today · PgUp/PgDn year",
            dim,
        )));
        lines.push(Line::from(Span::styled(
            "  mouse wheel / Ctrl+↑↓ scroll if clipped",
            dim,
        )));

        self.line_count = lines.len();
        self.view_h = area.height;
        scroll::render_scrollable(f, area, &lines, &mut self.scroll);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl
            && matches!(k.code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown)
        {
            self.scroll.on_key(
                KeyEvent::new(
                    match k.code {
                        KeyCode::Up => KeyCode::Up,
                        KeyCode::Down => KeyCode::Down,
                        KeyCode::PageUp => KeyCode::PageUp,
                        _ => KeyCode::PageDown,
                    },
                    KeyModifiers::NONE,
                ),
                self.line_count,
                self.view_h,
            );
            return Action::None;
        }
        match k.code {
            KeyCode::Left => self.cursor -= Duration::days(1),
            KeyCode::Right => self.cursor += Duration::days(1),
            KeyCode::Up => self.cursor -= Duration::days(7),
            KeyCode::Down => self.cursor += Duration::days(7),
            KeyCode::Char('[') => {
                let m = self.cursor.month();
                let y = self.cursor.year();
                let (ny, nm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
                let day = self.cursor.day().min(days_in_month(ny, nm));
                self.cursor = NaiveDate::from_ymd_opt(ny, nm, day).unwrap();
            }
            KeyCode::Char(']') => {
                let m = self.cursor.month();
                let y = self.cursor.year();
                let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                let day = self.cursor.day().min(days_in_month(ny, nm));
                self.cursor = NaiveDate::from_ymd_opt(ny, nm, day).unwrap();
            }
            KeyCode::PageUp => {
                let y = self.cursor.year() - 1;
                let m = self.cursor.month();
                let day = self.cursor.day().min(days_in_month(y, m));
                self.cursor = NaiveDate::from_ymd_opt(y, m, day).unwrap();
            }
            KeyCode::PageDown => {
                let y = self.cursor.year() + 1;
                let m = self.cursor.month();
                let day = self.cursor.day().min(days_in_month(y, m));
                self.cursor = NaiveDate::from_ymd_opt(y, m, day).unwrap();
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                self.cursor = Local::now().date_naive();
            }
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        self.scroll.on_mouse(m, self.line_count, self.view_h);
        Action::None
    }
}
