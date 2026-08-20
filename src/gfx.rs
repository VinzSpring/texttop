use std::sync::atomic::{AtomicU8, Ordering};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;

// ── truecolor detection ──────────────────────────────────────────────────────

/// 0 = not probed yet, 1 = 24-bit, 2 = 256-color. Probed once and cached; an
/// atomic rather than a `OnceLock` so tests can render both paths (see
/// [`force_truecolor`]) instead of being stuck with whatever terminal happened
/// to launch `cargo test`.
static TRUECOLOR: AtomicU8 = AtomicU8::new(0);

fn detect_truecolor() -> bool {
    if let Ok(v) = std::env::var("TERM_PROGRAM") {
        // Apple Terminal may advertise truecolor but does not reliably render
        // 24-bit background color sequences in this app.
        if v == "Apple_Terminal" {
            return false;
        }
        if matches!(v.as_str(), "iTerm.app" | "WezTerm" | "ghostty" | "Hyper") {
            return true;
        }
    }
    if let Ok(v) = std::env::var("COLORTERM") {
        if v == "truecolor" || v == "24bit" {
            return true;
        }
    }
    if let Ok(v) = std::env::var("TERM") {
        if v.contains("truecolor") || v == "xterm-kitty" {
            return true;
        }
    }
    false
}

pub fn truecolor() -> bool {
    match TRUECOLOR.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let v = detect_truecolor();
            TRUECOLOR.store(1 + !v as u8, Ordering::Relaxed);
            v
        }
    }
}

/// Pin the color mode. Tests only — the app probes the terminal instead.
#[cfg(test)]
pub fn force_truecolor(v: bool) {
    TRUECOLOR.store(1 + !v as u8, Ordering::Relaxed);
}

/// The six levels of the xterm 6×6×6 color cube. Note they are not evenly
/// spaced — the gap from 0 to 95 is wider than the rest — so a linear
/// `v * 5 / 255` mapping is badly wrong for dark colors (it pushes the
/// near-black window chrome this app uses up to a saturated 95).
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn nearest_cube(v: u8) -> usize {
    CUBE.iter()
        .enumerate()
        .min_by_key(|(_, &l)| (v as i32 - l as i32).abs())
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn dist2(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
    let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// Largest channel spread a color may have and still be treated as neutral.
/// The gray ramp is spaced 10 apart, so anything under this loses at most
/// half a step of hue by being desaturated onto it.
const NEUTRAL_CHROMA: u8 = 20;

fn to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let (ri, gi, bi) = (nearest_cube(r), nearest_cube(g), nearest_cube(b));
    let cube = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let cube_idx = (16 + 36 * ri + 6 * gi + bi) as u8;

    // Indices 232..=255 are a finer gray ramp at levels 8, 18, ... 238. It is
    // denser than the cube in the darks, so for neutral colors it wins — but
    // only for neutral ones. Plain RGB distance would also pull saturated
    // darks onto it (a dark purple is numerically nearer mid-gray than it is
    // to any cube entry), which strips the hue and then snaps back to a
    // saturated cube entry once the color is bright enough. That turned the
    // desktop's dark-purple gradient into flat gray with one hard magenta
    // edge, so only consider the ramp when the color really is near-neutral.
    let chroma = r.max(g).max(b) - r.min(g).min(b);
    if chroma > NEUTRAL_CHROMA {
        return cube_idx;
    }

    let avg = (r as i32 + g as i32 + b as i32) / 3;
    let step = ((avg - 8 + 5) / 10).clamp(0, 23);
    let level = (8 + step * 10) as u8;
    let gray = (level, level, level);

    if dist2((r, g, b), gray) < dist2((r, g, b), cube) {
        232 + step as u8
    } else {
        cube_idx
    }
}

/// Create a Color, falling back to the nearest xterm-256 index on terminals
/// that do not support 24-bit RGB.
pub fn rgb(r: u8, g: u8, b: u8) -> Color {
    rgb_for(r, g, b, truecolor())
}

/// `rgb` with the terminal capability passed in, so tests can exercise both
/// modes (the ambient one is cached process-wide in a `OnceLock`).
pub fn rgb_for(r: u8, g: u8, b: u8, truecolor: bool) -> Color {
    if truecolor {
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(to_ansi256(r, g, b))
    }
}

/// `adapt` with the terminal capability passed in. See [`rgb_for`].
pub fn adapt_for(c: Color, truecolor: bool) -> Color {
    match c {
        Color::Rgb(r, g, b) if !truecolor => Color::Indexed(to_ansi256(r, g, b)),
        other => other,
    }
}

/// Remap every `Color::Rgb` cell in `area` to the nearest xterm-256 index.
///
/// Run once over the finished frame, this covers every app in the codebase
/// without each one having to know what the terminal supports. A no-op when
/// `truecolor` is true.
pub fn adapt_buffer(buf: &mut Buffer, area: Rect, truecolor: bool) {
    if truecolor {
        return;
    }
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_fg(adapt_for(cell.fg, false));
                cell.set_bg(adapt_for(cell.bg, false));
            }
        }
    }
}

