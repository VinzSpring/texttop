use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;

pub fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

/// Vertical color gradient fill.
pub fn gradient(buf: &mut Buffer, area: Rect, top: (u8, u8, u8), bot: (u8, u8, u8)) {
    for y in 0..area.height {
        let t = if area.height <= 1 {
            0.0
        } else {
            y as f32 / (area.height - 1) as f32
        };
        let c = Color::Rgb(
            lerp(top.0, bot.0, t),
            lerp(top.1, bot.1, t),
            lerp(top.2, bot.2, t),
        );
        for x in 0..area.width {
            if let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) {
                cell.set_symbol(" ");
                cell.set_bg(c);
            }
        }
    }
}

// Quadrant characters indexed by bitmask: TL=1, TR=2, BL=4, BR=8.
const QUAD: [&str; 16] = [
    " ", "▘", "▝", "▀", "▖", "▌", "▞", "▛", "▗", "▚", "▐", "▜", "▄", "▙", "▟", "█",
];

/// Box-average the source region covering virtual pixel (vx, vy) on a
/// gw×gh grid mapped over an iw×ih RGB24 image.
pub fn sample(px: &[u8], iw: u32, ih: u32, gw: u32, gh: u32, vx: u32, vy: u32) -> (u16, u16, u16) {
    let x0 = vx * iw / gw;
    let x1 = ((vx + 1) * iw / gw).max(x0 + 1).min(iw);
    let y0 = vy * ih / gh;
    let y1 = ((vy + 1) * ih / gh).max(y0 + 1).min(ih);
    let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * iw + x) * 3) as usize;
            if i + 2 < px.len() {
                r += px[i] as u32;
                g += px[i + 1] as u32;
                b += px[i + 2] as u32;
                n += 1;
            }
        }
    }
    if n == 0 {
        return (0, 0, 0);
    }
    ((r / n) as u16, (g / n) as u16, (b / n) as u16)
}

fn luma(c: (u16, u16, u16)) -> u32 {
    (299 * c.0 as u32 + 587 * c.1 as u32 + 114 * c.2 as u32) / 1000
}

fn avg(cols: &[(u16, u16, u16)]) -> Color {
    let n = cols.len().max(1) as u32;
    let r = cols.iter().map(|c| c.0 as u32).sum::<u32>() / n;
    let g = cols.iter().map(|c| c.1 as u32).sum::<u32>() / n;
    let b = cols.iter().map(|c| c.2 as u32).sum::<u32>() / n;
    Color::Rgb(r as u8, g as u8, b as u8)
}

/// Quadrant-fit an RGB24 buffer into (symbol, fg, bg) cells — each cell
/// carries a 2×2 pixel pattern with the best two-color fit.
fn compute_cells(px: &[u8], iw: u32, ih: u32, w: u16, h: u16) -> Vec<(&'static str, Color, Color)> {
    let gw = w as u32 * 2;
    let gh = h as u32 * 2;
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for cy in 0..h as u32 {
        for cx in 0..w as u32 {
            let q = [
                sample(px, iw, ih, gw, gh, cx * 2, cy * 2),
                sample(px, iw, ih, gw, gh, cx * 2 + 1, cy * 2),
                sample(px, iw, ih, gw, gh, cx * 2, cy * 2 + 1),
                sample(px, iw, ih, gw, gh, cx * 2 + 1, cy * 2 + 1),
            ];
            let lumas = [luma(q[0]), luma(q[1]), luma(q[2]), luma(q[3])];
            let lmin = *lumas.iter().min().unwrap();
            let lmax = *lumas.iter().max().unwrap();
            if lmax - lmin < 8 {
                // flat region: solid background color, no pattern noise
                let c = avg(&q);
                out.push((" ", c, c));
                continue;
            }
            let mid = (lmin + lmax) / 2;
            let mut mask = 0usize;
            let (mut hi, mut lo) = (Vec::new(), Vec::new());
            for (i, &l) in lumas.iter().enumerate() {
                if l > mid {
                    mask |= 1 << i;
                    hi.push(q[i]);
                } else {
                    lo.push(q[i]);
                }
            }
            out.push((QUAD[mask], avg(&hi), avg(&lo)));
        }
    }
    out
}

