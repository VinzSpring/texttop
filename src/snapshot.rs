//! End-to-end rendering snapshots.
//!
//! Each test renders a real screen into an off-screen terminal, serializes it
//! to a text "screenshot" (a glyph grid plus a color grid and legend), and
//! compares that against a committed golden file in `tests/golden/`.
//!
//! Comparison is *similarity gated*: each test states the fraction of cells
//! that must match, and a failure reports both ratios plus the first differing
//! rows of the grid, so a drift is diagnosable without opening the golden.
//!
//! Volatile content — the clock, the git branch — is **masked** rather than
//! absorbed by a loose threshold. That distinction matters: real regressions
//! can be small. Widening the desktop's icon column moves only about 1% of a
//! 150x45 grid, so a gate slack enough to tolerate a ticking clock would sleep
//! through it. Masking the clock instead lets the gates sit at 0.999.
//!
//! Refresh goldens after an intentional change with:
//!
//! ```sh
//! UPDATE_SNAPSHOTS=1 cargo test --bin texttop
//! ```
//!
//! Review the resulting diff — the glyph grid is meant to be read.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::{Frame, Terminal};

use crate::gfx;

// ── environment ──────────────────────────────────────────────────────────────

static HOME: OnceLock<PathBuf> = OnceLock::new();

/// Pin everything ambient that rendering depends on.
///
/// `gfx::truecolor()` caches its answer, so the color mode has to be settled
/// before the first render or the goldens would depend on which terminal
/// happened to run `cargo test`. `TEXTTOP_HOME` points the desktop at an empty
/// profile so it finds no saved session or icon layout and comes up in its
/// default state.
///
/// Setting `HOME` is *not* enough: `dirs::home_dir` resolves the account's
/// home directly on macOS and ignores the variable, so these tests read the
/// developer's real `~/.texttop_desktop.json` — and started failing the moment
/// someone changed their wallpaper theme in the running app.
pub fn init_env() -> PathBuf {
    HOME.get_or_init(|| {
        let home = std::env::temp_dir().join("texttop-snapshot-home");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("create fake HOME");
        std::env::set_var("TEXTTOP_HOME", &home);
        std::env::set_var("HOME", &home);
        // Force the 24-bit path; the 256-color path is exercised explicitly by
        // passing `false` to the `*_for` helpers rather than by ambient state.
        std::env::set_var("TERM_PROGRAM", "iTerm.app");
        std::env::set_var("COLORTERM", "truecolor");
        home
    })
    .clone()
}

// ── rendering ────────────────────────────────────────────────────────────────

/// Serializes access to the process-wide color mode, which `render_mode`
/// swaps. Without this, tests running in parallel would see each other's mode.
static MODE_LOCK: Mutex<()> = Mutex::new(());

/// Render exactly the way `main` does for a terminal of the given capability:
/// draw with the color mode pinned, then fold the finished frame down to the
/// 256 palette.
///
/// Pinning the mode for the whole draw matters — apps consult it while
/// drawing (the wallpaper blends palette entries when 24-bit is unavailable),
/// so rendering in 24-bit and converting afterwards would exercise a path the
/// real 256-color terminal never takes.
pub fn render_mode(truecolor: bool, w: u16, h: u16, draw: impl FnOnce(&mut Frame)) -> Buffer {
    init_env();
    let _guard = MODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    gfx::force_truecolor(truecolor);

    let mut term = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
    term.draw(|f| {
        draw(f);
        let area = f.area();
        gfx::adapt_buffer(f.buffer_mut(), area, truecolor);
    })
    .expect("draw");
    term.backend().buffer().clone()
}

// ── serialization ────────────────────────────────────────────────────────────

fn color_key(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => format!("@{i}"),
        other => format!("{other:?}"),
    }
}

/// Legend symbols, in a fixed order so a given palette always serializes the
/// same way. Ordering is by color value, never by frequency — frequency would
/// reshuffle the whole grid over a one-cell change.
const LEGEND: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+/=<>[]{}()!$%&*";

/// Placeholder written for masked cells, so volatile content never registers
/// as a difference.
const MASKED: char = '~';

