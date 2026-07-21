use std::path::{Path, PathBuf};
use std::time::Instant;

use image::{DynamicImage, ImageDecoder, ImageReader, RgbImage, RgbaImage};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind,
};
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

const BG: image::Rgb<u8> = image::Rgb([5, 5, 8]);
const IMG_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "ico",
];
/// Slideshow dwell: ticks arrive every 25 ms → 160 ≈ 4 s per image.
const SLIDESHOW_TICKS: u32 = 160;
const MIN_SCALE: f64 = 0.02;
const MAX_SCALE: f64 = 40.0;

#[derive(Clone, Copy, PartialEq)]
enum Zoom {
    /// Letterbox the whole image into the window (follows resizes).
    Fit,
    /// Explicit scale in image pixels per terminal pixel.
    Scale(f64),
}

pub struct ImageViewer {
    path: PathBuf,
    img: Option<RgbaImage>,
    has_alpha: bool,
    file_size: u64,
    error: Option<String>,

    gallery: Vec<PathBuf>,
    index: usize,

    zoom: Zoom,
    /// Pan center in image coordinates.
    cx: f64,
    cy: f64,

    // Composited full-view canvas, regenerated only when parameters change.
    canvas: Option<RgbImage>,
    canvas_params: Option<(usize, u64, u64, u64, u16, u16)>,
    canvas_seq: usize,
    img_seq: usize,
    cache: gfx::RgbCache,

    // Geometry of the last render, needed for mouse math.
    view: Rect,
    scale: f64,

    show_help: bool,
    show_info: bool,
    slideshow: bool,
    ticks: u32,
    drag: Option<(u16, u16)>,
    last_click: Option<(Instant, u16, u16)>,
}

impl ImageViewer {
    pub fn new(path: PathBuf) -> Self {
        let mut v = ImageViewer {
            path,
            img: None,
            has_alpha: false,
            file_size: 0,
            error: None,
            gallery: Vec::new(),
            index: 0,
            zoom: Zoom::Fit,
            cx: 0.0,
            cy: 0.0,
            canvas: None,
            canvas_params: None,
            canvas_seq: 0,
            img_seq: 0,
            cache: gfx::RgbCache::default(),
            view: Rect::default(),
            scale: 1.0,
            show_help: false,
            show_info: false,
            slideshow: false,
            ticks: 0,
            drag: None,
            last_click: None,
        };
        v.scan_gallery();
        v.load();
        v
    }