fn blit(buf: &mut Buffer, area: Rect, cells: &[(&'static str, Color, Color)]) {
    let mut i = 0usize;
    for cy in 0..area.height {
        for cx in 0..area.width {
            if let Some(cell) = buf.cell_mut((area.x + cx, area.y + cy)) {
                if let Some((sym, fg, bg)) = cells.get(i) {
                    cell.set_symbol(sym);
                    cell.set_fg(*fg);
                    cell.set_bg(*bg);
                } else {
                    // Defensive: never leave a prior glyph behind if the
                    // cell list is short (e.g. after a partial cache clear).
                    cell.set_symbol(" ");
                    cell.set_fg(Color::Reset);
                    cell.set_bg(Color::Reset);
                }
                cell.modifier = ratatui::style::Modifier::empty();
            }
            i += 1;
        }
    }
}

/// Memoized quadrant renderer: `id` identifies the source frame (e.g. seq).
/// When the frame, size, and area are unchanged, the previously computed
/// cells are blitted directly — no re-sampling of pixels.
#[derive(Default)]
pub struct RgbCache {
    key: (usize, u16, u16, u32, u32),
    cells: Vec<(&'static str, Color, Color)>,
}

impl RgbCache {
    pub fn clear(&mut self) {
        self.key = (0, 0, 0, 0, 0);
        self.cells.clear();
    }
}

pub fn draw_rgb_cached(
    buf: &mut Buffer,
    area: Rect,
    px: &[u8],
    iw: u32,
    ih: u32,
    id: usize,
    cache: &mut RgbCache,
) {
    if area.width == 0 || area.height == 0 || iw == 0 || ih == 0 {
        return;
    }
    let key = (id, area.width, area.height, iw, ih);
    if cache.key != key {
        cache.cells = compute_cells(px, iw, ih, area.width, area.height);
        cache.key = key;
    }
    blit(buf, area, &cache.cells);
}

/// Fill `area` with a solid background so no prior cell glyphs can bleed through.
pub fn fill(buf: &mut Buffer, area: Rect, bg: Color) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ");
                cell.set_fg(bg);
                cell.set_bg(bg);
                cell.modifier = ratatui::style::Modifier::empty();
            }
        }
    }
}

/// Translucent-looking drop shadow to the right and below a window rect.
pub fn shadow(buf: &mut Buffer, rect: Rect, screen: Rect) {
    let bg = Color::Rgb(4, 4, 10);
    let fg = Color::Rgb(30, 30, 46);
    let bottom = rect.y + rect.height;
    if bottom < screen.y + screen.height {
        let x_end = (rect.x + rect.width + 2).min(screen.x + screen.width);
        for x in (rect.x + 2)..x_end {
            if let Some(c) = buf.cell_mut((x, bottom)) {
                c.set_bg(bg);
                c.set_fg(fg);
            }
        }
    }
    for dx in 0..2u16 {
        let x = rect.x + rect.width + dx;
        if x < screen.x + screen.width {
            for y in (rect.y + 1)..bottom.min(screen.y + screen.height) {
                if let Some(c) = buf.cell_mut((x, y)) {
                    c.set_bg(bg);
                    c.set_fg(fg);
                }
            }
        }
    }
}

/// Centered keymap overlay, shared by all apps ('?' convention).
pub fn key_overlay(f: &mut Frame, area: Rect, title: &str, keys: &[(&str, &str)]) {
    let key_w = keys.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(8);
    let desc_w = keys.iter().map(|(_, d)| d.chars().count()).max().unwrap_or(8);
    let w = ((key_w + desc_w + 7) as u16).min(area.width);
    let h = (keys.len() as u16 + 4).min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect::new(x, y, w, h);
    fill(f.buffer_mut(), rect, Color::Rgb(16, 19, 32));
    let buf = f.buffer_mut();
    buf.set_string(
        rect.x + 2,
        rect.y + 1,
        title,
        Style::default()
            .fg(Color::Rgb(150, 200, 255))
            .bg(Color::Rgb(16, 19, 32))
            .add_modifier(Modifier::BOLD),
    );
    for (i, (k, d)) in keys.iter().enumerate() {
        let iy = rect.y + 3 + i as u16;
        if iy >= rect.y + rect.height.saturating_sub(1) {
            break;
        }
        buf.set_string(
            rect.x + 2,
            iy,
            format!("{k:>key_w$}"),
            Style::default()
                .fg(Color::Rgb(210, 215, 235))
                .bg(Color::Rgb(16, 19, 32))
                .add_modifier(Modifier::BOLD),
        );
        buf.set_string(
            rect.x + 4 + key_w as u16,
            iy,
            *d,
            Style::default()
                .fg(Color::Rgb(120, 130, 160))
                .bg(Color::Rgb(16, 19, 32)),
        );
    }
}

pub fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn fmt_dur(d: std::time::Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}
