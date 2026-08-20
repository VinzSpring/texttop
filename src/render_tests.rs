//! Golden-file rendering tests for the three surfaces that draw pixels or
//! chrome: the desktop UI, the image viewer, and the browser's page area.
//!
//! See [`crate::snapshot`] for the harness and how to refresh goldens.

use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::app::Desktop;
use crate::apps::imageviewer::ImageViewer;
use crate::apps::AppWindow;
use crate::gfx;
use crate::snapshot::{
    assert_snapshot, assert_snapshot_masked, render_mode, test_image_file, test_pattern,
};

const DESK_W: u16 = 150;
const DESK_H: u16 = 45;

// ── desktop UI ───────────────────────────────────────────────────────────────

/// The right end of the taskbar, holding the git branch, clock and date.
///
/// These legitimately change between runs. Masking them beats loosening the
/// threshold: a layout regression moves surprisingly few cells — widening the
/// icon column shifts only ~1% of a 150x45 grid — so a gate slack enough to
/// absorb a ticking clock would sleep through it.
fn volatile_taskbar() -> [Rect; 1] {
    [Rect::new(DESK_W - 44, DESK_H - 1, 44, 1)]
}

/// The whole desktop as a user first sees it: wallpaper gradient, sidebar
/// icons, the window that opens on a fresh profile, and the taskbar.
#[test]
fn ui_desktop() {
    let mut desktop = Desktop::new();
    let buf = render_mode(true, DESK_W, DESK_H, |f| desktop.render(f));
    assert_snapshot_masked("ui_desktop", &buf, &volatile_taskbar(), 0.999, 0.999);
}

/// The same desktop as a 256-color terminal receives it. This is the surface
/// that looked wrong in Apple Terminal, so it gets its own golden.
#[test]
fn ui_desktop_256() {
    let mut desktop = Desktop::new();
    let buf = render_mode(false, DESK_W, DESK_H, |f| desktop.render(f));
    assert_snapshot_masked("ui_desktop_256", &buf, &volatile_taskbar(), 0.999, 0.999);
}

// ── wallpaper gradient ───────────────────────────────────────────────────────

/// The "midnight" theme, the default wallpaper and the exact case that
/// rendered as a few garish bands on a 256-color terminal.
const MIDNIGHT: ((u8, u8, u8), (u8, u8, u8)) = ((10, 14, 44), (66, 14, 78));

#[test]
fn gradient_truecolor() {
    let buf = render_mode(true, 40, 24, |f| {
        let area = f.area();
        gfx::gradient(f.buffer_mut(), area, MIDNIGHT.0, MIDNIGHT.1);
    });
    assert_snapshot("gradient_truecolor", &buf, 1.0, 1.0);
}

#[test]
fn gradient_256() {
    let buf = render_mode(false, 40, 24, |f| {
        let area = f.area();
        gfx::gradient(f.buffer_mut(), area, MIDNIGHT.0, MIDNIGHT.1);
    });
    assert_snapshot("gradient_256", &buf, 1.0, 1.0);
}

/// The wallpaper must stay dark on a 256-color terminal, and must track the
/// intended color closely.
///
/// Rounding each step to its nearest palette entry used to land on the cube's
/// 95 level — more than twice the intended brightness — which is what made the
/// desktop look garish. What matters is the color the cell optically resolves
/// to, not the entries used to build it: a blended cell deliberately pairs a
/// brighter foreground with a dark background at low coverage.
#[test]
fn gradient_256_tracks_the_intended_color() {
    let (w, h) = (40u16, 24u16);
    let buf = render_mode(false, w, h, |f| {
        let area = f.area();
        gfx::gradient(f.buffer_mut(), area, MIDNIGHT.0, MIDNIGHT.1);
    });
    const STEPS: u16 = 32;
    for y in 0..h {
        let step = y * (STEPS - 1) / (h - 1);
        let t = step as f32 / (STEPS - 1) as f32;
        let want = (
            gfx::lerp(MIDNIGHT.0 .0, MIDNIGHT.1 .0, t),
            gfx::lerp(MIDNIGHT.0 .1, MIDNIGHT.1 .1, t),
            gfx::lerp(MIDNIGHT.0 .2, MIDNIGHT.1 .2, t),
        );
        let cell = &buf[(0, y)];
        let (Color::Indexed(fi), Color::Indexed(bi)) = (cell.fg, cell.bg) else {
            panic!("row {y}: 256 mode emitted non-indexed colors");
        };
        let cover = gfx::shade_coverage(cell.symbol())
            .unwrap_or_else(|| panic!("row {y}: unexpected glyph {:?}", cell.symbol()));
        let got = gfx::mix(gfx::palette_level(fi), gfx::palette_level(bi), cover);

        let err: i32 = [0, 1, 2]
            .iter()
            .map(|&i| {
                let (g, w) = match i {
                    0 => (got.0, want.0),
                    1 => (got.1, want.1),
                    _ => (got.2, want.2),
                };
                (g as i32 - w as i32).pow(2)
            })
            .sum();
        // Squared error across all three channels. Rounding to one palette
        // entry scored 1500-3000 here; blending keeps it under a few hundred.
        assert!(
            err < 600,
            "row {y}: wanted {want:?}, got {got:?} (err {err}) from \
             {:?} fg={fi} bg={bi}",
            cell.symbol(),
        );
    }
}

