//! Measures snapshot throughput at different viewports, and whether a playing
//! video actually advances frames. Mirrors what the TUI browser does at 30fps.

use std::time::{Duration, Instant};
use webkit::prelude::*;

fn settle(_view: &WebView, secs: f64) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs_f64(secs) {
        webkit::pump_run_loop(0.02);
    }
}

/// Cheap fingerprint of a frame, to tell "moving" from "frozen".
fn hash(rgba: &[u8]) -> u64 {
    let mut h = 1469598103934665603u64;
    for px in rgba.iter().step_by(997) {
        h ^= *px as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

fn bench(view: &WebView, label: &str, w: f64, h: f64, snap_w: u32) {
    view.set_frame_size(w, h);
    if std::env::var("BENCH_WINDOW").is_ok() {
        view.attach_window(w, h, 1, 0.0);
    }
    settle(view, 1.0);

    let mut times = Vec::new();
    let mut hashes = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(3) {
        let t = Instant::now();
        match view.take_snapshot_rgba(Some(snap_w)) {
            Ok((rgba, _, _)) => {
                times.push(t.elapsed().as_secs_f64() * 1000.0);
                hashes.push(hash(&rgba));
            }
            Err(e) => {
                println!("{label}: snapshot error {e}");
                return;
            }
        }
        webkit::pump_run_loop(0.02);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = times.len();
    let med = times[n / 2];
    let distinct = {
        let mut v = hashes.clone();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    println!(
        "{label:22} {w:>5.0}x{h:<5.0} snap_w={snap_w:<5} \
         median {med:6.1}ms  max fps {:5.1}  frames {n:3}  distinct {distinct:3}",
        1000.0 / med,
    );
}

fn main() {
    let url = std::env::args().nth(1).unwrap_or_else(|| {
        "data:text/html,<style>body{margin:0}</style><div id=d style='width:100%;height:100vh;\
         background:linear-gradient(90deg,red,blue)'></div><script>\
         setInterval(function(){d.style.filter='hue-rotate('+(Date.now()/10%360)+'deg)';},16);\
         </script>"
            .into()
    });

    webkit::init_app();
    let config = WebViewConfiguration::new();
    config.set_media_types_requiring_user_action_for_playback(AudiovisualMediaTypes::NONE);
    config.add_user_script(
        &UserScript::new(
            "(function(){var d=Document.prototype;\
             Object.defineProperty(d,'visibilityState',{get:function(){return 'visible';},configurable:true});\
             Object.defineProperty(d,'hidden',{get:function(){return false;},configurable:true});\
             d.hasFocus=function(){return true;};})()",
        )
        .with_injection_time(InjectionTime::AtDocumentStart),
    );
    let view = WebView::with_config(&config).expect("webview");
    if std::env::var("BENCH_WINDOW").is_ok() {
        view.attach_window(1712.0, 1040.0, 1, 0.0);
        println!("window attached (on-screen, alpha 0)");
    }
    println!("url: {}\n", url);
    view.load_url(&url).expect("load");
    settle(&view, 6.0);

    let vinfo = view
        .evaluate_javascript(
            "(function(){var v=document.querySelector('video');\
             if(!v)return 'no <video>';return 'paused='+v.paused+' t='+v.currentTime.toFixed(1)+\
             ' w='+v.videoWidth+'x'+v.videoHeight;})()",
        )
        .unwrap_or_default();
    println!("video: {vinfo}\n");

    // Candidate caps, all at the 214x65 window aspect (1.646).
    for (label, w) in [
        ("cap  800", 800.0),
        ("cap  896", 896.0),
        ("cap  960", 960.0),
        ("cap 1024", 1024.0),
        ("cap 1120", 1120.0),
    ] {
        bench(&view, label, w, (w / 1.646).round(), w as u32);
    }
}
