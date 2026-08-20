//! Off-screen WKWebView engine for macOS. Uses the `webkit` crate's headless
//! WebKit bindings on a dedicated thread; raw RGBA snapshots (scaled to the
//! logical viewport width) are converted into RGB frames for the TUI.
//!
//! The WKWebView frame tracks the TUI window size (Cmd::ResizePx), matching
//! the Windows/Linux engines: pages lay out at the real viewport size and
//! mouse coordinates map 1:1 (divided by page zoom).

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use webkit::prelude::*;

use super::{
    clear_page_state, mouse_sim_js, parse_text_runs, Cmd, Frame, PageInfo, Status, TextRun,
    EXTRACT_JS, REMOVE_STYLE_JS, VISIBILITY_JS,
};

pub struct Engine {
    tx: Sender<Cmd>,
    frame: Arc<Mutex<Option<Arc<Frame>>>>,
    info: Arc<Mutex<PageInfo>>,
    status: Arc<Mutex<Status>>,
    texts: Arc<Mutex<Vec<TextRun>>>,
}

impl Engine {
    pub fn start(initial: Cmd, zoom: f64, w: i32, h: i32) -> Engine {
        let (tx, rx) = mpsc::channel();
        let frame: Arc<Mutex<Option<Arc<Frame>>>> = Arc::new(Mutex::new(None));
        let info = Arc::new(Mutex::new(PageInfo::default()));
        let status = Arc::new(Mutex::new(Status::Starting));
        let texts: Arc<Mutex<Vec<TextRun>>> = Arc::new(Mutex::new(Vec::new()));
        {
            let frame = frame.clone();
            let info = info.clone();
            let status = status.clone();
            let texts = texts.clone();
            std::thread::spawn(move || {
                run(rx, frame, info, status, texts, initial, zoom, (w, h))
            });
        }
        Engine {
            tx,
            frame,
            info,
            status,
            texts,
        }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn frame(&self) -> Option<Arc<Frame>> {
        self.frame.lock().unwrap().clone()
    }

    pub fn info(&self) -> PageInfo {
        self.info.lock().unwrap().clone()
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn texts(&self) -> Vec<TextRun> {
        self.texts.lock().unwrap().clone()
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    rx: Receiver<Cmd>,
    frame: Arc<Mutex<Option<Arc<Frame>>>>,
    info: Arc<Mutex<PageInfo>>,
    status: Arc<Mutex<Status>>,
    texts: Arc<Mutex<Vec<TextRun>>>,
    initial: Cmd,
    zoom: f64,
    size0: (i32, i32),
) {
    webkit::init_app();

    let config = WebViewConfiguration::new();
    // Allow video/audio autoplay without a prior gesture (video player).
    config.set_media_types_requiring_user_action_for_playback(AudiovisualMediaTypes::NONE);
    // The view is off-screen, so pages would otherwise see themselves as a
    // hidden tab and skip laying out. See VISIBILITY_JS.
    config.add_user_script(
        &UserScript::new(VISIBILITY_JS).with_injection_time(InjectionTime::AtDocumentStart),
    );

    let view = match WebView::with_config(&config) {
        Ok(v) => v,
        Err(e) => {
            *status.lock().unwrap() = Status::Error(format!(
                "cannot create WKWebView ({e}). Install Xcode Command Line Tools \
                 (`xcode-select --install`) and rebuild on macOS 13+."
            ));
            return;
        }
    };

    let mut page_zoom = zoom.max(0.01);
    view.set_page_zoom(page_zoom);
    // Snapshots are scaled to the logical width: keeps frames cheap and —
    // crucially — matches the Windows/Linux engines, where 1 frame pixel ==
    // 1 viewport point (the text overlay's mapping assumes this).
    let mut snap_w = size0.0.max(1) as u32;
    view.set_frame_size(size0.0.max(1) as f64, size0.1.max(1) as f64);
    let mut overlay = false;

    if let Err(e) = apply_nav(&view, &initial) {
        *status.lock().unwrap() = Status::Error(format!("initial navigation failed: {e}"));
        return;
    }
    *status.lock().unwrap() = Status::Ready;

    let mut last_cap = Instant::now() - Duration::from_secs(1);
    let mut last_extract = Instant::now() - Duration::from_secs(1);
    let mut last_info = Instant::now() - Duration::from_secs(1);

    loop {
        loop {
            match rx.try_recv() {
                Ok(cmd) => {
                    handle_cmd(
                        cmd,
                        &view,
                        &mut overlay,
                        &mut page_zoom,
                        &mut snap_w,
                        &frame,
                        &texts,
                    );
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        // In-page navigations (link clicks) don't go through Cmd::Navigate.
        // Throttled: url()/title() are main-thread round-trips.
        if last_info.elapsed() >= Duration::from_millis(200) {
            last_info = Instant::now();
            let url = view.url();
            let title = view.title();
            let mut i = info.lock().unwrap();
            if !url.is_empty() && !i.url.is_empty() && i.url != url {
                clear_page_state(&frame, &texts);
            }
            if !url.is_empty() {
                i.url = url;
            }
            i.title = title;
        }

        if overlay && last_extract.elapsed() >= Duration::from_millis(300) {
            last_extract = Instant::now();
            match view.evaluate_javascript(EXTRACT_JS) {
                Ok(json) => {
                    let payload = js_string_payload(&json);
                    *texts.lock().unwrap() = parse_text_runs(&payload);
                }
                Err(e) => log(&format!("extract err: {e}")),
            }
        }

        if last_cap.elapsed() >= Duration::from_millis(33) {
            last_cap = Instant::now();
            let gen = super::page_gen();
            match view.take_snapshot_rgba(Some(snap_w)) {
                Ok((rgba, w, h)) if super::page_gen() == gen => {
                    if let Some(f) = rgba_to_frame(&rgba, w, h) {
                        *frame.lock().unwrap() = Some(Arc::new(f));
                    }
                }
                Ok(_) => {} // navigated away while snapshotting
                Err(e) => log(&format!("snap err: {e}")),
            }
        }

        // The process main thread pumps the CFRunLoop (see main.rs), so this
        // thread only needs to pace itself between polls.
        std::thread::sleep(Duration::from_millis(4));
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_cmd(
    cmd: Cmd,
    view: &WebView,
    overlay: &mut bool,
    page_zoom: &mut f64,
    snap_w: &mut u32,
    frame: &Arc<Mutex<Option<Arc<Frame>>>>,
    texts: &Arc<Mutex<Vec<TextRun>>>,
) {
    match cmd {
        Cmd::TextOverlay(on) => {
            *overlay = on;
            if !on {
                texts.lock().unwrap().clear();
                let _ = view.evaluate_javascript(REMOVE_STYLE_JS);
            }
        }
        Cmd::Navigate(u) => {
            clear_page_state(frame, texts);
            if let Err(e) = view.load_url(&u) {
                log(&format!("navigate err: {e}"));
            }
        }
        Cmd::NavigateHtml(h) => {
            clear_page_state(frame, texts);
            if let Err(e) = view.load_html(&h, None) {
                log(&format!("navigate html err: {e}"));
            }
        }
        Cmd::Back => {
            clear_page_state(frame, texts);
            let _ = view.go_back();
        }
        Cmd::Forward => {
            clear_page_state(frame, texts);
            let _ = view.go_forward();
        }
        Cmd::Reload => {
            clear_page_state(frame, texts);
            let _ = view.reload();
        }
        Cmd::ResizePx(w, h) => {
            *snap_w = w.max(1) as u32;
            view.set_frame_size(w.max(1) as f64, h.max(1) as f64);
        }
        Cmd::Zoom(z) => {
            *page_zoom = z.max(0.01);
            view.set_page_zoom(*page_zoom);
        }
        Cmd::Script(s) => {
            let _ = view.evaluate_javascript(&s);
        }
        Cmd::Mouse { kind, x, y } => {
            // Viewport px → CSS px: the frame matches the TUI window, so only
            // page zoom needs undoing (same convention as the Linux engine).
            let cx = (x as f64 / *page_zoom).round() as i32;
            let cy = (y as f64 / *page_zoom).round() as i32;
            let _ = view.evaluate_javascript(&mouse_sim_js(kind, cx, cy));
        }
    }
}

fn apply_nav(view: &WebView, cmd: &Cmd) -> Result<(), WebKitError> {
    match cmd {
        Cmd::Navigate(u) => view.load_url(u),
        Cmd::NavigateHtml(h) => view.load_html(h, None),
        _ => Ok(()),
    }
}

/// `evaluateJavaScript` may return a JSON-encoded string for string results.
fn js_string_payload(s: &str) -> String {
    let t = s.trim();
    if t.starts_with('"') {
        if let Ok(inner) = serde_json::from_str::<String>(t) {
            return inner;
        }
    }
    t.to_string()
}

/// RGBA (premultiplied over an effectively opaque page) → tightly packed RGB,
/// compositing any transparency over white.
fn rgba_to_frame(rgba: &[u8], w: u32, h: u32) -> Option<Frame> {
    let px_count = (w as usize).checked_mul(h as usize)?;
    if px_count == 0 || rgba.len() < px_count * 4 {
        return None;
    }
    let mut rgb = Vec::with_capacity(px_count * 3);
    // Bound the walk to w*h: the frame must be exactly w*h*3 bytes even if the
    // snapshot buffer carries trailing slack.
    for px in rgba[..px_count * 4].chunks_exact(4) {
        let a = px[3] as u32;
        for c in 0..3 {
            rgb.push(((px[c] as u32 * a + 255 * (255 - a)) / 255) as u8);
        }
    }
    Some(Frame {
        w,
        h,
        rgb,
        seq: super::next_seq(),
    })
}

fn log(msg: &str) {
    if std::env::var_os("TEXTTOP_ENGINE_LOG").is_some() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("texttop-engine.log"))
        {
            let _ = writeln!(f, "{msg}");
        }
    }
}
