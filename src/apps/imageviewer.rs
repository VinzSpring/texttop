use std::path::PathBuf;

use image::{DynamicImage, ImageDecoder, ImageReader, RgbImage};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};
use crate::gfx;

/// Same cell pixel geometry as web/video — keeps image aspect correct on
/// typical ~1∶2 terminal glyphs.
const CELL_PX_W: u32 = 8;
const CELL_PX_H: u32 = 16;

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
        self.cache.clear();
        match load_oriented(&self.path) {
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

/// Decode with EXIF orientation applied (phone JPGs etc.).
fn load_oriented(path: &std::path::Path) -> Result<DynamicImage, image::ImageError> {
    let mut decoder = ImageReader::open(path)?.with_guessed_format()?.into_decoder()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Fit image into the view; returns cell size and a canvas whose pixel aspect
/// matches the cell grid (cell_w×8 by cell_h×16) with the image letterboxed.
fn fit_letterbox(
    img: &RgbImage,
    view_w: u16,
    view_h: u16,
) -> (u16, u16, RgbImage) {
    let (iw, ih) = img.dimensions();
    let max_w = view_w as u32 * CELL_PX_W;
    let max_h = view_h as u32 * CELL_PX_H;
    let scale = (max_w as f64 / iw as f64).min(max_h as f64 / ih as f64);
    let dw = ((iw as f64 * scale).round() as u32).max(1);
    let dh = ((ih as f64 * scale).round() as u32).max(1);

    let cell_w = dw.div_ceil(CELL_PX_W).min(view_w as u32) as u16;
    let cell_h = dh.div_ceil(CELL_PX_H).min(view_h as u32) as u16;
    let canvas_w = cell_w as u32 * CELL_PX_W;
    let canvas_h = cell_h as u32 * CELL_PX_H;

    let resized = image::imageops::resize(img, dw, dh, image::imageops::FilterType::Triangle);
    let mut canvas = RgbImage::from_pixel(canvas_w, canvas_h, image::Rgb([5, 5, 8]));
    let ox = ((canvas_w - dw) / 2) as i64;
    let oy = ((canvas_h - dh) / 2) as i64;
    image::imageops::replace(&mut canvas, &resized, ox, oy);
    (cell_w.max(1), cell_h.max(1), canvas)
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
        if view.width == 0 || view.height == 0 {
            return;
        }

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

        let (iw, ih) = img.dimensions();

        // Peek target cell size (same math as fit) to know if cache is stale.
        let max_w = view.width as u32 * CELL_PX_W;
        let max_h = view.height as u32 * CELL_PX_H;
        let scale = (max_w as f64 / iw as f64).min(max_h as f64 / ih as f64);
        let dw = ((iw as f64 * scale).round() as u32).max(1);
        let dh = ((ih as f64 * scale).round() as u32).max(1);
        let want_w = dw.div_ceil(CELL_PX_W).min(view.width as u32) as u16;
        let want_h = dh.div_ceil(CELL_PX_H).min(view.height as u32) as u16;

        if self
            .scaled
            .as_ref()
            .map(|(w, h, _)| (*w, *h) != (want_w, want_h))
            .unwrap_or(true)
        {
            let (cell_w, cell_h, canvas) = fit_letterbox(img, view.width, view.height);
            self.scaled = Some((cell_w, cell_h, canvas));
            self.cache.clear();
        }

        if let Some((cell_w, cell_h, scaled)) = &self.scaled {
            let cell_w = *cell_w;
            let cell_h = *cell_h;
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
                Style::default()
                    .fg(Color::Rgb(140, 150, 175))
                    .bg(Color::Rgb(18, 22, 36)),
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

    fn accepts_drop(&self) -> bool {
        true
    }

    fn on_drop(&mut self, paths: &[PathBuf], _cut: bool) -> Action {
        if let Some(p) = paths.first().filter(|p| p.is_file()) {
            self.path = p.clone();
            self.load();
        }
        Action::None
    }
}
