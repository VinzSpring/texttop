//! Wallpaper chooser — gradient themes + a custom-image tile (via Files).

use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageDecoder, ImageReader, RgbImage};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::ui::{Controls, Item, Look};
use super::{Action, AppWindow, Launch};
use crate::gfx;

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Image,
    Done,
}

/// `(top_rgb, bottom_rgb, name)` — shared by the desktop and this chooser.
pub type Theme = ((u8, u8, u8), (u8, u8, u8), &'static str);

pub const THEMES: &[Theme] = &[
    // classic
    ((10, 14, 44), (66, 14, 78), "midnight"),
    ((3, 30, 52), (0, 96, 116), "ocean"),
    ((34, 8, 10), (140, 66, 12), "sunset"),
    ((6, 30, 18), (22, 88, 54), "forest"),
    ((14, 6, 34), (0, 110, 100), "aurora"),
    ((44, 10, 64), (150, 40, 80), "vapor"),
    ((30, 30, 38), (8, 8, 12), "graphite"),
    // vivid
    ((48, 0, 96), (255, 0, 200), "neon"),
    ((180, 20, 90), (40, 220, 255), "candy"),
    ((20, 0, 80), (80, 40, 255), "plasma"),
    ((90, 8, 0), (255, 140, 20), "ember"),
    ((70, 0, 30), (255, 40, 120), "cherry"),
    ((0, 20, 90), (0, 255, 180), "electric"),
    ((0, 70, 80), (255, 120, 90), "coral"),
    ((90, 20, 140), (255, 160, 40), "mango"),
    ((0, 40, 100), (140, 230, 255), "ice"),
    ((60, 0, 0), (255, 200, 40), "lava"),
];

/// Pixel size of one terminal cell — must match ImageViewer / web / video.
const CELL_PX_W: u32 = 8;
const CELL_PX_H: u32 = 16;
const MAX_SRC: u32 = 2400;
const FLATTEN: image::Rgb<u8> = image::Rgb([8, 10, 18]);

const COLS: usize = 5;
const TILE_GAP: u16 = 1;
const LABEL_H: u16 = 1;
const HEADER_H: u16 = 1;
const FOOTER_H: u16 = 1;

const BG: Color = Color::Rgb(12, 14, 22);
const FG: Color = Color::Rgb(215, 218, 230);
const DIM: Color = Color::Rgb(140, 150, 175);
const ACCENT: Color = Color::Rgb(0, 190, 255);
const SEL: Color = Color::Rgb(255, 220, 90);
const APPLIED: Color = Color::Rgb(110, 230, 140);
const CUSTOM_BG: Color = Color::Rgb(22, 28, 42);

pub fn is_image_path(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico"
    )
}

/// Cover-fill an image into a cell-grid canvas (RGB24, no letterbox).
pub fn cover_rgb(path: &Path, cells_w: u16, cells_h: u16) -> Result<RgbImage, String> {
    if cells_w == 0 || cells_h == 0 {
        return Err("empty desk".into());
    }
    let img = load_oriented(path).map_err(|e| e.to_string())?;
    let mut rgba = img.to_rgba8();
    if rgba.width() > MAX_SRC || rgba.height() > MAX_SRC {
        rgba = image::imageops::thumbnail(&rgba, MAX_SRC, MAX_SRC);
    }
    let (iw, ih) = (rgba.width().max(1), rgba.height().max(1));
    let cw = (cells_w as u32).saturating_mul(CELL_PX_W).max(1);
    let ch = (cells_h as u32).saturating_mul(CELL_PX_H).max(1);

    let scale = (cw as f64 / iw as f64).max(ch as f64 / ih as f64);
    let vis_w = ((cw as f64) / scale).min(iw as f64).max(1.0);
    let vis_h = ((ch as f64) / scale).min(ih as f64).max(1.0);
    let x0 = ((iw as f64 - vis_w) / 2.0).floor().max(0.0) as u32;
    let y0 = ((ih as f64 - vis_h) / 2.0).floor().max(0.0) as u32;
    let x1 = ((x0 as f64 + vis_w).ceil() as u32).min(iw).max(x0 + 1);
    let y1 = ((y0 as f64 + vis_h).ceil() as u32).min(ih).max(y0 + 1);

    let crop = image::imageops::crop_imm(&rgba, x0, y0, x1 - x0, y1 - y0).to_image();
    let resized = image::imageops::resize(&crop, cw, ch, image::imageops::FilterType::Triangle);

    let mut canvas = RgbImage::from_pixel(cw, ch, FLATTEN);
    for (px, py, p) in resized.enumerate_pixels() {
        let a = p[3] as u32;
        let blend = |fg: u8, bg: u8| ((fg as u32 * a + bg as u32 * (255 - a)) / 255) as u8;
        canvas.put_pixel(
            px,
            py,
            image::Rgb([
                blend(p[0], FLATTEN[0]),
                blend(p[1], FLATTEN[1]),
                blend(p[2], FLATTEN[2]),
            ]),
        );
    }
    Ok(canvas)
}

