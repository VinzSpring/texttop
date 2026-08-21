//! Which window placement makes an off-screen WKWebView behave like a real
//! browser tab: visibilityState visible, rAF firing, and <video> decoding.

use std::time::{Duration, Instant};
use webkit::prelude::*;

fn settle(secs: f64) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs_f64(secs) {
        webkit::pump_run_loop(0.02);
    }
}

fn js(view: &WebView, src: &str) -> String {
    view.evaluate_javascript(src)
        .unwrap_or_else(|e| format!("<err {e}>"))
        .trim()
        .trim_matches('"')
        .to_string()
}

fn hash(rgba: &[u8]) -> u64 {
    let mut h = 1469598103934665603u64;
    for px in rgba.iter().step_by(997) {
        h ^= *px as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

fn probe(label: &str, mode: Option<(i32, f64)>, url: &str) {
    let config = WebViewConfiguration::new();
    config.set_media_types_requiring_user_action_for_playback(AudiovisualMediaTypes::NONE);
    let view = WebView::with_config(&config).expect("webview");
    view.set_frame_size(1000.0, 700.0);
    if let Some((m, a)) = mode {
        view.attach_window(1000.0, 700.0, m, a);
    }
    view.load_url(url).expect("load");
    settle(12.0);

    let _ = view.evaluate_javascript("window.__raf=0;requestAnimationFrame(function(){window.__raf=1;});");
    settle(1.5);

    // Are consecutive snapshots actually different?
    let mut hs = Vec::new();
    for _ in 0..12 {
        if let Ok((rgba, _, _)) = view.take_snapshot_rgba(Some(500)) {
            hs.push(hash(&rgba));
        }
        settle(0.15);
    }
    hs.sort_unstable();
    hs.dedup();

    println!(
        "{label:26} vis={:8} raf={:1} video={:34} distinct_frames={}",
        js(&view, "document.visibilityState"),
        js(&view, "String(window.__raf)"),
        js(
            &view,
            "(function(){var v=document.querySelector('video');if(!v)return 'none';\
             return 'paused='+v.paused+' t='+v.currentTime.toFixed(1)+' '+v.videoWidth+'x'+v.videoHeight;})()"
        ),
        hs.len(),
    );
}

fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://www.youtube.com/watch?v=aqz-KE-bpKQ".into());
    webkit::init_app();
    println!("url: {url}\n");
    probe("no window", None, &url);
    probe("mode 0 off-screen", Some((0, 1.0)), &url);
    probe("mode 1 on-screen a=0.00", Some((1, 0.0)), &url);
    probe("mode 1 on-screen a=0.01", Some((1, 0.01)), &url);
}