fn is_masked(mask: &[Rect], x: u16, y: u16) -> bool {
    mask.iter().any(|r| {
        x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
    })
}

/// A frame rendered as text: the glyph grid, a per-cell color grid, and the
/// legend mapping grid symbols back to concrete colors.
///
/// `mask` lists rectangles whose contents are replaced by a placeholder. Use
/// it for regions that legitimately change between runs — a clock, a branch
/// name — so the rest of the frame can be gated tightly instead of loosening
/// the threshold for the whole grid.
pub fn serialize_masked(buf: &Buffer, mask: &[Rect]) -> String {
    let area = buf.area();

    let mut pairs: Vec<(String, String)> = Vec::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if is_masked(mask, x, y) {
                continue;
            }
            let c = &buf[(x, y)];
            let p = (color_key(c.fg), color_key(c.bg));
            if !pairs.contains(&p) {
                pairs.push(p);
            }
        }
    }
    pairs.sort();

    let mut out = String::new();
    out.push_str(&format!("size: {}x{}\n", area.width, area.height));

    out.push_str("--- chars ---\n");
    for y in 0..area.height {
        for x in 0..area.width {
            if is_masked(mask, x, y) {
                out.push(MASKED);
                continue;
            }
            let s = buf[(x, y)].symbol();
            // Keep the grid one column per cell so it stays readable and
            // diffable; empty (zero-width) symbols show as a space.
            out.push(s.chars().next().unwrap_or(' '));
        }
        out.push('\n');
    }

    out.push_str("--- colors ---\n");
    for y in 0..area.height {
        for x in 0..area.width {
            if is_masked(mask, x, y) {
                out.push(MASKED);
                continue;
            }
            let c = &buf[(x, y)];
            let p = (color_key(c.fg), color_key(c.bg));
            let i = pairs.iter().position(|q| *q == p).unwrap_or(usize::MAX);
            out.push(LEGEND.get(i).map(|b| *b as char).unwrap_or('?'));
        }
        out.push('\n');
    }

    out.push_str("--- legend ---\n");
    for (i, (fg, bg)) in pairs.iter().enumerate() {
        let sym = LEGEND.get(i).map(|b| *b as char).unwrap_or('?');
        out.push_str(&format!("{sym} fg={fg} bg={bg}\n"));
    }
    out
}

fn section<'a>(s: &'a str, name: &str) -> Vec<&'a str> {
    let head = format!("--- {name} ---");
    let mut out = Vec::new();
    let mut in_sec = false;
    for line in s.lines() {
        if line.starts_with("--- ") {
            in_sec = line == head;
            continue;
        }
        if in_sec {
            out.push(line);
        }
    }
    out
}

/// Fraction of cells that match, comparing grids position by position.
/// Differing dimensions score 0 — a resize is never a near miss.
fn grid_similarity(a: &[&str], b: &[&str]) -> f64 {
    if a.len() != b.len() {
        return 0.0;
    }
    let (mut same, mut total) = (0usize, 0usize);
    for (ra, rb) in a.iter().zip(b) {
        let (ca, cb): (Vec<char>, Vec<char>) = (ra.chars().collect(), rb.chars().collect());
        if ca.len() != cb.len() {
            return 0.0;
        }
        for (x, y) in ca.iter().zip(&cb) {
            total += 1;
            if x == y {
                same += 1;
            }
        }
    }
    if total == 0 {
        return 1.0;
    }
    same as f64 / total as f64
}

/// Rows that differ, as `(row index, expected, actual)`, capped for legibility.
fn diff_rows<'a>(exp: &[&'a str], act: &[&'a str], max: usize) -> Vec<(usize, &'a str, &'a str)> {
    exp.iter()
        .zip(act)
        .enumerate()
        .filter(|(_, (e, a))| e != a)
        .map(|(i, (e, a))| (i, *e, *a))
        .take(max)
        .collect()
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.snap"))
}

/// Compare a rendered frame against its golden file.
///
/// `min_chars` / `min_colors` are the fractions of cells that must match.
/// Writes the golden and passes when it does not exist yet, or when
/// `UPDATE_SNAPSHOTS=1` is set.
pub fn assert_snapshot(name: &str, buf: &Buffer, min_chars: f64, min_colors: f64) {
    assert_snapshot_masked(name, buf, &[], min_chars, min_colors)
}

