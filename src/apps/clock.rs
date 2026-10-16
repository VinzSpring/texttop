use std::time::Instant;

use chrono::{Datelike, Local};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;

use super::ui::{Controls, Item, Look};
use super::{Action, AppWindow};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Clock,
    Stopwatch,
    Timer,
}

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    Mode(Mode),
    StartStop,
    Reset,
    /// Adjust the timer preset by this many seconds.
    Adjust(i64),
    /// Set the timer preset to this many minutes.
    Preset(u64),
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
    ui: Controls<Cmd>,
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
            ui: Controls::new(),
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

    fn set_preset(&mut self, secs: u64) {
        self.timer_secs = secs.clamp(30, 99 * 3600);
        self.timer_left_ms = self.timer_secs as u128 * 1000;
        self.timer_done = false;
    }

    fn running(&self) -> bool {
        match self.mode {
            Mode::Stopwatch => self.sw_running,
            Mode::Timer => self.timer_running,
            Mode::Clock => false,
        }
    }

    fn run(&mut self, c: Cmd) {
        match c {
            Cmd::Mode(m) => self.mode = m,
            Cmd::StartStop => match self.mode {
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
            Cmd::Reset => match self.mode {
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
            Cmd::Adjust(d) if !self.timer_running => {
                self.set_preset((self.timer_secs as i64 + d).max(0) as u64)
            }
            Cmd::Preset(min) if !self.timer_running => self.set_preset(min * 60),
            Cmd::Adjust(_) | Cmd::Preset(_) => {}
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
        if area.height < 2 {
            return;
        }
        self.on_tick();
        self.ui.begin();
        let now = Local::now();
        let accent = Style::default()
            .fg(Color::Rgb(255, 190, 90))
            .add_modifier(Modifier::BOLD);
        let big = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::Rgb(130, 140, 165));
        let ok = Style::default().fg(Color::Rgb(100, 220, 160));
        let warn = Style::default()
            .fg(Color::Rgb(255, 120, 120))
            .add_modifier(Modifier::BOLD);

        let buf = f.buffer_mut();
        let tab = |label: &str, m: Mode, cur: Mode| Item::with(label, Cmd::Mode(m), Look::on_if(m == cur));
        self.ui.toolbar(
            buf,
            Rect::new(area.x, area.y, area.width, 1),
            &[
                tab("🕐 Clock", Mode::Clock, self.mode),
                tab("⏱ Stopwatch", Mode::Stopwatch, self.mode),
                tab("⏲ Timer", Mode::Timer, self.mode),
            ],
            &[],
        );

        let bottom = area.y + area.height;
        let center = |buf: &mut ratatui::buffer::Buffer, y: u16, s: &str, st: Style| {
            if y < bottom {
                let w = unicode_width::UnicodeWidthStr::width(s) as u16;
                buf.set_string(area.x + area.width.saturating_sub(w) / 2, y, s, st);
            }
        };
        let y0 = area.y + 2;
        match self.mode {
            Mode::Clock => {
                center(buf, y0, &now.format("%H:%M:%S").to_string(), big);
                center(buf, y0 + 2, &now.format("%A, %B %d %Y").to_string(), accent);
                center(
                    buf,
                    y0 + 3,
                    &format!("day {} of {}", now.ordinal(), now.year()),
                    dim,
                );
                return;
            }
            Mode::Stopwatch => {
                let ms = self.stopwatch_ms();
                center(buf, y0, &Self::fmt_ms(ms), big);
                let (label, st) = if self.sw_running {
                    ("● running", ok)
                } else if ms > 0 {
                    ("■ paused", dim)
                } else {
                    ("○ ready", dim)
                };
                center(buf, y0 + 1, label, st);
            }
            Mode::Timer => {
                let left = self.timer_remaining_ms();
                center(buf, y0, &Self::fmt_ms(left), if self.timer_done { warn } else { big });
                if self.timer_done {
                    center(buf, y0 + 1, "✦ time's up!", warn);
                } else {
                    center(buf, y0 + 1, &format!("preset {}", fmt_preset(self.timer_secs)), dim);
                }
            }
        }

        // Start / Reset as large buttons under the readout.
        let w = area.width.saturating_sub(4).min(40);
        let x = area.x + area.width.saturating_sub(w) / 2;
        let row = y0 + 3;
        if row + 3 <= bottom {
            let start = if self.running() {
                ("❚❚ Pause", Look::Raised)
            } else {
                ("▶ Start", Look::Primary)
            };
            self.ui.button_row(
                buf,
                Rect::new(x, row, w, 3),
                &[(start.0, Cmd::StartStop, start.1), ("↺ Reset", Cmd::Reset, Look::Raised)],
            );
        }
        if self.mode == Mode::Timer && row + 5 <= bottom {
            let idle = if self.timer_running { Look::Disabled } else { Look::Raised };
            self.ui.button_row(
                buf,
                Rect::new(x, row + 4, w, 1),
                &[
                    ("−1m", Cmd::Adjust(-60), idle),
                    ("−30s", Cmd::Adjust(-30), idle),
                    ("+30s", Cmd::Adjust(30), idle),
                    ("+1m", Cmd::Adjust(60), idle),
                ],
            );
            if row + 7 <= bottom {
                let preset = |m: u64| {
                    if self.timer_running {
                        Look::Disabled
                    } else {
                        Look::on_if(self.timer_secs == m * 60)
                    }
                };
                self.ui.button_row(
                    buf,
                    Rect::new(x, row + 6, w, 1),
                    &[
                        ("1 min", Cmd::Preset(1), preset(1)),
                        ("5 min", Cmd::Preset(5), preset(5)),
                        ("10 min", Cmd::Preset(10), preset(10)),
                        ("25 min", Cmd::Preset(25), preset(25)),
                    ],
                );
            }
        }
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
            KeyCode::Char(' ') => self.run(Cmd::StartStop),
            KeyCode::Char('r') | KeyCode::Char('R') => self.run(Cmd::Reset),
            KeyCode::Char('+') | KeyCode::Char('=') if self.mode == Mode::Timer => {
                self.run(Cmd::Adjust(30))
            }
            KeyCode::Char('-') if self.mode == Mode::Timer => self.run(Cmd::Adjust(-30)),
            KeyCode::Char(']') if self.mode == Mode::Timer => self.run(Cmd::Adjust(60)),
            KeyCode::Char('[') if self.mode == Mode::Timer => self.run(Cmd::Adjust(-60)),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        if let Some(Some(c)) = self.ui.route(&m) {
            self.run(c);
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

fn fmt_preset(secs: u64) -> String {
    match (secs / 60, secs % 60) {
        (m, 0) => format!("{m} min"),
        (0, s) => format!("{s} s"),
        (m, s) => format!("{m} min {s} s"),
    }
}
