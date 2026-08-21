//! Off-screen WKWebView engine for macOS. Uses the `webkit` crate's headless
//! WebKit bindings on a dedicated thread; raw RGBA snapshots (scaled to the
//! logical viewport width) are converted into RGB frames for the TUI.
//!
//! The view is hosted in a real (invisible, click-through) window, without
//! which pages never composite; see `resize` and WINDOW_ALPHA below. The
//! layout viewport tracks the TUI window so pages get their true CSS width.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use webkit::prelude::*;

use super::{
    clear_page_state, mouse_sim_js, parse_text_runs, Cmd, CookieInfo, DataKind, Frame, PageInfo,
    Status, TextRun, EXTRACT_JS, REMOVE_STYLE_JS, VISIBILITY_JS,
};

/// Safety bound on the layout viewport, well above any real terminal.
///
/// The viewport tracks the TUI window so pages get their true CSS width and
/// lay out like a desktop browser. Shrinking it does buy frame rate — capture
/// cost tracks the layout viewport, not the snapshot resolution, so a video
/// at 1712x1040 costs ~81ms per frame against ~30ms at 856x520 — but it also
/// narrows the page, and sites with a min-width (YouTube's consent dialog
/// among them) then overflow horizontally and the crisp-text overlay spreads
/// words apart, because a cell no longer corresponds to CELL_PX_W CSS pixels.
/// Fidelity wins; this bound only exists so an enormous terminal cannot ask
/// for an absurd surface.
const MAX_VIEW_W: f64 = 2560.0;
const MAX_VIEW_H: f64 = 1600.0;

/// Fully transparent, so the hosting window is invisible; see `attach_window`.
const WINDOW_ALPHA: f64 = 0.0;

/// Fit `w`x`h` inside [`MAX_VIEW_W`]/[`MAX_VIEW_H`], preserving aspect.
/// Returns the viewport size and the CSS-pixels-per-TUI-pixel scale that
/// mouse input must undo — 1.0 unless the bound actually bit.
fn capped_viewport(w: f64, h: f64) -> (f64, f64, f64) {
    let (w, h) = (w.max(1.0), h.max(1.0));
    let s = (MAX_VIEW_W / w).min(MAX_VIEW_H / h).min(1.0);
    ((w * s).round().max(1.0), (h * s).round().max(1.0), s)
}

/// Engine state that commands mutate.
struct ViewState {
    overlay: bool,
    page_zoom: f64,
    /// Snapshot width, equal to the capped viewport width so one frame pixel
    /// is one CSS pixel.
    snap_w: u32,
    /// CSS pixels per TUI pixel, from capping the viewport.
    scale: f64,
}

pub struct Engine {
    tx: Sender<Cmd>,
    frame: Arc<Mutex<Option<Arc<Frame>>>>,
    info: Arc<Mutex<PageInfo>>,
    status: Arc<Mutex<Status>>,
    texts: Arc<Mutex<Vec<TextRun>>>,
    cookies: Arc<Mutex<Vec<CookieInfo>>>,
}