/// [`assert_snapshot`], ignoring the given rectangles. See [`serialize_masked`].
pub fn assert_snapshot_masked(
    name: &str,
    buf: &Buffer,
    mask: &[Rect],
    min_chars: f64,
    min_colors: f64,
) {
    let actual = serialize_masked(buf, mask);
    let path = golden_path(name);
    let updating = std::env::var("UPDATE_SNAPSHOTS").is_ok();

    if updating || !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("create golden dir");
        std::fs::write(&path, &actual).expect("write golden");
        if !updating {
            eprintln!("snapshot {name}: created {}", path.display());
        }
        return;
    }

    let expected = std::fs::read_to_string(&path).expect("read golden");
    let (exp_c, act_c) = (section(&expected, "chars"), section(&actual, "chars"));
    let (exp_k, act_k) = (section(&expected, "colors"), section(&actual, "colors"));
    let chars = grid_similarity(&exp_c, &act_c);
    let colors = grid_similarity(&exp_k, &act_k);

    if chars >= min_chars && colors >= min_colors {
        return;
    }

    let mut msg = format!(
        "snapshot {name} drifted\n  chars  {chars:.4} (need {min_chars:.4})\n  \
         colors {colors:.4} (need {min_colors:.4})\n\
         re-run with UPDATE_SNAPSHOTS=1 to accept\n",
    );
    if chars < min_chars {
        msg.push_str("\nfirst differing glyph rows (expected / actual):\n");
        for (i, e, a) in diff_rows(&exp_c, &act_c, 6) {
            msg.push_str(&format!("{i:3} exp |{e}|\n{i:3} act |{a}|\n"));
        }
    }
    if colors < min_colors {
        msg.push_str("\nfirst differing color rows (expected / actual):\n");
        for (i, e, a) in diff_rows(&exp_k, &act_k, 6) {
            msg.push_str(&format!("{i:3} exp |{e}|\n{i:3} act |{a}|\n"));
        }
        msg.push_str("\nlegend differences:\n");
        let (le, la) = (section(&expected, "legend"), section(&actual, "legend"));
        for l in le.iter().filter(|l| !la.contains(l)).take(10) {
            msg.push_str(&format!("  -{l}\n"));
        }
        for l in la.iter().filter(|l| !le.contains(l)).take(10) {
            msg.push_str(&format!("  +{l}\n"));
        }
    }
    panic!("{msg}");
}

// ── fixtures ─────────────────────────────────────────────────────────────────

/// A deterministic RGB24 test pattern: color bars over a vertical ramp, with a
/// one-pixel border and a diagonal. Exercises flat fills, gradients, hard
/// edges, and sub-cell detail — the four things the quadrant renderer can get
/// wrong.
pub fn test_pattern(w: u32, h: u32) -> Vec<u8> {
    const BARS: [(u8, u8, u8); 8] = [
        (255, 255, 255),
        (255, 255, 0),
        (0, 255, 255),
        (0, 255, 0),
        (255, 0, 255),
        (255, 0, 0),
        (0, 0, 255),
        (0, 0, 0),
    ];
    let mut px = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let c = if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                (255, 128, 0)
            } else if x * h == y * w {
                (0, 0, 0)
            } else if y < h / 2 {
                BARS[(x * BARS.len() as u32 / w) as usize]
            } else {
                let t = ((y - h / 2) * 255 / (h / 2).max(1)) as u8;
                (t, 40, 255 - t)
            };
            px.extend_from_slice(&[c.0, c.1, c.2]);
        }
    }
    px
}

/// Write the test pattern out as a PNG so file-backed apps can open it.
pub fn test_image_file(name: &str, w: u32, h: u32) -> PathBuf {
    let home = init_env();
    let path = home.join(name);
    let px = test_pattern(w, h);
    let img: image::RgbImage = image::ImageBuffer::from_raw(w, h, px).expect("image buffer");
    img.save(&path).expect("write test png");
    path
}
