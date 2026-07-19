use std::time::Instant;

use chrono::{Datelike, Local};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Clock,
    Stopwatch,
    Timer,
}

pub struct ClockApp {
    mode: Mode,
    sw_running: bool,
    sw_started: Option<Instant>,
    sw_elapsed_ms: u128,
    timer_secs: u64,
    timer_left_ms: u128,
    timer_running: bool,
    timer_started: Option<Instant>,
    timer_done: bool,
}

impl ClockApp {
    pub fn new() -> Self {
        ClockApp {
            mode: Mode::Clock,
            sw_running: false,
            sw_started: None,
            sw_elapsed_ms: 0,
            timer_secs: 5 * 60,
            timer_left_ms: 5 * 60 * 1000,
            timer_running: false,
            timer_started: None,
            timer_done: false,
        }
    }

    fn stopwatch_ms(&self) -> u128 {
        let mut ms = self.sw_elapsed_ms;
        if let Some(t) = self.sw_started {
            if self.sw_running {
                ms += t.elapsed().as_millis();
            }
        }
        ms
    }

    fn timer_remaining_ms(&self) -> u128 {
        if !self.timer_running {
            return self.timer_left_ms;
        }
        let Some(t) = self.timer_started else {
            return self.timer_left_ms;
        };
        self.timer_left_ms
            .saturating_sub(t.elapsed().as_millis())
    }

    fn fmt_ms(ms: u128) -> String {
        let total = ms / 1000;
        let h = total / 3600;
        let m = (total % 3600) / 60;
        let s = total % 60;
        let frac = (ms % 1000) / 10;
        if h > 0 {
            format!("{h:02}:{m:02}:{s:02}.{frac:02}")
        } else {
            format!("{m:02}:{s:02}.{frac:02}")
        }
    }
}