/// The color the terminal actually paints for an xterm-256 index.
pub fn palette_level(idx: u8) -> (u8, u8, u8) {
    if idx >= 232 {
        let v = 8 + (idx as u16 - 232) * 10;
        (v as u8, v as u8, v as u8)
    } else if idx >= 16 {
        let i = idx as usize - 16;
        (CUBE[i / 36], CUBE[(i / 6) % 6], CUBE[i % 6])
    } else {
        // The low 16 are theme-dependent; never emitted by to_ansi256.
        (0, 0, 0)
    }
}

/// Relative luminance, 0-255 (ITU-R BT.601 weights).
pub fn luminance(r: u8, g: u8, b: u8) -> u8 {
    ((299 * r as u32 + 587 * g as u32 + 114 * b as u32) / 1000).min(255) as u8
}

/// The xterm-256 grey-ramp index (232..=255) closest to a given luminance.
/// The ramp runs 8, 18, ... 238 — the only densely-spaced region the palette
/// has in the darks.
pub fn grey_ramp(luma: u8) -> u8 {
    let step = ((luma as i32 - 8 + 5) / 10).clamp(0, 23);
    232 + step as u8
}

// ── drawing helpers ──────────────────────────────────────────────────────────

pub fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

/// Vertical color gradient fill.  The gradient is quantised to at most 32
/// distinct colour steps so that terminals that struggle with many unique
/// background-colour sequences produce clean bands rather than visual noise.
pub fn gradient(buf: &mut Buffer, area: Rect, top: (u8, u8, u8), bot: (u8, u8, u8)) {
    gradient_for(buf, area, top, bot, truecolor())
}

