//! Verifies the invariants src/engine/mac.rs relies on:
//!   1. set_frame_size drives the layout viewport (innerWidth/innerHeight)
//!   2. take_snapshot_rgba returns tightly packed w*h*4 RGBA at the requested width
//!   3. Row 0 of the buffer is the TOP of the page (top-down orientation)
//!   4. 1 frame pixel == 1 CSS pixel at zoom 1.0 (mouse + text-overlay mapping)
//!   5. At zoom Z, frame pixel p maps to CSS pixel p/Z

use webkit::prelude::*;

fn px(rgba: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8) {
    let i = ((y * w + x) * 4) as usize;
    (rgba[i], rgba[i + 1], rgba[i + 2])
}

fn near(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    let d = |x: u8, y: u8| (x as i32 - y as i32).abs();
    d(a.0, b.0) <= 24 && d(a.1, b.1) <= 24 && d(a.2, b.2) <= 24
}

fn check(name: &str, ok: bool, detail: String) {
    println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
}

const RED: (u8, u8, u8) = (255, 0, 0);
const BLUE: (u8, u8, u8) = (0, 0, 255);
const GREEN: (u8, u8, u8) = (0, 128, 0);

fn main() {
    webkit::init_app();
    let view = WebView::new_offscreen().expect("webview");

    // Top half red, bottom half blue, plus a green 40x40 marker whose top-left
    // sits at CSS (100, 60).
    view.load_html(
        "<html><head><style>\
         html,body{margin:0;padding:0}\
         #t{position:absolute;left:0;top:0;width:100%;height:50%;background:#f00}\
         #b{position:absolute;left:0;top:50%;width:100%;height:50%;background:#00f}\
         #m{position:absolute;left:100px;top:60px;width:40px;height:40px;background:#008000}\
         </style></head><body><div id=t></div><div id=b></div><div id=m></div></body></html>",
        None,
    )
    .expect("load");
    webkit::pump_run_loop(0.5);

    // ---- 1 & 2: viewport parity and packing -------------------------------
    for (w, h) in [(800u32, 600u32), (1696, 960), (400, 300)] {
        view.set_frame_size(w as f64, h as f64);
        webkit::pump_run_loop(0.3);
        let (rgba, pw, ph) = view.take_snapshot_rgba(Some(w)).expect("snapshot");
        let inner = view
            .evaluate_javascript("window.innerWidth + 'x' + window.innerHeight")
            .unwrap_or_default();
        let inner = inner.trim().trim_matches('"').to_string();

        check(
            &format!("viewport {w}x{h}"),
            inner == format!("{w}x{h}"),
            format!("innerSize={inner}"),
        );
        check(
            &format!("snapshot dims {w}x{h}"),
            pw == w && ph == h,
            format!("got {pw}x{ph}"),
        );
        check(
            &format!("tight packing {w}x{h}"),
            rgba.len() == (pw * ph * 4) as usize,
            format!("len={} expected={}", rgba.len(), pw * ph * 4),
        );

        // ---- 3: orientation ------------------------------------------------
        let top = px(&rgba, pw, pw / 2, 2);
        let bot = px(&rgba, pw, pw / 2, ph - 3);
        check(
            &format!("top-down orientation {w}x{h}"),
            near(top, RED) && near(bot, BLUE),
            format!("row2={top:?} (want red) row{}={bot:?} (want blue)", ph - 3),
        );
    }

    // ---- 4: 1 frame px == 1 CSS px at zoom 1 ------------------------------
    view.set_page_zoom(1.0);
    view.set_frame_size(800.0, 600.0);
    webkit::pump_run_loop(0.3);
    let (rgba, pw, _ph) = view.take_snapshot_rgba(Some(800)).expect("snapshot");
    // Marker occupies CSS x 100..140, y 60..100 -> sample its center (120, 80).
    let center = px(&rgba, pw, 120, 80);
    // And just outside it (95, 80) should still be the red top half.
    let outside = px(&rgba, pw, 95, 80);
    check(
        "zoom 1.0 marker at CSS (120,80)",
        near(center, GREEN) && near(outside, RED),
        format!("center={center:?} (want green) outside={outside:?} (want red)"),
    );

    // ---- 5: at zoom Z, frame px == CSS px * Z -----------------------------
    for z in [1.5f64, 0.5] {
        view.set_page_zoom(z);
        webkit::pump_run_loop(0.4);
        let (rgba, pw, ph) = view.take_snapshot_rgba(Some(800)).expect("snapshot");
        // CSS (120,80) should now land at frame (120*z, 80*z).
        let (fx, fy) = ((120.0 * z) as u32, (80.0 * z) as u32);
        if fx >= pw || fy >= ph {
            check(&format!("zoom {z} in bounds"), false, format!("{fx},{fy}"));
        }
        let got = px(&rgba, pw, fx, fy);
        check(
            &format!("zoom {z} marker at frame ({fx},{fy})"),
            near(got, GREEN),
            format!("got={got:?} (want green)"),
        );

        // Inverse: mac.rs converts frame px -> CSS px by dividing by zoom, then
        // hands that to elementFromPoint. Confirm it hits the marker.
        let cx = (fx as f64 / z).round() as i32;
        let cy = (fy as f64 / z).round() as i32;
        let hit = view
            .evaluate_javascript(&format!(
                "(document.elementFromPoint({cx},{cy})||{{id:'none'}}).id"
            ))
            .unwrap_or_default();
        let hit = hit.trim().trim_matches('"').to_string();
        check(
            &format!("zoom {z} elementFromPoint({cx},{cy})"),
            hit == "m",
            format!("hit id={hit:?} (want \"m\")"),
        );
    }

    println!("\nall checks passed");
}
