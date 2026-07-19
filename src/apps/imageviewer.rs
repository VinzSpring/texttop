use std::path::PathBuf;

use image::RgbImage;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};
use crate::gfx;

pub struct ImageViewer {
    path: PathBuf,
    img: Option<RgbImage>,
    scaled: Option<(u16, u16, RgbImage)>, // keyed by target cell area
    error: Option<String>,
    cache: gfx::RgbCache,
}

impl ImageViewer {
    pub fn new(path: PathBuf) -> Self {
        let mut v = ImageViewer {
            path,
            img: None,
            scaled: None,
            error: None,
            cache: gfx::RgbCache::default(),
        };
        v.load();
        v
    }

    fn load(&mut self) {
        self.scaled = None;
        match image::open(&self.path) {
            Ok(img) => {
                // cap huge images before storing
                let img = if img.width() > 2400 || img.height() > 2400 {
                    img.thumbnail(2400, 2400)
                } else {
                    img
                };
                self.img = Some(img.to_rgb8());
                self.error = None;
            }
            Err(e) => self.error = Some(format!("cannot load image: {e}")),
        }
    }
}

impl AppWindow for ImageViewer {
    fn title(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Image".into())
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 2 {
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
        let Some(img) = &self.img else { return };
        let view = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));

        // fill background
        {
            let buf = f.buffer_mut();
            for y in view.y..view.y + view.height {
                for x in view.x..view.x + view.width {
                    if let Some(c) = buf.cell_mut((x, y)) {
                        c.set_symbol(" ");
                        c.set_bg(Color::Rgb(5, 5, 8));
                    }
                }
            }
        }

        // fit image (1 cell = 2×2 quadrant pixels)
        let (iw, ih) = img.dimensions();
        let avail_px_w = view.width as u32 * 2;
        let avail_px_h = view.height as u32 * 2;
        if avail_px_w == 0 || avail_px_h == 0 {
            return;
        }
        let scale = (avail_px_w as f64 / iw as f64).min(avail_px_h as f64 / ih as f64);
        let dw = ((iw as f64 * scale) as u32).max(1);
        let dh = ((ih as f64 * scale) as u32).max(1);
        let cell_w = (dw.div_ceil(2) as u16).min(view.width);
        let cell_h = (dh.div_ceil(2) as u16).min(view.height);

        if self
            .scaled
            .as_ref()
            .map(|(w, h, _)| (*w, *h) != (cell_w, cell_h))
            .unwrap_or(true)
        {
            let resized =
                image::imageops::resize(img, dw, dh, image::imageops::FilterType::Triangle);
            self.scaled = Some((cell_w, cell_h, resized));
        }

        if let Some((_, _, scaled)) = &self.scaled {
            let ox = view.x + (view.width - cell_w) / 2;
            let oy = view.y + (view.height - cell_h) / 2;
            let target = Rect::new(ox, oy, cell_w, cell_h);
            gfx::draw_rgb_cached(
                f.buffer_mut(),
                target,
                scaled.as_raw(),
                scaled.width(),
                scaled.height(),
                scaled.as_raw().as_ptr() as usize,
                &mut self.cache,
            );
        }

        let info = format!(" {}×{} px · r reload · q close ", iw, ih);
        f.render_widget(
            Paragraph::new(Span::styled(
                info,
                Style::default().fg(Color::Rgb(140, 150, 175)).bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Char('r') => self.load(),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }
}
