//! Diagnoses why a real site renders blank / unscrollable in the TUI browser.
//! Loads a URL at the app's viewport and reports document metrics.

use std::time::{Duration, Instant};
use webkit::prelude::*;

/// Make an off-screen WKWebView look "visible" to the Page Visibility API.
const VISIBILITY_SHIM: &str = r#"(function(){
 var d=Document.prototype;
 for (var p of ['visibilityState','webkitVisibilityState'])
   Object.defineProperty(d,p,{get:function(){return 'visible';},configurable:true});
 for (var p2 of ['hidden','webkitHidden'])
   Object.defineProperty(d,p2,{get:function(){return false;},configurable:true});
 d.hasFocus=function(){return true;};
 window.addEventListener('DOMContentLoaded',function(){
   document.dispatchEvent(new Event('visibilitychange'));
 });
})()"#;

fn js(view: &WebView, src: &str) -> String {
    view.evaluate_javascript(src)
        .unwrap_or_else(|e| format!("<err {e}>"))
        .trim()
        .trim_matches('"')
        .to_string()
}

fn settle(view: &WebView, secs: f64) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs_f64(secs) {
        webkit::pump_run_loop(0.05);
    }
}

fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://www.youtube.com/".into());
    // Matches the app at 214x67: content cells * CELL_PX per cell.
    let (w, h) = (
        std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(1712.0),
        std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(1170.0),
    );

    webkit::init_app();
    let config = WebViewConfiguration::new();
    if std::env::var("DIAG_VISIBLE").is_ok() {
        config.add_user_script(
            &UserScript::new(VISIBILITY_SHIM)
                .with_injection_time(InjectionTime::AtDocumentStart),
        );
        println!("visibility shim installed");
    }
    let view = WebView::with_config(&config).expect("webview");
    view.set_frame_size(w, h);
    if let Ok(ua) = std::env::var("DIAG_UA") {
        view.set_custom_user_agent(Some(&ua));
        println!("custom UA set");
    }
    println!("loading {url} at {w}x{h}\n");
    view.load_url(&url).expect("load");
    settle(&view, std::env::var("DIAG_WAIT").ok().and_then(|s| s.parse().ok()).unwrap_or(8.0));

    println!("userAgent        = {}", js(&view, "navigator.userAgent"));
    println!("title            = {}", view.title());
    println!("url              = {}", view.url());
    println!("readyState       = {}", js(&view, "document.readyState"));
    println!("visibilityState  = {}", js(&view, "document.visibilityState"));
    println!("document.hidden  = {}", js(&view, "document.hidden"));
    println!("hasFocus()       = {}", js(&view, "document.hasFocus()"));
    println!("body clientHeight= {}", js(&view, "document.body?document.body.clientHeight:-1"));
    println!("body children    = {}", js(&view, "document.body?document.body.children.length:-1"));
    println!("body display     = {}", js(&view, "document.body?getComputedStyle(document.body).display:'-'"));
    println!("innerWidth x H   = {} x {}", js(&view, "innerWidth"), js(&view, "innerHeight"));
    println!("scrollHeight     = {}", js(&view, "document.documentElement.scrollHeight"));
    println!("body.scrollHeight= {}", js(&view, "document.body?document.body.scrollHeight:-1"));
    println!(
        "scrollable       = {}",
        js(&view, "document.documentElement.scrollHeight > innerHeight + 4")
    );
    println!("elements         = {}", js(&view, "document.querySelectorAll('*').length"));
    println!("images           = {}", js(&view, "document.images.length"));
    println!(
        "imgs w/ real src = {}",
        js(&view, "[...document.images].filter(i=>i.currentSrc&&i.naturalWidth>0).length")
    );
    println!(
        "visible text len = {}",
        js(&view, "(document.body?document.body.innerText:'').trim().length")
    );

    // Does anything actually paint below the fold?
    println!(
        "\nIntersectionObserver present = {}",
        js(&view, "typeof IntersectionObserver")
    );
    println!(
        "elements below fold          = {}",
        js(
            &view,
            "[...document.querySelectorAll('*')].filter(e=>{var r=e.getBoundingClientRect();\
             return r.top>innerHeight&&r.height>0}).length"
        )
    );

    // Site-specific: YouTube's grid renderer.
    for sel in [
        "ytd-rich-grid-renderer",
        "ytd-rich-item-renderer",
        "ytd-video-renderer",
        "#contents",
    ] {
        println!(
            "{sel:28} count={} ",
            js(&view, &format!("document.querySelectorAll('{sel}').length"))
        );
    }

    println!("\nfinal url = {}", view.url());
    println!("title     = {}", view.title());
    println!("h1/h2     = {}", js(&view, "[...document.querySelectorAll('h1,h2')].map(e=>e.textContent.trim()).slice(0,5).join(' | ')"));
    println!("buttons   = {}", js(&view, "[...document.querySelectorAll('button,a[role=button]')].map(e=>e.textContent.trim()).filter(t=>t).slice(0,8).join(' | ')"));
    println!("text head = {}", js(&view, "(document.body.innerText||document.body.textContent||'').replace(/\\s+/g,' ').trim().slice(0,300)"));
    // Decisive check: what do the pixels actually look like? A page that
    // never laid out comes back essentially blank.
    let (rgba, sw, sh) = view.take_snapshot_rgba(Some(1200)).expect("snap");
    let nonwhite = rgba
        .chunks_exact(4)
        .filter(|px| px[0] < 240 || px[1] < 240 || px[2] < 240)
        .count();
    let total = (sw * sh) as usize;
    println!(
        "snapshot {sw}x{sh}: {nonwhite}/{total} non-white px ({:.1}%)",
        nonwhite as f64 * 100.0 / total as f64
    );

    println!("\n--- scroll test ---");
    println!("scrollY before = {}", js(&view, "window.scrollY"));
    let _ = view.evaluate_javascript("window.scrollBy(0,600);");
    settle(&view, 1.5);
    println!("scrollY after  = {}", js(&view, "window.scrollY"));
    settle(&view, 2.0);
    println!(
        "scrollHeight after scroll = {}",
        js(&view, "document.documentElement.scrollHeight")
    );
}