// ── image viewer ─────────────────────────────────────────────────────────────

/// A real PNG opened through the real app. The pattern mixes flat color bars,
/// a smooth ramp, hard edges and a diagonal, so the quadrant renderer's
/// symbol/fg/bg choices all show up in the glyph grid.
#[test]
fn image_viewer() {
    let path = test_image_file("pattern.png", 240, 160);
    let mut app = ImageViewer::new(path);
    let buf = render_mode(true, 100, 30, |f| {
        let area = f.area();
        app.render(f, area, true);
    });
    assert_snapshot("image_viewer", &buf, 1.0, 1.0);
}

/// The same image as a 256-color terminal receives it.
#[test]
fn image_viewer_256() {
    let path = test_image_file("pattern256.png", 240, 160);
    let mut app = ImageViewer::new(path);
    let buf = render_mode(false, 100, 30, |f| {
        let area = f.area();
        app.render(f, area, true);
    });
    assert_snapshot("image_viewer_256", &buf, 1.0, 1.0);
}

// ── browser page area ────────────────────────────────────────────────────────

/// The browser's page area.
///
/// The live engine cannot run here — WebKit funnels its work through the
/// process main thread's run loop, which `main` dedicates a thread to and a
/// test harness does not have — so this drives the same composition the
/// browser performs (`gfx::fill` then `gfx::draw_rgb_cached`) over a frame
/// standing in for the engine's output. That is where page rendering actually
/// happens; the engine side is covered by
/// `vendor/webkit/examples/98_texttop_verify.rs`.
fn draw_page(w: u16, h: u16, truecolor: bool) -> ratatui::buffer::Buffer {
    let (iw, ih) = (w as u32 * 2, h as u32 * 2);
    let px = test_pattern(iw, ih);
    let mut cache = gfx::RgbCache::default();
    render_mode(truecolor, w, h, |f| {
        let area = f.area();
        let content = Rect::new(area.x, area.y + 1, area.width, area.height - 2);
        gfx::fill(f.buffer_mut(), content, Color::Rgb(250, 250, 250));
        gfx::draw_rgb_cached(f.buffer_mut(), content, &px, iw, ih, 1, &mut cache);
    })
}

#[test]
fn browser_page() {
    let buf = draw_page(96, 28, true);
    assert_snapshot("browser_page", &buf, 1.0, 1.0);
}

#[test]
fn browser_page_256() {
    let buf = draw_page(96, 28, false);
    assert_snapshot("browser_page_256", &buf, 1.0, 1.0);
}

/// The page area must actually be painted — an unpainted grid means the frame
/// never reached the buffer, which is the failure that shows up as a blank
/// window. Most cells of a flat region are a space over a background color, so
/// "painted" means the background was set, not that a glyph was drawn.
#[test]
fn browser_page_is_not_blank() {
    let (w, h) = (96u16, 28u16);
    let buf = draw_page(w, h, true);
    let content_cells = (w as usize) * (h as usize - 2);
    let painted = (1..h - 1)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .filter(|&(x, y)| buf[(x, y)].bg != Color::Reset)
        .count();
    assert!(
        painted == content_cells,
        "page area not fully painted: {painted}/{content_cells} cells have a background",
    );

    // And it must not be a single flat color — that would mean the fill
    // survived but the frame never got drawn over it.
    let distinct = (1..h - 1)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| format!("{:?}", buf[(x, y)].bg))
        .collect::<std::collections::HashSet<_>>();
    assert!(
        distinct.len() > 8,
        "page area is nearly flat: only {} distinct backgrounds",
        distinct.len(),
    );
}
