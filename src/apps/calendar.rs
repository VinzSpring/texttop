use chrono::{Datelike, Duration, Local, NaiveDate};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;

use super::ui::{Controls, Item, Look};
use super::{Action, AppWindow};

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    PrevYear,
    PrevMonth,
    Today,
    NextMonth,
    NextYear,
    Day(NaiveDate),
}

pub struct Calendar {
    cursor: NaiveDate,
    ui: Controls<Cmd>,
}

impl Calendar {
    pub fn new() -> Self {
        Calendar {
            cursor: Local::now().date_naive(),
            ui: Controls::new(),
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

    /// Move by whole months, clamping the day to the target month's length.
    fn shift_months(&mut self, delta: i32) {
        let total = self.cursor.year() * 12 + self.cursor.month0() as i32 + delta;
        let (y, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
        let day = self.cursor.day().min(days_in_month(y, m));
        self.cursor = NaiveDate::from_ymd_opt(y, m, day).unwrap();
    }

    fn run(&mut self, c: Cmd) {
        match c {
            Cmd::PrevYear => self.shift_months(-12),
            Cmd::PrevMonth => self.shift_months(-1),
            Cmd::Today => self.cursor = Local::now().date_naive(),
            Cmd::NextMonth => self.shift_months(1),
            Cmd::NextYear => self.shift_months(12),
            Cmd::Day(d) => self.cursor = d,
        }
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
        if area.height < 2 || area.width < 10 {
            return;
        }
        self.ui.begin();
        let today = Local::now().date_naive();
        let year = self.cursor.year();
        let month = self.cursor.month();
        let weeks = Self::month_matrix(year, month);

        let buf = f.buffer_mut();
        self.ui.toolbar(
            buf,
            Rect::new(area.x, area.y, area.width, 1),
            &[
                Item::btn("◀◀", Cmd::PrevYear),
                Item::btn("◀", Cmd::PrevMonth),
                Item::Label(
                    format!(" {} ", self.cursor.format("%B %Y")),
                    Color::Rgb(120, 200, 255),
                ),
                Item::btn("▶", Cmd::NextMonth),
                Item::btn("▶▶", Cmd::NextYear),
            ],
            &[Item::with("Today", Cmd::Today, Look::on_if(self.cursor == today))],
        );

        let dim = Style::default().fg(Color::Rgb(120, 130, 155));
        let accent = Style::default()
            .fg(Color::Rgb(120, 200, 255))
            .add_modifier(Modifier::BOLD);
        let day_s = Style::default().fg(Color::Rgb(210, 215, 230));
        let today_s = Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(0, 190, 255))
            .add_modifier(Modifier::BOLD);
        let sel_s = Style::default()
            .fg(Color::White)
            .bg(Color::Rgb(60, 80, 140))
            .add_modifier(Modifier::BOLD);
        let hover_s = Style::default().fg(Color::White).bg(Color::Rgb(0, 90, 140));
        let weekend = Style::default().fg(Color::Rgb(255, 160, 140));

        // Day cells are 4 wide; center the 28-cell grid.
        let gx = area.x + area.width.saturating_sub(28) / 2;
        let bottom = area.y + area.height;
        let mut y = area.y + 2;
        let put = |buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, s: &str, st: Style| {
            if y < bottom {
                buf.set_string(x, y, s, st);
            }
        };
        put(buf, gx, y, " Su  Mo  Tu  We  Th  Fr  Sa", dim);
        y += 1;
        for week in &weeks {
            if y >= bottom {
                break;
            }
            for (i, day) in week.iter().enumerate() {
                let Some(d) = day else { continue };
                let date = NaiveDate::from_ymd_opt(year, month, *d).unwrap();
                let cell = Rect::new(gx + i as u16 * 4, y, 4, 1);
                let style = if date == today {
                    today_s
                } else if date == self.cursor {
                    sel_s
                } else if self.ui.hot(cell) {
                    hover_s
                } else if i == 0 || i == 6 {
                    weekend
                } else {
                    day_s
                };
                buf.set_string(cell.x, y, format!(" {d:2} "), style);
                self.ui.add(cell, Cmd::Day(date));
            }
            y += 1;
        }

        y += 1;
        put(
            buf,
            gx,
            y,
            &self.cursor.format("%A, %Y-%m-%d").to_string(),
            accent,
        );
        let rel = match (self.cursor - today).num_days() {
            0 => "today".to_string(),
            1 => "tomorrow".to_string(),
            -1 => "yesterday".to_string(),
            d if d > 0 => format!("in {d} days"),
            d => format!("{} days ago", -d),
        };
        put(buf, gx, y + 1, &rel, dim);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Left => self.cursor -= Duration::days(1),
            KeyCode::Right => self.cursor += Duration::days(1),
            KeyCode::Up => self.cursor -= Duration::days(7),
            KeyCode::Down => self.cursor += Duration::days(7),
            KeyCode::Char('[') => self.run(Cmd::PrevMonth),
            KeyCode::Char(']') => self.run(Cmd::NextMonth),
            KeyCode::PageUp => self.run(Cmd::PrevYear),
            KeyCode::PageDown => self.run(Cmd::NextYear),
            KeyCode::Char('t') | KeyCode::Char('T') => self.run(Cmd::Today),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        match m.kind {
            MouseEventKind::ScrollUp => self.run(Cmd::PrevMonth),
            MouseEventKind::ScrollDown => self.run(Cmd::NextMonth),
            _ => {
                if let Some(Some(c)) = self.ui.route(&m) {
                    self.run(c);
                }
            }
        }
        Action::None
    }
}
