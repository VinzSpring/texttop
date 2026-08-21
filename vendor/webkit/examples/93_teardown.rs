//! Closing a browser tab must stop its audio. Reproduces the engine's
//! shutdown path and checks the page really is torn down.

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

fn media(view: &WebView) -> String {
    js(
        view,
        "(function(){var v=document.querySelector('video,audio');if(!v)return 'no media element';\
         return 'paused='+v.paused+' muted='+v.muted+' t='+v.currentTime.toFixed(2);})()",
    )
}

fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://www.youtube.com/watch?v=aqz-KE-bpKQ".into());
    webkit::init_app();

    let config = WebViewConfiguration::new();
    config.set_media_types_requiring_user_action_for_playback(AudiovisualMediaTypes::NONE);
    let view = WebView::with_config(&config).expect("webview");
    view.set_frame_size(1000.0, 700.0);
    view.attach_window(1000.0, 700.0, 1, 0.0);
    view.load_url(&url).expect("load");
    settle(14.0);

    println!("while open   : {}", media(&view));
    let t1 = js(&view, "(function(){var v=document.querySelector('video');return v?v.currentTime:-1;})()");
    settle(2.0);
    let t2 = js(&view, "(function(){var v=document.querySelector('video');return v?v.currentTime:-1;})()");
    println!("advancing    : {t1} -> {t2}");

    // TEARDOWN_SKIP=1 drops the handle without detaching, reproducing the
    // leak: NSWindow keeps the view alive and the audio carries on.
    if std::env::var("TEARDOWN_SKIP").is_ok() {
        println!("\n--- dropping handle WITHOUT detaching ---");
        drop(view);
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(8) {
            webkit::pump_run_loop(0.02);
        }
        println!("still running; check system audio assertions");
        return;
    }

    // Exactly what engine/mac.rs `shutdown` does.
    println!("\n--- shutdown ---");
    let _ = view.evaluate_javascript(
        "(function(){try{document.querySelectorAll('video,audio')\
         .forEach(function(m){m.pause();m.muted=true;m.removeAttribute('src');m.load();});\
         }catch(e){}})()",
    );
    let _ = view.load_html("", None);
    view.detach_window();
    settle(2.0);

    println!("after teardown: {}", media(&view));
    println!("has window    : {}", js(&view, "String(!!document.querySelector('video,audio'))"));

    drop(view);
    // Hold open so system audio assertions can be sampled from outside.
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(8) {
        webkit::pump_run_loop(0.02);
    }
    println!("dropped view without crashing");
}
