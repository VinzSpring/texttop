//! Video player backed by the embedded WebView2 engine — decodes any format
//! Edge can play (mp4/h264, webm, mkv, …) with real audio output, no
//! external programs required.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};
use crate::engine::{Cmd, Engine, MouseKind, Status};
use crate::gfx;

const CELL_PX_W: i32 = 8;
const CELL_PX_H: i32 = 16;
const V: &str = "var v=document.querySelector('video');if(v)";

pub struct VideoPlayer {
    engine: Option<Engine>,
    path: PathBuf,
    error: Option<String>,
    last_px: (i32, i32),
    t: u64,
    cache: gfx::RgbCache,
    last_frame_id: u64,
    fps_count: u32,
    fps: u32,
    fps_window: std::time::Instant,
}

impl VideoPlayer {
    pub fn new(path: PathBuf) -> Self {
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        let (engine, error) = match url::Url::from_file_path(&abs) {
            Ok(u) => (
                Some(Engine::start(Cmd::Navigate(u.to_string()), 1.0, 800, 450, false)),
                None,
            ),
            Err(_) => (None, Some("invalid file path".to_string())),
        };
        VideoPlayer {
            engine,
            path,
            error,
            last_px: (800, 450),
            t: 0,
            cache: gfx::RgbCache::default(),
            last_frame_id: 0,
            fps_count: 0,
            fps: 0,
            fps_window: std::time::Instant::now(),
        }
    }

    fn script(&self, s: &str) {
        if let Some(e) = &self.engine {
            e.send(Cmd::Script(s.to_string()));
        }
    }
}

impl AppWindow for VideoPlayer {
    fn title(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Video".into())
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 2 {
            return;
        }
        let view = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));

        let want = (
            view.width as i32 * CELL_PX_W,
            view.height as i32 * CELL_PX_H,
        );
        if want != self.last_px && want.0 > 0 && want.1 > 0 {
            self.last_px = want;
            if let Some(e) = &self.engine {
                e.send(Cmd::ResizePx(want.0, want.1));
            }
        }

        let mut err = self.error.clone();
        if let Some(e) = &self.engine {
            if let Status::Error(msg) = e.status() {
                err = Some(msg);
            }
        }
        if let Some(err) = err {
            let mut lines = vec![Line::default()];
            for chunk in err.split(". ") {
                lines.push(Line::from(Span::styled(
                    format!("  ✗ {chunk}"),
                    Style::default().fg(Color::Rgb(255, 110, 110)),
                )));
            }
            f.render_widget(Paragraph::new(lines), view);
        } else if let Some(frame) = self.engine.as_ref().and_then(|e| e.frame()) {
            gfx::draw_rgb_cached(
                f.buffer_mut(),
                view,
                &frame.rgb,
                frame.w,
                frame.h,
                frame.seq as usize,
                &mut self.cache,
            );
        } else {
            let msg = "⏳ opening video …";
            let y = view.y + view.height / 2;
            let x = view.x + view.width.saturating_sub(msg.chars().count() as u16) / 2;
            f.render_widget(
                Paragraph::new(Span::styled(
                    msg,
                    Style::default()
                        .fg(Color::Rgb(0, 190, 255))
                        .add_modifier(Modifier::BOLD),
                )),
                Rect::new(x, y, view.width.saturating_sub(x - view.x), 1),
            );
        }

        let status = format!(
            " 🎬 {} fps · Space pause · ←→ seek 5s · ↑↓ volume · m mute · click controls · q close ",
            self.fps
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                status,
                Style::default().fg(Color::Rgb(140, 150, 175)).bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Char(' ') | KeyCode::Char('k') => {
                self.script(&format!("{V}{{if(v.paused)v.play();else v.pause();}}"))
            }
            KeyCode::Right => self.script(&format!("{V}v.currentTime+=5;")),
            KeyCode::Left => self.script(&format!("{V}v.currentTime=Math.max(0,v.currentTime-5);")),
            KeyCode::Up => self.script(&format!("{V}v.volume=Math.min(1,v.volume+0.1);")),
            KeyCode::Down => self.script(&format!("{V}v.volume=Math.max(0,v.volume-0.1);")),
            KeyCode::Char('m') => self.script(&format!("{V}v.muted=!v.muted;")),
            KeyCode::Char('r') => self.script(&format!("{V}{{v.currentTime=0;v.play();}}")),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        let view = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        let inside = m.column >= view.x
            && m.column < view.x + view.width
            && m.row >= view.y
            && m.row < view.y + view.height;
        let kind = match m.kind {
            MouseEventKind::Down(MouseButton::Left) if inside => Some(MouseKind::Down),
            MouseEventKind::Up(MouseButton::Left) => Some(MouseKind::Up),
            MouseEventKind::Drag(MouseButton::Left) => Some(MouseKind::Drag),
            MouseEventKind::Moved if inside => Some(MouseKind::Move),
            _ => None,
        };
        if let (Some(kind), Some(engine)) = (kind, self.engine.as_ref()) {
            let col = m
                .column
                .max(view.x)
                .min(view.x + view.width.saturating_sub(1));
            let row = m
                .row
                .max(view.y)
                .min(view.y + view.height.saturating_sub(1));
            let x = (col.saturating_sub(view.x)) as i32 * CELL_PX_W + CELL_PX_W / 2;
            let y = (row.saturating_sub(view.y)) as i32 * CELL_PX_H + CELL_PX_H / 2;
            engine.send(Cmd::Mouse { kind, x, y });
        }
        Action::None
    }

    fn on_tick(&mut self) {
        self.t += 1;
        if let Some(frame) = self.engine.as_ref().and_then(|e| e.frame()) {
            if frame.seq != self.last_frame_id {
                self.last_frame_id = frame.seq;
                self.fps_count += 1;
            }
        }
        if self.fps_window.elapsed().as_millis() >= 1000 {
            self.fps = self.fps_count;
            self.fps_count = 0;
            self.fps_window = std::time::Instant::now();
        }
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    fn on_drop(&mut self, paths: &[PathBuf], _cut: bool) -> Action {
        if let Some(p) = paths.first().filter(|p| p.is_file()) {
            return Action::Launch(super::Launch::Video(p.clone()));
        }
        Action::None
    }
}
