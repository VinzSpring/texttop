use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph};
use ratatui::Frame;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use super::ui::{Controls, Look};
use super::{Action, AppWindow};
use crate::gfx;

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    PlayPause,
    Back,
    Forward,
    Replay,
    Quieter,
    Louder,
    /// The progress / volume gauges: the click column decides the value.
    Seek,
    Volume,
}

pub struct AudioPlayer {
    path: PathBuf,
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
    duration: Option<Duration>,
    error: Option<String>,
    t: u64,
    ui: Controls<Cmd>,
    seek_bar: Rect,
    vol_bar: Rect,
    /// Which gauge a held button is dragging.
    dragging: Option<Cmd>,
}

impl AudioPlayer {
    pub fn new(path: PathBuf) -> Self {
        let mut p = AudioPlayer {
            path,
            _stream: None,
            handle: None,
            sink: None,
            duration: None,
            error: None,
            t: 0,
            ui: Controls::new(),
            seek_bar: Rect::default(),
            vol_bar: Rect::default(),
            dragging: None,
        };
        p.start();
        p
    }

    fn start(&mut self) {
        self.error = None;
        let (stream, handle) = match OutputStream::try_default() {
            Ok(x) => x,
            Err(e) => {
                self.error = Some(format!("no audio device: {e}"));
                return;
            }
        };
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) => {
                self.error = Some(format!("cannot open file: {e}"));
                return;
            }
        };
        let source = match Decoder::new(BufReader::new(file)) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(format!("cannot decode: {e}"));
                return;
            }
        };
        self.duration = source.total_duration();
        let sink = match Sink::try_new(&handle) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(format!("audio error: {e}"));
                return;
            }
        };
        sink.append(source);
        self._stream = Some(stream);
        self.handle = Some(handle);
        self.sink = Some(sink);
    }

    fn run(&mut self, c: Cmd) {
        if c == Cmd::Replay || (c == Cmd::PlayPause && self.sink.as_ref().is_some_and(|s| s.empty())) {
            if let Some(s) = &self.sink {
                s.stop();
            }
            self.start();
            return;
        }
        let Some(s) = &self.sink else { return };
        match c {
            Cmd::PlayPause => {
                if s.is_paused() {
                    s.play();
                } else {
                    s.pause();
                }
            }
            Cmd::Back => {
                let _ = s.try_seek(s.get_pos().saturating_sub(Duration::from_secs(5)));
            }
            Cmd::Forward => {
                let _ = s.try_seek(s.get_pos() + Duration::from_secs(5));
            }
            Cmd::Quieter => s.set_volume((s.volume() - 0.1).max(0.0)),
            Cmd::Louder => s.set_volume((s.volume() + 0.1).min(1.5)),
            Cmd::Replay | Cmd::Seek | Cmd::Volume => {}
        }
    }

    /// Set position or volume from a click / drag column on a gauge.
    fn drag_to(&mut self, which: Cmd, col: u16) {
        let bar = if which == Cmd::Seek { self.seek_bar } else { self.vol_bar };
        if bar.width == 0 {
            return;
        }
        let frac = (col.saturating_sub(bar.x) as f64 / bar.width.max(1) as f64).clamp(0.0, 1.0);
        let Some(s) = &self.sink else { return };
        match which {
            Cmd::Seek => {
                if let Some(d) = self.duration {
                    let _ = s.try_seek(d.mul_f64(frac));
                }
            }
            _ => s.set_volume((frac * 1.5) as f32),
        }
    }

    fn playing(&self) -> bool {
        self.sink
            .as_ref()
            .map(|s| !s.is_paused() && !s.empty())
            .unwrap_or(false)
    }
}