/// `gradient` with the terminal capability passed in. See [`rgb_for`].
pub fn gradient_for(
    buf: &mut Buffer,
    area: Rect,
    top: (u8, u8, u8),
    bot: (u8, u8, u8),
    truecolor: bool,
) {
    const STEPS: u16 = 32;
    for y in 0..area.height {
        let step = if area.height <= 1 {
            0
        } else {
            y * (STEPS - 1) / (area.height - 1)
        };
        let t = step as f32 / (STEPS - 1) as f32;
        let (r, g, b) = (
            lerp(top.0, bot.0, t),
            lerp(top.1, bot.1, t),
            lerp(top.2, bot.2, t),
        );
        // Without 24-bit color there is no honest way to draw these themes.
        // The cube's darkest non-black level is 95 — brighter than most of the
        // ramp — so rounding to the nearest entry gives a few garish bands,
        // and dithering two entries together makes the wallpaper visibly
        // striped. The 24-step grey ramp is the one densely-spaced region down
        // here, so track luminance and let the hue go: a smooth dark wash
        // reads far better than banding or texture. Windows and icons keep
        // their color, since those sit at brightnesses the cube covers.
        let c = if truecolor {
            Color::Rgb(r, g, b)
        } else {
            Color::Indexed(grey_ramp(luminance(r, g, b)))
        };
        for x in 0..area.width {
            if let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) {
                cell.set_symbol(" ");
                cell.set_fg(c);
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
    rgb(r as u8, g as u8, b as u8)
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
    let bg = rgb(4, 4, 10);
    let fg = rgb(30, 30, 46);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Level rendered by the terminal for a given xterm-256 index.
    fn level_of(idx: u8) -> (u8, u8, u8) {
        if idx >= 232 {
            let v = 8 + (idx as u16 - 232) * 10;
            (v as u8, v as u8, v as u8)
        } else {
            let i = idx as usize - 16;
            (CUBE[i / 36], CUBE[(i / 6) % 6], CUBE[i % 6])
        }
    }

    fn chroma(c: (u8, u8, u8)) -> u8 {
        c.0.max(c.1).max(c.2) - c.0.min(c.1).min(c.2)
    }

    /// Nearest entry among the candidates the mapping is allowed to pick:
    /// the whole palette for neutral colors, the cube alone for saturated
    /// ones (the gray ramp is off-limits there — it would erase the hue).
    fn assert_nearest_allowed(c: (u8, u8, u8)) {
        let got = to_ansi256(c.0, c.1, c.2);
        let got_err = dist2(c, level_of(got));
        let last = if chroma(c) > NEUTRAL_CHROMA { 231 } else { 255 };
        for idx in 16..=last {
            let err = dist2(c, level_of(idx));
            assert!(
                err >= got_err,
                "{c:?}: picked {got} (err {got_err}) but {idx} is closer (err {err})",
            );
        }
    }

    #[test]
    fn picks_the_nearest_allowed_entry() {
        // The window chrome, shadows and text colors this app actually uses,
        // plus the endpoints of every desktop theme gradient.
        for c in [
            (16, 19, 32),
            (4, 4, 10),
            (30, 30, 46),
            (24, 26, 38),
            (150, 200, 255),
            (120, 130, 160),
            (210, 215, 235),
            (0, 0, 0),
            (255, 255, 255),
            (10, 14, 44),
            (66, 14, 78),
            (3, 30, 52),
            (0, 96, 116),
            (34, 8, 10),
            (140, 66, 12),
        ] {
            assert_nearest_allowed(c);
        }
    }

    #[test]
    fn saturated_colors_keep_their_hue() {
        // Regression: preferring the gray ramp on pure RGB distance flattened
        // the dark half of the "midnight" desktop gradient (10,14,44) ->
        // (66,14,78) to gray, then jumped straight to a saturated magenta.
        for step in 0..32u32 {
            let t = step as f32 / 31.0;
            let c = (
                lerp(10, 66, t),
                lerp(14, 14, t),
                lerp(44, 78, t),
            );
            let idx = to_ansi256(c.0, c.1, c.2);
            assert!(
                idx < 232,
                "{c:?} (chroma {}) landed on gray ramp index {idx}",
                chroma(c),
            );
        }
    }

    #[test]
    fn dark_chrome_stays_dark_and_neutral() {
        // Regression: a linear cube mapping sent the near-black window chrome
        // to (0,0,95) and (95,95,95) — saturated blue and mid gray.
        for c in [(16, 19, 32), (4, 4, 10), (30, 30, 46), (24, 26, 38)] {
            let idx = to_ansi256(c.0, c.1, c.2);
            let got = level_of(idx);
            assert!(
                dist2(c, got) < 400,
                "{c:?} -> index {idx} = {got:?}, too far off",
            );
        }
    }

    #[test]
    fn grays_use_the_fine_ramp() {
        // 128 is 8 + 12*10, exactly on the gray ramp; the cube's nearest is 135.
        assert_eq!(to_ansi256(128, 128, 128), 244);
    }

    #[test]
    fn every_index_is_in_range() {
        for r in (0..=255u8).step_by(17) {
            for g in (0..=255u8).step_by(17) {
                for b in (0..=255u8).step_by(17) {
                    let i = to_ansi256(r, g, b);
                    assert!(i >= 16, "({r},{g},{b}) -> {i}");
                }
            }
        }
    }
}
