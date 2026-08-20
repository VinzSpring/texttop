//! Reproduces the browser's scrolling path: set a viewport, load a long page,
//! run the exact JS the app runs, and check both the reported scroll position
//! and whether the *snapshot pixels* actually moved.

use webkit::prelude::*;

fn js(view: &WebView, src: &str) -> String {
    view.evaluate_javascript(src)
        .unwrap_or_else(|e| format!("<err {e}>"))
        .trim()
        .trim_matches('"')
        .to_string()
}

/// Mean of the first row of pixels — changes when the page moves.
fn top_row(rgba: &[u8], w: u32) -> u64 {
    rgba[..(w * 4) as usize].iter().map(|&b| b as u64).sum()
}

fn main() {
    webkit::init_app();
    let view = WebView::new_offscreen().expect("webview");
    view.set_frame_size(900.0, 600.0);

    // A page much taller than the viewport, with a distinct band per screen.
    let mut body = String::from(
        "<html><head><style>html,body{margin:0}\
         div{height:120px;font:48px monospace}</style></head><body>",
    );
    for i in 0..60 {
        let shade = 40 + (i * 3) % 200;
        body.push_str(&format!(
            "<div style='background:rgb({shade},{shade},{shade})'>row {i}</div>"
        ));
    }
    body.push_str("</body></html>");
    view.load_html(&body, None).expect("load");
    webkit::pump_run_loop(0.6);

    println!("innerHeight     = {}", js(&view, "window.innerHeight"));
    println!("scrollHeight    = {}", js(&view, "document.documentElement.scrollHeight"));
    println!("scrollingElement= {}", js(&view, "document.scrollingElement?document.scrollingElement.tagName:'none'"));
    println!();

    let (before, w, _h) = view.take_snapshot_rgba(Some(900)).expect("snap");
    let before_row = top_row(&before, w);
    println!("initial scrollY = {}", js(&view, "window.scrollY"));

    // Exactly what WebBrowser::scroll_by sends.
    println!("--- window.scrollBy(0,120) ---");
    let _ = view.evaluate_javascript("window.scrollBy(0,120);");
    webkit::pump_run_loop(0.4);
    println!("scrollY         = {}", js(&view, "window.scrollY"));
    println!("documentElement.scrollTop = {}", js(&view, "document.documentElement.scrollTop"));
    println!("body.scrollTop  = {}", js(&view, "document.body.scrollTop"));

    let (after, _, _) = view.take_snapshot_rgba(Some(900)).expect("snap");
    let after_row = top_row(&after, w);
    println!(
        "top row of snapshot {}",
        if after_row == before_row {
            "UNCHANGED  <-- snapshot did not follow the scroll"
        } else {
            "changed (snapshot followed the scroll)"
        }
    );
    println!();

    // Alternatives, in case the window is not the scrolling box.
    for (label, src) in [
        ("scrollTo(0,400)", "window.scrollTo(0,400);"),
        (
            "documentElement.scrollTop=400",
            "document.documentElement.scrollTop=400;",
        ),
        (
            "scrollingElement.scrollTop=400",
            "document.scrollingElement.scrollTop=400;",
        ),
    ] {
        let _ = view.evaluate_javascript("window.scrollTo(0,0);");
        webkit::pump_run_loop(0.2);
        let _ = view.evaluate_javascript(src);
        webkit::pump_run_loop(0.3);
        let y = js(&view, "window.scrollY");
        let (snap, _, _) = view.take_snapshot_rgba(Some(900)).expect("snap");
        let moved = top_row(&snap, w) != before_row;
        println!("{label:32} -> scrollY={y:<6} snapshot moved={moved}");
    }
}
