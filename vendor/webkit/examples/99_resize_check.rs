//! Verify set_frame_size changes layout viewport and snapshot dimensions.

use webkit::prelude::*;

fn main() {
    webkit::init_app();
    let view = WebView::new_offscreen().expect("webview");
    view.load_html(
        "<html><body><h1>resize test</h1></body></html>",
        None,
    )
    .expect("load");

    for (w, h) in [(800.0, 600.0), (1696.0, 960.0), (400.0, 300.0)] {
        view.set_frame_size(w, h);
        webkit::pump_run_loop(0.3);
        let t = std::time::Instant::now();
        let (rgba, pw, ph) = view
            .take_snapshot_rgba(Some(w as u32))
            .expect("snapshot");
        let took = t.elapsed();
        let inner = view
            .evaluate_javascript("window.innerWidth + 'x' + window.innerHeight")
            .unwrap_or_default();
        println!(
            "frame {w}x{h} -> rgba {pw}x{ph} ({} bytes, {took:?}), innerSize {inner}",
            rgba.len()
        );
    }
}
