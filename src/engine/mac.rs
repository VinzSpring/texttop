//! Off-screen WKWebView engine for macOS. Uses the `webkit` crate's headless
//! WebKit bindings on a dedicated thread; PNG snapshots are decoded into RGB
//! frames for the TUI.
//!
//! Note: the bridge creates WKWebView at a fixed 800×600. Logical ResizePx
//! sizes from the TUI are retained for mouse mapping; frames are scaled by
//! the renderer to the window.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use webkit::prelude::*;

use super::{
    clear_page_state, mouse_sim_js, parse_text_runs, Cmd, Frame, PageInfo, Status, TextRun,
    EXTRACT_JS, REMOVE_STYLE_JS,
};

/// Native off-screen WKWebView size used by the `webkit` bridge (fixed).
const NATIVE_W: f64 = 800.0;
const NATIVE_H: f64 = 600.0;

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
    let mut logical = (size0.0.max(1) as f64, size0.1.max(1) as f64);
    let mut overlay = false;

    if let Err(e) = apply_nav(&view, &initial) {
        *status.lock().unwrap() = Status::Error(format!("initial navigation failed: {e}"));
        return;
    }
    *status.lock().unwrap() = Status::Ready;

    let mut last_cap = Instant::now() - Duration::from_secs(1);
    let mut last_extract = Instant::now() - Duration::from_secs(1);

    loop {
        loop {
            match rx.try_recv() {
                Ok(cmd) => {
                    handle_cmd(
                        cmd,
                        &view,
                        &mut overlay,
                        &mut page_zoom,
                        &mut logical,
                        &frame,
                        &texts,
                    );
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        // In-page navigations (link clicks) don't go through Cmd::Navigate.
        {
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
            match view.take_snapshot_png() {
                Ok(png) if super::page_gen() == gen => {
                    if let Some(f) = png_to_frame(&png) {
                        log(&format!("snap ok {}x{}", f.w, f.h));
                        *frame.lock().unwrap() = Some(Arc::new(f));
                    }
                }
                Ok(_) => {} // navigated away while snapshotting
                Err(e) => log(&format!("snap err: {e}")),
            }
        }

        webkit::pump_run_loop(0.015);
    }
}

fn handle_cmd(
    cmd: Cmd,
    view: &WebView,
    overlay: &mut bool,
    page_zoom: &mut f64,
    logical: &mut (f64, f64),
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
            logical.0 = w.max(1) as f64;
            logical.1 = h.max(1) as f64;
        }
        Cmd::Zoom(z) => {
            *page_zoom = z.max(0.01);
            view.set_page_zoom(*page_zoom);
        }
        Cmd::Script(s) => {
            let _ = view.evaluate_javascript(&s);
        }
        Cmd::Mouse { kind, x, y } => {
            // Map TUI logical pixels → native WKWebView CSS pixels.
            let sx = (x as f64) * (NATIVE_W / logical.0) / *page_zoom;
            let sy = (y as f64) * (NATIVE_H / logical.1) / *page_zoom;
            let js = mouse_sim_js(kind, sx.round() as i32, sy.round() as i32);
            let _ = view.evaluate_javascript(&js);
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

fn png_to_frame(png: &[u8]) -> Option<Frame> {
    let img = image::load_from_memory(png).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for px in rgba.pixels() {
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