    /// Collect sibling images so ←/→ can browse the folder.
    fn scan_gallery(&mut self) {
        self.gallery.clear();
        let Some(dir) = self.path.parent() else { return };
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if p.is_file() && IMG_EXTS.contains(&ext.as_str()) {
                    self.gallery.push(p);
                }
            }
        }
        self.gallery
            .sort_by_key(|p| p.file_name().map(|n| n.to_ascii_lowercase()));
        self.index = self
            .gallery
            .iter()
            .position(|p| p == &self.path)
            .unwrap_or(0);
    }

    fn load(&mut self) {
        self.canvas = None;
        self.canvas_params = None;
        self.cache.clear();
        self.img_seq += 1;
        self.file_size = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        match load_oriented(&self.path) {
            Ok(img) => {
                self.has_alpha = img.color().has_alpha();
                // cap huge images before storing
                let img = if img.width() > 2400 || img.height() > 2400 {
                    img.thumbnail(2400, 2400)
                } else {
                    img
                };
                let rgba = img.to_rgba8();
                self.cx = rgba.width() as f64 / 2.0;
                self.cy = rgba.height() as f64 / 2.0;
                self.img = Some(rgba);
                self.error = None;
            }
            Err(e) => {
                self.img = None;
                self.error = Some(format!("cannot load image: {e}"));
            }
        }
        self.zoom = Zoom::Fit;
        self.ticks = 0;
    }

    fn nav(&mut self, delta: isize) {
        if self.gallery.len() < 2 {
            return;
        }
        let n = self.gallery.len() as isize;
        self.index = ((self.index as isize + delta).rem_euclid(n)) as usize;
        self.path = self.gallery[self.index].clone();
        self.load();
    }

    fn nav_to(&mut self, index: usize) {
        if self.gallery.is_empty() || index >= self.gallery.len() {
            return;
        }
        self.index = index;
        self.path = self.gallery[self.index].clone();
        self.load();
    }

    fn effective_scale(&self, iw: u32, ih: u32, canvas_w: u32, canvas_h: u32) -> f64 {
        match self.zoom {
            Zoom::Scale(s) => s,
            Zoom::Fit => {
                (canvas_w as f64 / iw as f64).min(canvas_h as f64 / ih as f64)
            }
        }
    }

    /// Multiply zoom, keeping the image point under `anchor` (screen cell)
    /// stationary; without an anchor, zoom around the view center.
    fn zoom_by(&mut self, factor: f64, anchor: Option<(u16, u16)>) {
        let Some(img) = &self.img else { return };
        let (iw, ih) = img.dimensions();
        if self.view.width == 0 || self.view.height == 0 {
            return;
        }
        let cw = self.view.width as u32 * CELL_PX_W;
        let ch = self.view.height as u32 * CELL_PX_H;
        let old = self.scale.max(f64::MIN_POSITIVE);
        let new = (old * factor).clamp(MIN_SCALE, MAX_SCALE);
        if let Some((col, row)) = anchor {
            let px = (col.saturating_sub(self.view.x)) as f64 * CELL_PX_W as f64
                + CELL_PX_W as f64 / 2.0;
            let py = (row.saturating_sub(self.view.y)) as f64 * CELL_PX_H as f64
                + CELL_PX_H as f64 / 2.0;
            let dx = px - cw as f64 / 2.0;
            let dy = py - ch as f64 / 2.0;
            let ix = self.cx + dx / old;
            let iy = self.cy + dy / old;
            self.cx = ix - dx / new;
            self.cy = iy - dy / new;
        }
        self.cx = self.cx.clamp(0.0, iw as f64);
        self.cy = self.cy.clamp(0.0, ih as f64);
        self.zoom = Zoom::Scale(new);
    }

    fn pan(&mut self, dx_cells: f64, dy_cells: f64) {
        if matches!(self.zoom, Zoom::Fit) {
            return;
        }
        let s = self.scale.max(f64::MIN_POSITIVE);
        self.cx += dx_cells * CELL_PX_W as f64 / s;
        self.cy += dy_cells * CELL_PX_H as f64 / s;
        if let Some(img) = &self.img {
            self.cx = self.cx.clamp(0.0, img.width() as f64);
            self.cy = self.cy.clamp(0.0, img.height() as f64);
        }
    }

    fn transform(&mut self, f: impl Fn(&RgbaImage) -> RgbaImage) {
        if let Some(img) = self.img.take() {
            let out = f(&img);
            self.cx = out.width() as f64 / 2.0;
            self.cy = out.height() as f64 / 2.0;
            self.img = Some(out);
            self.img_seq += 1;
            self.canvas_params = None;
            self.zoom = Zoom::Fit;
        }
    }

    /// Rebuild the full-view canvas (BG + checkerboard + scaled crop).
    fn rebuild_canvas(&mut self, view_w: u16, view_h: u16) {
        let Some(img) = &self.img else { return };
        let (iw, ih) = img.dimensions();
        let cw = view_w as u32 * CELL_PX_W;
        let ch = view_h as u32 * CELL_PX_H;
        let scale = self
            .effective_scale(iw, ih, cw, ch)
            .clamp(MIN_SCALE, MAX_SCALE);
        self.scale = scale;

        // Per-axis: letterbox when the scaled image fits, else crop a window
        // around the pan center (clamped so we never show past the edges).
        let axis = |dim: u32, canvas: u32, center: f64| -> (u32, u32, f64, f64, f64) {
            let full = dim as f64 * scale;
            if full <= canvas as f64 + 0.5 {
                let dw = (full.round() as u32).clamp(1, canvas);
                ((canvas - dw) / 2, dw, 0.0, dim as f64, dim as f64 / 2.0)
            } else {
                let vis = canvas as f64 / scale;
                let x = (center - vis / 2.0).clamp(0.0, dim as f64 - vis);
                (0, canvas, x, vis, x + vis / 2.0)
            }
        };
        let (dx0, dw, sx, sw, ncx) = axis(iw, cw, self.cx);
        let (dy0, dh, sy, sh, ncy) = axis(ih, ch, self.cy);
        self.cx = ncx;
        self.cy = ncy;

        let x0 = sx.floor() as u32;
        let y0 = sy.floor() as u32;
        let x1 = ((sx + sw).ceil() as u32).min(iw).max(x0 + 1);
        let y1 = ((sy + sh).ceil() as u32).min(ih).max(y0 + 1);
        let crop = image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image();
        // Crisp pixels when zoomed past 1∶1, smooth averaging when shrinking.
        let filter = if scale >= 1.0 {
            image::imageops::FilterType::Nearest
        } else {
            image::imageops::FilterType::Triangle
        };
        let resized = image::imageops::resize(&crop, dw, dh, filter);

        let mut canvas = RgbImage::from_pixel(cw, ch, BG);
        for (px, py, p) in resized.enumerate_pixels() {
            let gx = dx0 + px;
            let gy = dy0 + py;
            let a = p[3] as u32;
            let under = if a == 255 {
                (0u8, 0u8, 0u8)
            } else if self.has_alpha {
                // screen-space checkerboard so transparency reads at any zoom
                if ((gx / 16) + (gy / 16)) % 2 == 0 {
                    (52, 52, 60)
                } else {
                    (34, 34, 40)
                }
            } else {
                (BG[0], BG[1], BG[2])
            };
            let blend = |fg: u8, bg: u8| ((fg as u32 * a + bg as u32 * (255 - a)) / 255) as u8;
            canvas.put_pixel(
                gx,
                gy,
                image::Rgb([
                    blend(p[0], under.0),
                    blend(p[1], under.1),
                    blend(p[2], under.2),
                ]),
            );
        }
        self.canvas = Some(canvas);
        self.canvas_seq += 1;
        self.cache.clear();
    }

    fn status_line(&self) -> String {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let pos = if self.gallery.len() > 1 {
            format!(" {}/{} ·", self.index + 1, self.gallery.len())
        } else {
            String::new()
        };
        let dims = self
            .img
            .as_ref()
            .map(|i| format!(" {}×{} ·", i.width(), i.height()))
            .unwrap_or_default();
        let zoom = match self.zoom {
            Zoom::Fit => " fit".to_string(),
            Zoom::Scale(_) => format!(" {:.0}%", self.scale * 100.0),
        };
        let show = if self.slideshow { " · ▶ auto" } else { "" };
        format!(" {name} ·{pos}{dims}{zoom}{show} · ? help ")
    }
}