fn load_oriented(path: &Path) -> Result<DynamicImage, image::ImageError> {
    let mut decoder = ImageReader::open(path)?.with_guessed_format()?.into_decoder()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

pub struct WallpaperChooser {
    /// Highlighted tile (`0..THEMES.len()` themes, last index = custom image).
    sel: usize,
    /// Last applied gradient index (fallback when image is cleared).
    applied_theme: usize,
    /// Image currently applied on the desktop (if any).
    applied_image: Option<PathBuf>,
    last_click: Option<(usize, std::time::Instant)>,
    /// Mini preview for the custom-image tile.
    thumb: Option<RgbImage>,
    thumb_for: Option<PathBuf>,
    thumb_seq: usize,
    thumb_cache: gfx::RgbCache,
    ui: Controls<Btn>,
}

impl WallpaperChooser {
    pub fn new(theme: usize, image: Option<PathBuf>) -> Self {
        let n = THEMES.len().max(1);
        let theme = theme % n;
        let sel = if image.is_some() {
            Self::custom_idx()
        } else {
            theme
        };
        WallpaperChooser {
            sel,
            applied_theme: theme,
            applied_image: image,
            last_click: None,
            ui: Controls::new(),
            thumb: None,
            thumb_for: None,
            thumb_seq: 0,
            thumb_cache: gfx::RgbCache::default(),
        }
    }

    fn custom_idx() -> usize {
        THEMES.len()
    }

    fn tile_count(&self) -> usize {
        THEMES.len() + 1
    }

    fn is_custom(&self, idx: usize) -> bool {
        idx == Self::custom_idx()
    }

    fn cols(&self) -> usize {
        COLS.min(self.tile_count()).max(1)
    }

    fn rows(&self) -> usize {
        self.tile_count().div_ceil(self.cols())
    }

    fn picker_start(&self) -> PathBuf {
        if let Some(p) = &self.applied_image {
            if let Some(parent) = p.parent().filter(|p| p.is_dir()) {
                return parent.to_path_buf();
            }
        }
        dirs::picture_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn open_files_picker(&mut self) -> Action {
        self.sel = Self::custom_idx();
        Action::Launch(Launch::WallpaperPicker {
            start: self.picker_start(),
        })
    }

    fn move_sel(&mut self, dx: isize, dy: isize) -> Action {
        let cols = self.cols() as isize;
        let n = self.tile_count() as isize;
        let cur = self.sel as isize;
        let col = cur % cols;
        let row = cur / cols;
        let nrow = self.rows() as isize;

        let mut ncol = col + dx;
        let mut nrow_i = row + dy;
        if ncol < 0 {
            ncol = cols - 1;
            nrow_i -= 1;
        } else if ncol >= cols {
            ncol = 0;
            nrow_i += 1;
        }
        if nrow_i < 0 {
            nrow_i = nrow - 1;
        } else if nrow_i >= nrow {
            nrow_i = 0;
        }
        let mut idx = nrow_i * cols + ncol;
        if idx >= n {
            idx = n - 1;
        }
        if idx < 0 {
            idx = 0;
        }
        self.focus_tile(idx as usize)
    }

    fn focus_tile(&mut self, idx: usize) -> Action {
        self.sel = idx.min(self.tile_count().saturating_sub(1));
        Action::None
    }

    fn activate_tile(&mut self, idx: usize, close: bool) -> Action {
        self.sel = idx;
        if self.is_custom(idx) {
            self.open_files_picker()
        } else if close {
            self.apply_theme_and_close(idx)
        } else {
            self.select_theme(idx)
        }
    }

    fn select_theme(&mut self, idx: usize) -> Action {
        let idx = idx.min(THEMES.len().saturating_sub(1));
        self.sel = idx;
        self.applied_theme = idx;
        self.applied_image = None;
        self.thumb = None;
        self.thumb_for = None;
        self.thumb_cache.clear();
        Action::SetTheme(idx)
    }

    fn apply_theme_and_close(&mut self, idx: usize) -> Action {
        let idx = idx.min(THEMES.len().saturating_sub(1));
        self.sel = idx;
        self.applied_theme = idx;
        self.applied_image = None;
        self.thumb = None;
        self.thumb_for = None;
        self.thumb_cache.clear();
        Action::SetThemeAndClose(idx)
    }

    fn apply_image_local(&mut self, path: PathBuf) {
        self.sel = Self::custom_idx();
        self.applied_image = Some(path);
        self.thumb = None;
        self.thumb_for = None;
        self.thumb_cache.clear();
    }

    fn ensure_thumb(&mut self, area: Rect) {
        let Some(path) = self.applied_image.clone() else {
            self.thumb = None;
            self.thumb_for = None;
            return;
        };
        if self.thumb_for.as_ref() == Some(&path)
            && self.thumb.as_ref().is_some_and(|t| {
                t.width() == area.width as u32 * CELL_PX_W
                    && t.height() == area.height as u32 * CELL_PX_H
            })
        {
            return;
        }
        match cover_rgb(&path, area.width, area.height) {
            Ok(img) => {
                self.thumb = Some(img);
                self.thumb_for = Some(path);
                self.thumb_seq = self.thumb_seq.wrapping_add(1);
                self.thumb_cache.clear();
            }
            Err(_) => {
                self.thumb = None;
                self.thumb_for = None;
            }
        }
    }

    fn tile_rects(&self, area: Rect) -> Vec<(usize, Rect, Rect)> {
        let mut out = Vec::new();
        if area.width < 8 || area.height < HEADER_H + FOOTER_H + 4 {
            return out;
        }
        let grid = Rect {
            x: area.x,
            y: area.y + HEADER_H,
            width: area.width,
            height: area.height.saturating_sub(HEADER_H + FOOTER_H),
        };
        let cols = self.cols() as u16;
        let rows = self.rows() as u16;
        if cols == 0 || rows == 0 || grid.height < 3 {
            return out;
        }
        let tile_w = (grid.width.saturating_sub(TILE_GAP * cols.saturating_sub(1))) / cols;
        let tile_h = (grid.height.saturating_sub(TILE_GAP * rows.saturating_sub(1))) / rows;
        if tile_w < 6 || tile_h < 3 {
            return out;
        }
        let grad_h = tile_h.saturating_sub(LABEL_H).max(2);

        for i in 0..self.tile_count() {
            let c = (i % self.cols()) as u16;
            let r = (i / self.cols()) as u16;
            let x = grid.x + c * (tile_w + TILE_GAP);
            let y = grid.y + r * (tile_h + TILE_GAP);
            let grad = Rect::new(x, y, tile_w, grad_h);
            let label = Rect::new(x, y + grad_h, tile_w, LABEL_H);
            out.push((i, grad, label));
        }
        out
    }

    fn hit_test(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        self.tile_rects(area)
            .into_iter()
            .find(|(_, grad, label)| {
                let full = Rect {
                    x: grad.x,
                    y: grad.y,
                    width: grad.width,
                    height: grad.height + label.height,
                };
                col >= full.x
                    && col < full.x + full.width
                    && row >= full.y
                    && row < full.y + full.height
            })
            .map(|(i, _, _)| i)
    }

    fn draw_border(buf: &mut ratatui::buffer::Buffer, r: Rect, color: Color) {
        if r.width == 0 || r.height == 0 {
            return;
        }
        let mut set = |x: u16, y: u16, sym: &str| {
            if let Some(cell) = buf.cell_mut((x, y)) {
                let bg = cell.bg;
                cell.set_symbol(sym);
                cell.set_fg(color);
                cell.set_bg(bg);
            }
        };
        let x1 = r.x + r.width - 1;
        let y1 = r.y + r.height - 1;
        set(r.x, r.y, "┌");
        set(x1, r.y, "┐");
        set(r.x, y1, "└");
        set(x1, y1, "┘");
        for x in r.x + 1..x1 {
            set(x, r.y, "─");
            set(x, y1, "─");
        }
        for y in r.y + 1..y1 {
            set(r.x, y, "│");
            set(x1, y, "│");
        }
    }

    fn draw_custom_placeholder(buf: &mut ratatui::buffer::Buffer, r: Rect) {
        gfx::fill(buf, r, CUSTOM_BG);
        if r.width < 3 || r.height < 2 {
            return;
        }
        let icon = "＋";
        let hint = "files";
        let ix = r.x + r.width.saturating_sub(icon.width() as u16) / 2;
        let iy = r.y + r.height.saturating_sub(2) / 2;
        if let Some(cell) = buf.cell_mut((ix, iy)) {
            cell.set_symbol(icon);
            cell.set_fg(ACCENT);
            cell.set_bg(CUSTOM_BG);
        }
        if iy + 1 < r.y + r.height {
            let hx = r.x + r.width.saturating_sub(hint.width() as u16) / 2;
            for (i, ch) in hint.chars().enumerate() {
                if let Some(cell) = buf.cell_mut((hx + i as u16, iy + 1)) {
                    cell.set_symbol(&ch.to_string());
                    cell.set_fg(DIM);
                    cell.set_bg(CUSTOM_BG);
                }
            }
        }
    }

    fn applied_label(&self) -> String {
        if let Some(p) = &self.applied_image {
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.to_string_lossy().to_string())
        } else {
            THEMES[self.applied_theme % THEMES.len()].2.to_string()
        }
    }

    fn label_for(&self, idx: usize) -> String {
        if self.is_custom(idx) {
            if self.applied_image.is_some() {
                let name = self.applied_label();
                format!("✓ {name}")
            } else {
                "image…".into()
            }
        } else {
            let mark = if self.applied_image.is_none() && idx == self.applied_theme {
                "✓ "
            } else {
                ""
            };
            format!("{mark}{}", THEMES[idx].2)
        }
    }
}

impl AppWindow for WallpaperChooser {
    fn title(&self) -> String {
        if self.applied_image.is_some() {
            format!("Wallpaper · {}", self.applied_label())
        } else if self.is_custom(self.sel) {
            "Wallpaper · image…".into()
        } else {
            format!("Wallpaper · {}", THEMES[self.sel % THEMES.len()].2)
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        {
            let buf = f.buffer_mut();
            for y in area.y..area.y + area.height {
                for x in area.x..area.x + area.width {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_symbol(" ");
                        cell.set_fg(BG);
                        cell.set_bg(BG);
                    }
                }
            }
        }

        self.ui.begin();
        self.ui.toolbar(
            f.buffer_mut(),
            Rect::new(area.x, area.y, area.width, 1),
            &[Item::Label(" Click a theme to try it".into(), FG)],
            &[
                Item::btn("🖼 Image…", Btn::Image),
                Item::with("✓ Done", Btn::Done, Look::Primary),
            ],
        );

        let tiles = self.tile_rects(area);
        if let Some((_, grad, _)) = tiles.iter().find(|(i, _, _)| self.is_custom(*i)) {
            let grad = *grad;
            self.ensure_thumb(grad);
        }

        for (i, grad, label) in &tiles {
            let selected = *i == self.sel;
            let applied = if self.is_custom(*i) {
                self.applied_image.is_some()
            } else {
                self.applied_image.is_none() && *i == self.applied_theme
            };

            if self.is_custom(*i) {
                if self.applied_image.is_some() {
                    if let Some(thumb) = &self.thumb {
                        gfx::fill(f.buffer_mut(), *grad, CUSTOM_BG);
                        gfx::draw_rgb_cached(
                            f.buffer_mut(),
                            *grad,
                            thumb.as_raw(),
                            thumb.width(),
                            thumb.height(),
                            self.thumb_seq,
                            &mut self.thumb_cache,
                        );
                    } else {
                        Self::draw_custom_placeholder(f.buffer_mut(), *grad);
                    }
                } else {
                    Self::draw_custom_placeholder(f.buffer_mut(), *grad);
                }
            } else {
                let th = THEMES[*i];
                gfx::gradient_for(f.buffer_mut(), *grad, th.0, th.1, true);
            }

            let border_color = if selected {
                SEL
            } else if applied {
                APPLIED
            } else {
                Color::Rgb(180, 185, 200)
            };
            Self::draw_border(f.buffer_mut(), *grad, border_color);

            let name = self.label_for(*i);
            let name_style = if selected {
                Style::default()
                    .fg(SEL)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD)
            } else if applied {
                Style::default().fg(APPLIED).bg(BG)
            } else {
                Style::default().fg(FG).bg(BG)
            };
            let truncated = if name.chars().count() > label.width as usize {
                name.chars()
                    .take(label.width.saturating_sub(1) as usize)
                    .collect::<String>()
                    + "…"
            } else {
                name
            };
            let pad = label
                .width
                .saturating_sub(truncated.chars().count() as u16)
                / 2;
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!("{:pad$}{truncated}", "", pad = pad as usize),
                    name_style,
                )),
                *label,
            );
        }

        let footer_y = area.y + area.height.saturating_sub(1);
        let kind = if self.applied_image.is_some() {
            "image"
        } else {
            "theme"
        };
        let foot = Line::from(vec![
            Span::styled(
                format!(" applied · {kind} · "),
                Style::default().fg(DIM).bg(BG),
            ),
            Span::styled(
                self.applied_label(),
                Style::default()
                    .fg(APPLIED)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        f.render_widget(
            Paragraph::new(foot),
            Rect::new(area.x, footer_y, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => Action::Close,
            KeyCode::Char('i') | KeyCode::Char('I') => self.open_files_picker(),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_tile(self.sel, true),
            KeyCode::Left | KeyCode::Char('h') => self.move_sel(-1, 0),
            KeyCode::Right | KeyCode::Char('l') => self.move_sel(1, 0),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(0, -1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(0, 1),
            KeyCode::Home => self.focus_tile(0),
            KeyCode::End => self.focus_tile(Self::custom_idx()),
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let n = c.to_digit(10).unwrap_or(0) as usize;
                if n >= 1 && n <= THEMES.len() {
                    self.activate_tile(n - 1, false)
                } else {
                    Action::None
                }
            }
            _ => Action::None,
        }
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        match self.ui.route(&m) {
            Some(Some(Btn::Image)) => return self.open_files_picker(),
            Some(Some(Btn::Done)) => return Action::Close,
            Some(None) => return Action::None,
            None => {}
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(i) = self.hit_test(area, m.column, m.row) else {
                    return Action::None;
                };
                let now = std::time::Instant::now();
                let dbl = self
                    .last_click
                    .map(|(prev, t)| prev == i && now.duration_since(t).as_millis() < 400)
                    .unwrap_or(false);
                self.last_click = Some((i, now));
                if self.is_custom(i) {
                    self.open_files_picker()
                } else if dbl {
                    self.activate_tile(i, true)
                } else {
                    self.activate_tile(i, false)
                }
            }
            MouseEventKind::ScrollUp => self.move_sel(0, -1),
            MouseEventKind::ScrollDown => self.move_sel(0, 1),
            _ => Action::None,
        }
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    fn on_drop(&mut self, paths: &[PathBuf], _cut: bool) -> Action {
        let Some(path) = paths.iter().find(|p| p.is_file() && is_image_path(p)) else {
            return Action::None;
        };
        self.apply_image_local(path.clone());
        Action::SetWallpaperImage(path.clone())
    }

    fn on_wallpaper_changed(&mut self, image: Option<PathBuf>, theme: usize) {
        self.applied_theme = theme % THEMES.len().max(1);
        if let Some(path) = image {
            self.apply_image_local(path);
        } else {
            self.applied_image = None;
            self.thumb = None;
            self.thumb_for = None;
            self.thumb_cache.clear();
            if self.is_custom(self.sel) {
                self.sel = self.applied_theme;
            }
        }
    }
}
