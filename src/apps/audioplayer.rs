use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph};
use ratatui::Frame;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use super::{Action, AppWindow};
use crate::gfx;

pub struct AudioPlayer {
    path: PathBuf,
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
    duration: Option<Duration>,
    error: Option<String>,
    t: u64,
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
        if area.height < 4 {
            return;
        }
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
            ("⏹ finished — press r to replay", Color::Rgb(160, 165, 185))
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
        let viz_h = area.height.saturating_sub(5);
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
        f.render_widget(
            Gauge::default()
                .ratio(ratio)
                .label(label)
                .gauge_style(Style::default().fg(Color::Rgb(0, 170, 230)).bg(Color::Rgb(30, 34, 54))),
            Rect::new(area.x + 1, area.y + area.height - 4, area.width.saturating_sub(2), 1),
        );

        // volume
        f.render_widget(
            Gauge::default()
                .ratio((vol as f64 / 1.5).clamp(0.0, 1.0))
                .label(format!("vol {:>3}%", (vol * 100.0) as u32))
                .gauge_style(Style::default().fg(Color::Rgb(190, 140, 255)).bg(Color::Rgb(30, 34, 54))),
            Rect::new(area.x + 1, area.y + area.height - 3, area.width.saturating_sub(2), 1),
        );

        f.render_widget(
            Paragraph::new(Span::styled(
                " Space pause · ←→ seek 5s · ↑↓ volume · r replay · q close ",
                Style::default().fg(Color::Rgb(140, 150, 175)),
            )),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Char(' ') => {
                if let Some(s) = &self.sink {
                    if s.is_paused() {
                        s.play();
                    } else {
                        s.pause();
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('+') => {
                if let Some(s) = &self.sink {
                    s.set_volume((s.volume() + 0.1).min(1.5));
                }
            }
            KeyCode::Down | KeyCode::Char('-') => {
                if let Some(s) = &self.sink {
                    s.set_volume((s.volume() - 0.1).max(0.0));
                }
            }
            KeyCode::Right => {
                if let Some(s) = &self.sink {
                    let _ = s.try_seek(s.get_pos() + Duration::from_secs(5));
                }
            }
            KeyCode::Left => {
                if let Some(s) = &self.sink {
                    let target = s.get_pos().saturating_sub(Duration::from_secs(5));
                    let _ = s.try_seek(target);
                }
            }
            KeyCode::Char('r') => {
                if let Some(s) = &self.sink {
                    s.stop();
                }
                self.start();
            }
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_tick(&mut self) {
        self.t += 1;
    }
}