impl Engine {
    /// `private` gives the view a non-persistent data store: cookies, caches
    /// and local storage live in memory and die with the window.
    pub fn start(initial: Cmd, zoom: f64, w: i32, h: i32, private: bool) -> Engine {
        let (tx, rx) = mpsc::channel();
        let frame: Arc<Mutex<Option<Arc<Frame>>>> = Arc::new(Mutex::new(None));
        let info = Arc::new(Mutex::new(PageInfo::default()));
        let status = Arc::new(Mutex::new(Status::Starting));
        let texts: Arc<Mutex<Vec<TextRun>>> = Arc::new(Mutex::new(Vec::new()));
        let cookies: Arc<Mutex<Vec<CookieInfo>>> = Arc::new(Mutex::new(Vec::new()));
        {
            let frame = frame.clone();
            let info = info.clone();
            let status = status.clone();
            let texts = texts.clone();
            let cookies = cookies.clone();
            std::thread::spawn(move || {
                run(
                    rx, frame, info, status, texts, cookies, initial, zoom, (w, h), private,
                )
            });
        }
        Engine {
            tx,
            frame,
            info,
            status,
            texts,
            cookies,
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

    /// Cookies as of the last [`Cmd::RefreshCookies`] — reading the store is a
    /// main-thread round-trip, so it happens on request, not every frame.
    pub fn cookies(&self) -> Vec<CookieInfo> {
        self.cookies.lock().unwrap().clone()
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    rx: Receiver<Cmd>,
    frame: Arc<Mutex<Option<Arc<Frame>>>>,
    info: Arc<Mutex<PageInfo>>,
    status: Arc<Mutex<Status>>,
    texts: Arc<Mutex<Vec<TextRun>>>,
    cookies: Arc<Mutex<Vec<CookieInfo>>>,
    initial: Cmd,
    zoom: f64,
    size0: (i32, i32),
    private: bool,
) {
    webkit::init_app();

    let config = WebViewConfiguration::new();
    // Private browsing is a property of the data store, and the store can only
    // be chosen before the view exists — which is why the setting applies to
    // newly opened windows rather than toggling the current one. A private
    // store keeps cookies, caches and local storage in memory so they die with
    // the window. The store is held for the engine's lifetime so the cookie and
    // site-data commands can reach the same store the view uses.
    let store = if private {
        WebsiteDataStore::non_persistent()
    } else {
        WebsiteDataStore::default_data_store()
    };
    config.set_website_data_store(&store);
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

    let mut st = ViewState {
        overlay: false,
        page_zoom: zoom.max(0.01),
        snap_w: 1,
        scale: 1.0,
    };
    view.set_page_zoom(st.page_zoom);
    resize(&view, &mut st, size0.0, size0.1);

    if let Err(e) = apply_nav(&view, &initial) {
        *status.lock().unwrap() = Status::Error(format!("initial navigation failed: {e}"));
        shutdown(&view);
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
                    handle_cmd(cmd, &view, &mut st, &frame, &texts, &cookies, &store);
                }
                Err(TryRecvError::Empty) => break,
                // The app window closed and dropped the sender.
                Err(TryRecvError::Disconnected) => {
                    shutdown(&view);
                    return;
                }
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

        if st.overlay && last_extract.elapsed() >= Duration::from_millis(300) {
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
            match view.take_snapshot_rgba(Some(st.snap_w)) {
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

/// Stop everything the page is doing and release the hosting window.
///
/// Dropping the `WebView` is not enough on its own: the window created by
/// `attach_window` retains it as its content view, so the page survives its
/// tab being closed and carries on playing audio. Media is paused explicitly
/// first, because a page can hold references that keep it alive regardless.
fn shutdown(view: &WebView) {
    let _ = view.evaluate_javascript(
        "(function(){try{document.querySelectorAll('video,audio')\
         .forEach(function(m){m.pause();m.muted=true;m.removeAttribute('src');m.load();});\
         }catch(e){}})()",
    );
    let _ = view.load_html("", None);
    view.detach_window();
}

/// Apply a capped viewport of `w`x`h` TUI pixels, keeping the hosting window
/// in step.
///
/// The view must live in a real window. Without one the page is treated as a
/// background tab: `requestAnimationFrame` is never serviced and `<video>`
/// never decodes (`videoWidth` stays 0), so pages capture frozen. The window
/// is fully transparent and click-through, so it is never seen.
fn resize(view: &WebView, st: &mut ViewState, w: i32, h: i32) {
    let (vw, vh, scale) = capped_viewport(w as f64, h as f64);
    st.snap_w = vw as u32;
    st.scale = scale;
    view.set_frame_size(vw, vh);
    view.attach_window(vw, vh, 1, WINDOW_ALPHA);
}

fn handle_cmd(
    cmd: Cmd,
    view: &WebView,
    st: &mut ViewState,
    frame: &Arc<Mutex<Option<Arc<Frame>>>>,
    texts: &Arc<Mutex<Vec<TextRun>>>,
    cookies: &Arc<Mutex<Vec<CookieInfo>>>,
    store: &WebsiteDataStore,
) {
    match cmd {
        Cmd::TextOverlay(on) => {
            st.overlay = on;
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
        Cmd::ResizePx(w, h) => resize(view, st, w, h),
        Cmd::Zoom(z) => {
            st.page_zoom = z.max(0.01);
            view.set_page_zoom(st.page_zoom);
        }
        Cmd::Script(s) => {
            let _ = view.evaluate_javascript(&s);
        }
        Cmd::Mouse { kind, x, y } => {
            // TUI px -> CSS px: undo the viewport cap, then the page zoom.
            let cx = (x as f64 * st.scale / st.page_zoom).round() as i32;
            let cy = (y as f64 * st.scale / st.page_zoom).round() as i32;
            let _ = view.evaluate_javascript(&mouse_sim_js(kind, cx, cy));
        }
        Cmd::ClearData(kind) => {
            // Removing everything modified since the epoch clears the lot.
            if let Err(e) =
                store.remove_data_modified_since(&data_types(kind), SystemTime::UNIX_EPOCH)
            {
                log(&format!("clear data err: {e}"));
            }
        }
        Cmd::SetCookiePolicy(accept) => {
            let policy = if accept {
                CookiePolicy::Allow
            } else {
                CookiePolicy::Disallow
            };
            match store.http_cookie_store() {
                Ok(cs) => {
                    if let Err(e) = cs.set_cookie_policy(policy) {
                        log(&format!("cookie policy err: {e}"));
                    }
                }
                Err(e) => log(&format!("cookie store err: {e}")),
            }
        }
        Cmd::RefreshCookies => read_cookies(store, cookies),
    }
}

fn data_types(kind: DataKind) -> Vec<WebsiteDataType> {
    match kind {
        DataKind::Cookies => vec![WebsiteDataType::cookies()],
        DataKind::Cache => vec![
            WebsiteDataType::disk_cache(),
            WebsiteDataType::memory_cache(),
            WebsiteDataType::fetch_cache(),
        ],
        DataKind::All => WebsiteDataStore::all_website_data_types(),
    }
}

fn read_cookies(store: &WebsiteDataStore, out: &Arc<Mutex<Vec<CookieInfo>>>) {
    let list = store
        .http_cookie_store()
        .and_then(|c| c.all_cookies())
        .unwrap_or_default();
    let mut v: Vec<CookieInfo> = list
        .into_iter()
        .map(|c| CookieInfo {
            domain: c.domain,
            name: c.name,
            secure: c.secure,
            http_only: c.http_only,
            session_only: c.session_only,
        })
        .collect();
    v.sort_by(|a, b| a.domain.cmp(&b.domain).then(a.name.cmp(&b.name)));
    *out.lock().unwrap() = v;
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