impl AppWindow for AudioPlayer {
    fn title(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Audio".into())
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 5 {
            return;
        }
        self.ui.begin();
        if let Some(err) = &self.error {
            f.render_widget(
                Paragraph::new(vec![
                    Line::default(),
                    Line::from(Span::styled(
                        format!("  ✗ {err}"),
                        Style::default().fg(Color::Rgb(255, 110, 110)),
                    )),
                ]),
                area,
            );
            return;
        }
        let sink = self.sink.as_ref();
        let pos = sink.map(|s| s.get_pos()).unwrap_or_default();
        let vol = sink.map(|s| s.volume()).unwrap_or(1.0);
        let finished = sink.map(|s| s.empty()).unwrap_or(false);
        let paused = sink.map(|s| s.is_paused()).unwrap_or(false);

        // header
        let state = if finished {
            ("⏹ finished", Color::Rgb(160, 165, 185))
        } else if paused {
            ("⏸ paused", Color::Rgb(255, 200, 90))
        } else {
            ("▶ playing", Color::Rgb(110, 230, 140))
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("  ♪ ", Style::default().fg(Color::Rgb(190, 140, 255))),
                Span::styled(
                    self.title(),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::raw("   "),
                Span::styled(state.0, Style::default().fg(state.1)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );

        // visualizer
        let viz_h = area.height.saturating_sub(6);
        if viz_h > 0 {
            let viz = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), viz_h);
            let buf = f.buffer_mut();
            let bars = (viz.width / 3).max(1);
            for b in 0..bars {
                let ph = self.t as f32 * 0.21;
                let i = b as f32;
                let v = if self.playing() {
                    ((ph + i * 0.9).sin() * (ph * 0.63 + i * 0.37).cos()).abs()
                        * (0.55 + 0.45 * (ph * 0.11 + i * 1.7).sin().abs())
                } else {
                    0.04
                };
                let h = ((v * viz.height as f32).round() as u16).min(viz.height);
                for y in 0..h {
                    let frac = y as f32 / viz.height.max(1) as f32;
                    let color = Color::Rgb(
                        gfx::lerp(60, 255, frac),
                        gfx::lerp(220, 80, frac),
                        gfx::lerp(140, 200, frac),
                    );
                    for dx in 0..2 {
                        let cx = viz.x + b * 3 + dx;
                        let cy = viz.y + viz.height - 1 - y;
                        if cx < viz.x + viz.width {
                            if let Some(c) = buf.cell_mut((cx, cy)) {
                                c.set_symbol("█");
                                c.set_fg(color);
                            }
                        }
                    }
                }
            }
        }

        // progress
        let label = match self.duration {
            Some(d) => format!("{} / {}", gfx::fmt_dur(pos), gfx::fmt_dur(d)),
            None => format!("{} / --:--", gfx::fmt_dur(pos)),
        };
        let ratio = self
            .duration
            .map(|d| (pos.as_secs_f64() / d.as_secs_f64().max(0.001)).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        self.seek_bar = Rect::new(area.x + 1, area.y + area.height - 5, area.width.saturating_sub(2), 1);
        f.render_widget(
            Gauge::default()
                .ratio(ratio)
                .label(label)
                .gauge_style(Style::default().fg(Color::Rgb(0, 170, 230)).bg(Color::Rgb(30, 34, 54))),
            self.seek_bar,
        );
        if self.duration.is_some() {
            self.ui.add(self.seek_bar, Cmd::Seek);
        }

        // volume: − [gauge] +
        let vy = area.y + area.height - 4;
        let buf = f.buffer_mut();
        let minus = self.ui.button(buf, area.x + 1, vy, "🔉", Cmd::Quieter, Look::Raised);
        let plus_w = 4;
        self.vol_bar = Rect::new(
            area.x + 2 + minus,
            vy,
            area.width.saturating_sub(4 + minus + plus_w),
            1,
        );
        self.ui.button(buf, self.vol_bar.x + self.vol_bar.width + 1, vy, "🔊", Cmd::Louder, Look::Raised);
        f.render_widget(
            Gauge::default()
                .ratio((vol as f64 / 1.5).clamp(0.0, 1.0))
                .label(format!("volume {:>3}%", (vol * 100.0) as u32))
                .gauge_style(Style::default().fg(Color::Rgb(190, 140, 255)).bg(Color::Rgb(30, 34, 54))),
            self.vol_bar,
        );
        self.ui.add(self.vol_bar, Cmd::Volume);

        // transport
        let play = if finished {
            ("▶ Play again", Look::Primary)
        } else if paused {
            ("▶ Play", Look::Primary)
        } else {
            ("❚❚ Pause", Look::Raised)
        };
        let w = area.width.saturating_sub(2).min(64);
        let x = area.x + area.width.saturating_sub(w) / 2;
        self.ui.button_row(
            f.buffer_mut(),
            Rect::new(x, area.y + area.height - 3, w, 3),
            &[
                ("⏪ 5s", Cmd::Back, Look::Raised),
                (play.0, Cmd::PlayPause, play.1),
                ("5s ⏩", Cmd::Forward, Look::Raised),
                ("↺ Restart", Cmd::Replay, Look::Raised),
            ],
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Char(' ') => self.run(Cmd::PlayPause),
            KeyCode::Up | KeyCode::Char('+') => self.run(Cmd::Louder),
            KeyCode::Down | KeyCode::Char('-') => self.run(Cmd::Quieter),
            KeyCode::Right => self.run(Cmd::Forward),
            KeyCode::Left => self.run(Cmd::Back),
            KeyCode::Char('r') => self.run(Cmd::Replay),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        match m.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(which) = self.dragging {
                    self.drag_to(which, m.column);
                }
                return Action::None;
            }
            MouseEventKind::Up(_) => self.dragging = None,
            MouseEventKind::ScrollUp => self.run(Cmd::Louder),
            MouseEventKind::ScrollDown => self.run(Cmd::Quieter),
            _ => {}
        }
        if let Some(Some(c)) = self.ui.route(&m) {
            if matches!(c, Cmd::Seek | Cmd::Volume) {
                self.dragging = Some(c);
                self.drag_to(c, m.column);
            } else {
                self.run(c);
            }
        }
        Action::None
    }

    fn on_tick(&mut self) {
        self.t += 1;
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    fn on_drop(&mut self, paths: &[PathBuf], _cut: bool) -> Action {
        if let Some(p) = paths.first().filter(|p| p.is_file()) {
            return Action::Launch(super::Launch::Audio(p.clone()));
        }
        Action::None
    }
}