impl AppWindow for ClockApp {
    fn title(&self) -> String {
        match self.mode {
            Mode::Clock => "Clock".into(),
            Mode::Stopwatch => "Stopwatch".into(),
            Mode::Timer => "Timer".into(),
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        let now = Local::now();
        let accent = Style::default()
            .fg(Color::Rgb(255, 190, 90))
            .add_modifier(Modifier::BOLD);
        let big = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::Rgb(130, 140, 165));
        let ok = Style::default().fg(Color::Rgb(100, 220, 160));
        let warn = Style::default().fg(Color::Rgb(255, 120, 120));

        let tab = |label: &str, on: bool| {
            Span::styled(
                format!(" {label} "),
                if on {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Rgb(255, 190, 90))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Rgb(160, 170, 190))
                        .bg(Color::Rgb(28, 34, 52))
                },
            )
        };

        let mut lines = vec![
            Line::from(vec![
                Span::raw("  "),
                tab("Clock", self.mode == Mode::Clock),
                Span::raw(" "),
                tab("Stopwatch", self.mode == Mode::Stopwatch),
                Span::raw(" "),
                tab("Timer", self.mode == Mode::Timer),
            ]),
            Line::default(),
        ];

        match self.mode {
            Mode::Clock => {
                let time = now.format("%H:%M:%S").to_string();
                let date = now.format("%A, %B %d %Y").to_string();
                lines.push(Line::from(Span::styled(format!("     {time}"), big)));
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(format!("     {date}"), accent)));
                lines.push(Line::from(Span::styled(
                    format!("     day {} of {}", now.ordinal(), now.year()),
                    dim,
                )));
            }
            Mode::Stopwatch => {
                let ms = self.stopwatch_ms();
                lines.push(Line::from(Span::styled(
                    format!("     {}", Self::fmt_ms(ms)),
                    big,
                )));
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(
                    if self.sw_running {
                        "     ● running"
                    } else if ms > 0 {
                        "     ■ paused"
                    } else {
                        "     ○ ready"
                    },
                    if self.sw_running { ok } else { dim },
                )));
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(
                    "     Space start/stop · R reset",
                    dim,
                )));
            }
            Mode::Timer => {
                let left = self.timer_remaining_ms();
                if self.timer_running && left == 0 {
                    self.timer_running = false;
                    self.timer_done = true;
                    self.timer_left_ms = 0;
                    self.timer_started = None;
                }
                lines.push(Line::from(Span::styled(
                    format!("     {}", Self::fmt_ms(left)),
                    if self.timer_done { warn } else { big },
                )));
                lines.push(Line::default());
                if self.timer_done {
                    lines.push(Line::from(Span::styled("     ✦ time's up!", warn)));
                } else {
                    lines.push(Line::from(Span::styled(
                        format!(
                            "     {} min preset · +/- 30s · [/] ±1 min",
                            self.timer_secs / 60
                        ),
                        dim,
                    )));
                }
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(
                    "     Space start/stop · R reset",
                    dim,
                )));
            }
        }

        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  Tab cycle modes · 1/2/3 jump",
            dim,
        )));

        f.render_widget(Paragraph::new(lines), area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Tab => {
                self.mode = match self.mode {
                    Mode::Clock => Mode::Stopwatch,
                    Mode::Stopwatch => Mode::Timer,
                    Mode::Timer => Mode::Clock,
                };
            }
            KeyCode::Char('1') => self.mode = Mode::Clock,
            KeyCode::Char('2') => self.mode = Mode::Stopwatch,
            KeyCode::Char('3') => self.mode = Mode::Timer,
            KeyCode::Char(' ') => match self.mode {
                Mode::Stopwatch => {
                    if self.sw_running {
                        if let Some(t) = self.sw_started.take() {
                            self.sw_elapsed_ms += t.elapsed().as_millis();
                        }
                        self.sw_running = false;
                    } else {
                        self.sw_started = Some(Instant::now());
                        self.sw_running = true;
                    }
                }
                Mode::Timer => {
                    if self.timer_done {
                        self.timer_done = false;
                        self.timer_left_ms = self.timer_secs as u128 * 1000;
                    }
                    if self.timer_running {
                        self.timer_left_ms = self.timer_remaining_ms();
                        self.timer_running = false;
                        self.timer_started = None;
                    } else if self.timer_left_ms > 0 {
                        self.timer_started = Some(Instant::now());
                        self.timer_running = true;
                        self.timer_done = false;
                    }
                }
                Mode::Clock => {}
            },
            KeyCode::Char('r') | KeyCode::Char('R') => match self.mode {
                Mode::Stopwatch => {
                    self.sw_running = false;
                    self.sw_started = None;
                    self.sw_elapsed_ms = 0;
                }
                Mode::Timer => {
                    self.timer_running = false;
                    self.timer_started = None;
                    self.timer_done = false;
                    self.timer_left_ms = self.timer_secs as u128 * 1000;
                }
                Mode::Clock => {}
            },
            KeyCode::Char('+') | KeyCode::Char('=')
                if self.mode == Mode::Timer && !self.timer_running =>
            {
                self.timer_secs = (self.timer_secs + 30).min(99 * 3600);
                self.timer_left_ms = self.timer_secs as u128 * 1000;
                self.timer_done = false;
            }
            KeyCode::Char('-') if self.mode == Mode::Timer && !self.timer_running => {
                self.timer_secs = self.timer_secs.saturating_sub(30).max(30);
                self.timer_left_ms = self.timer_secs as u128 * 1000;
                self.timer_done = false;
            }
            KeyCode::Char(']') if self.mode == Mode::Timer && !self.timer_running => {
                self.timer_secs = (self.timer_secs + 60).min(99 * 3600);
                self.timer_left_ms = self.timer_secs as u128 * 1000;
            }
            KeyCode::Char('[') if self.mode == Mode::Timer && !self.timer_running => {
                self.timer_secs = self.timer_secs.saturating_sub(60).max(30);
                self.timer_left_ms = self.timer_secs as u128 * 1000;
            }
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_tick(&mut self) {
        if self.mode == Mode::Timer && self.timer_running && self.timer_remaining_ms() == 0 {
            self.timer_running = false;
            self.timer_done = true;
            self.timer_left_ms = 0;
            self.timer_started = None;
        }
    }
}