/// Decode with EXIF orientation applied (phone JPGs etc.).
fn load_oriented(path: &Path) -> Result<DynamicImage, image::ImageError> {
    let mut decoder = ImageReader::open(path)?.with_guessed_format()?.into_decoder()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

fn draw_overlay(f: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>) {
    let w = (lines
        .iter()
        .map(|l| l.width())
        .max()
        .unwrap_or(0)
        .max(title.len())
        + 6)
    .min(area.width as usize) as u16;
    let h = (lines.len() as u16 + 4).min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);
    gfx::fill(f.buffer_mut(), rect, Color::Rgb(16, 19, 32));
    let mut text = vec![
        Line::from(Span::styled(
            format!("  {title}"),
            Style::default()
                .fg(Color::Rgb(150, 200, 255))
                .add_modifier(ratatui::style::Modifier::BOLD),
        )),
        Line::default(),
    ];
    text.extend(lines);
    f.render_widget(
        Paragraph::new(text),
        Rect::new(rect.x, rect.y + 1, rect.width, rect.height.saturating_sub(2)),
    );
}

fn kv(k: &str, v: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {k:<11}"),
            Style::default().fg(Color::Rgb(120, 130, 160)),
        ),
        Span::styled(v, Style::default().fg(Color::Rgb(210, 215, 235))),
    ])
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
        let view = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        self.view = view;
        if view.width == 0 || view.height == 0 {
            return;
        }

        if let Some(err) = &self.error {
            gfx::fill(f.buffer_mut(), view, Color::Rgb(5, 5, 8));
            f.render_widget(
                Paragraph::new(vec![
                    Line::default(),
                    Line::from(Span::styled(
                        format!("  ✗ {err}"),
                        Style::default().fg(Color::Rgb(255, 110, 110)),
                    )),
                    Line::from(Span::styled(
                        "    ←/→ next · previous image",
                        Style::default().fg(Color::Rgb(150, 155, 175)),
                    )),
                ]),
                view,
            );
        } else if self.img.is_some() {
            let params = (
                self.img_seq,
                match self.zoom {
                    Zoom::Fit => 0u64,
                    Zoom::Scale(s) => s.to_bits(),
                },
                self.cx.to_bits(),
                self.cy.to_bits(),
                view.width,
                view.height,
            );
            if self.canvas_params != Some(params) {
                self.rebuild_canvas(view.width, view.height);
                // re-key with the (possibly clamped) pan center
                self.canvas_params = Some((
                    params.0,
                    params.1,
                    self.cx.to_bits(),
                    self.cy.to_bits(),
                    params.4,
                    params.5,
                ));
            }
            if let Some(canvas) = &self.canvas {
                gfx::draw_rgb_cached(
                    f.buffer_mut(),
                    view,
                    canvas.as_raw(),
                    canvas.width(),
                    canvas.height(),
                    self.canvas_seq,
                    &mut self.cache,
                );
            }
        }

        if self.show_info {
            let ext = self
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("?")
                .to_ascii_uppercase();
            let mut lines = vec![
                kv("file", self.title()),
                kv("format", ext),
                kv("size", gfx::fmt_size(self.file_size)),
            ];
            if let Some(img) = &self.img {
                lines.push(kv("pixels", format!("{}×{}", img.width(), img.height())));
                lines.push(kv(
                    "alpha",
                    if self.has_alpha { "yes" } else { "no" }.into(),
                ));
            }
            lines.push(kv("zoom", format!("{:.0}%", self.scale * 100.0)));
            if self.gallery.len() > 1 {
                lines.push(kv(
                    "gallery",
                    format!("{} of {}", self.index + 1, self.gallery.len()),
                ));
            }
            draw_overlay(f, view, "Image info", lines);
        } else if self.show_help {
            let dim = Style::default().fg(Color::Rgb(120, 130, 160));
            let lit = Style::default().fg(Color::Rgb(210, 215, 235));
            let row = |k: &'static str, d: &'static str| {
                Line::from(vec![
                    Span::styled(format!("  {k:<14}"), lit),
                    Span::styled(d, dim),
                ])
            };
            draw_overlay(
                f,
                view,
                "Image viewer keys",
                vec![
                    row("scroll · + -", "zoom (wheel zooms at cursor)"),
                    row("drag · hjkl", "pan when zoomed"),
                    row("f or 0", "fit to window"),
                    row("1", "100% pixel view"),
                    row("← → · n p", "previous / next image"),
                    row("Home / End", "first / last image"),
                    row("[ ]", "rotate left / right"),
                    row("x / y", "flip horizontal / vertical"),
                    row("s", "slideshow"),
                    row("i", "image info"),
                    row("r", "reload"),
                    row("q / Esc", "close"),
                ],
            );
        }

        f.render_widget(
            Paragraph::new(Span::styled(
                self.status_line(),
                Style::default()
                    .fg(Color::Rgb(140, 150, 175))
                    .bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let zoomed = matches!(self.zoom, Zoom::Scale(_));
        match k.code {
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
                self.show_info = false;
            }
            KeyCode::Char('i') => {
                self.show_info = !self.show_info;
                self.show_help = false;
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.show_help || self.show_info {
                    self.show_help = false;
                    self.show_info = false;
                } else {
                    return Action::Close;
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom_by(1.25, None),
            KeyCode::Char('-') => self.zoom_by(0.8, None),
            KeyCode::Char('f') | KeyCode::Char('0') => self.zoom = Zoom::Fit,
            KeyCode::Char('1') => {
                self.scale = self.scale.max(f64::MIN_POSITIVE);
                self.zoom_by(1.0 / self.scale, None);
            }
            KeyCode::Left if zoomed => self.pan(-4.0, 0.0),
            KeyCode::Right if zoomed => self.pan(4.0, 0.0),
            KeyCode::Up if zoomed => self.pan(0.0, -2.0),
            KeyCode::Down if zoomed => self.pan(0.0, 2.0),
            KeyCode::Char('h') if zoomed => self.pan(-4.0, 0.0),
            KeyCode::Char('l') if zoomed => self.pan(4.0, 0.0),
            KeyCode::Char('k') if zoomed => self.pan(0.0, -2.0),
            KeyCode::Char('j') if zoomed => self.pan(0.0, 2.0),
            KeyCode::Left | KeyCode::Char('p') | KeyCode::PageUp => self.nav(-1),
            KeyCode::Right | KeyCode::Char('n') | KeyCode::PageDown | KeyCode::Char(' ') => {
                self.nav(1)
            }
            KeyCode::Home => self.nav_to(0),
            KeyCode::End => self.nav_to(self.gallery.len().saturating_sub(1)),
            KeyCode::Char('[') => self.transform(image::imageops::rotate270),
            KeyCode::Char(']') => self.transform(image::imageops::rotate90),
            KeyCode::Char('x') => self.transform(image::imageops::flip_horizontal),
            KeyCode::Char('y') => self.transform(image::imageops::flip_vertical),
            KeyCode::Char('s') => {
                self.slideshow = !self.slideshow;
                self.ticks = 0;
            }
            KeyCode::Char('r') => self.load(),
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
        match m.kind {
            MouseEventKind::ScrollUp if inside => {
                self.zoom_by(1.25, Some((m.column, m.row)))
            }
            MouseEventKind::ScrollDown if inside => {
                self.zoom_by(0.8, Some((m.column, m.row)))
            }
            MouseEventKind::Down(MouseButton::Left) if inside => {
                self.drag = Some((m.column, m.row));
                let now = Instant::now();
                let dbl = self
                    .last_click
                    .map(|(t, c, r)| {
                        now.duration_since(t).as_millis() < 400 && c == m.column && r == m.row
                    })
                    .unwrap_or(false);
                if dbl {
                    // double-click toggles fit ↔ 100%, zooming at the cursor
                    if matches!(self.zoom, Zoom::Fit) {
                        self.scale = self.scale.max(f64::MIN_POSITIVE);
                        self.zoom_by(1.0 / self.scale, Some((m.column, m.row)));
                    } else {
                        self.zoom = Zoom::Fit;
                    }
                    self.last_click = None;
                } else {
                    self.last_click = Some((now, m.column, m.row));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((px, py)) = self.drag {
                    self.pan(px as f64 - m.column as f64, py as f64 - m.row as f64);
                }
                self.drag = Some((m.column, m.row));
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            _ => {}
        }
        Action::None
    }

    fn on_tick(&mut self) {
        if self.slideshow && self.gallery.len() > 1 && !self.show_help && !self.show_info {
            self.ticks += 1;
            if self.ticks >= SLIDESHOW_TICKS {
                self.ticks = 0;
                self.nav(1);
            }
        }
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    fn on_drop(&mut self, paths: &[PathBuf], _cut: bool) -> Action {
        if let Some(p) = paths.first().filter(|p| p.is_file()) {
            self.path = p.clone();
            self.scan_gallery();
            self.load();
        }
        Action::None
    }
}
