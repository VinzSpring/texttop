//! Off-screen WebKitGTK engine for Linux. A GTK offscreen window hosts a
//! WebView on a dedicated thread running the GLib main loop by hand;
//! snapshots are converted from cairo surfaces into RGB frames.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use javascriptcore::ValueExt;
use webkit2gtk::{
    CookieAcceptPolicy, CookieManagerExt, CookiePersistentStorage, SettingsExt, SnapshotOptions,
    SnapshotRegion, UserContentInjectedFrames, UserContentManager, UserContentManagerExt,
    UserScript, UserScriptInjectionTime, WebContext, WebContextExt, WebView, WebViewExt,
    WebsiteDataManagerExt, WebsiteDataManagerExtManual, WebsiteDataTypes,
};

use super::{
    clear_page_state, mouse_sim_js, parse_text_runs, Cmd, CookieInfo, DataKind, Frame, PageInfo,
    Status, TextRun, CHROME_UA, EXTRACT_JS, REMOVE_STYLE_JS, VISIBILITY_JS,
};

pub struct Engine {
    tx: Sender<Cmd>,
    frame: Arc<Mutex<Option<Arc<Frame>>>>,
    info: Arc<Mutex<PageInfo>>,
    status: Arc<Mutex<Status>>,
    texts: Arc<Mutex<Vec<TextRun>>>,
    cookies: Arc<Mutex<Vec<CookieInfo>>>,
}

impl Engine {
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
                    rx,
                    frame,
                    info,
                    status,
                    texts,
                    cookies,
                    initial,
                    zoom,
                    (w, h),
                    private,
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

    /// Cookies as of the last [`Cmd::RefreshCookies`] — the cookie manager
    /// answers asynchronously on the GTK thread.
    pub fn cookies(&self) -> Vec<CookieInfo> {
        self.cookies.lock().unwrap().clone()
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
    cookies: Arc<Mutex<Vec<CookieInfo>>>,
    initial: Cmd,
    zoom: f64,
    size0: (i32, i32),
    private: bool,
) {
    if gtk::init().is_err() {
        *status.lock().unwrap() = Status::Error(
            "cannot initialize GTK — no display found. The web/video engine needs a \
             graphical session (X11 or Wayland; on servers try xvfb-run)."
                .into(),
        );
        return;
    }

    let win = gtk::OffscreenWindow::new();
    let cm = UserContentManager::new();
    let script = UserScript::new(
        VISIBILITY_JS,
        UserContentInjectedFrames::AllFrames,
        UserScriptInjectionTime::Start,
        &[],
        &[],
    );
    cm.add_script(&script);
    // Private windows get an ephemeral context: cookies, caches and storage
    // stay in memory and die with the view. Otherwise cookies go to disk so
    // logins survive restarts, as they do with the other engines.
    let ctx = if private {
        WebContext::new_ephemeral()
    } else {
        let ctx = WebContext::default().unwrap_or_else(WebContext::new);
        if let (Some(cookie_mgr), Some(dir)) = (ctx.cookie_manager(), dirs::data_local_dir()) {
            let dir = dir.join("texttop").join("webkitgtk");
            if std::fs::create_dir_all(&dir).is_ok() {
                cookie_mgr.set_persistent_storage(
                    &dir.join("cookies.sqlite").to_string_lossy(),
                    CookiePersistentStorage::Sqlite,
                );
            }
        }
        ctx
    };
    let view = WebView::builder()
        .web_context(&ctx)
        .user_content_manager(&cm)
        .build();
    let privacy = Privacy {
        ctx,
        cookies,
        inflight: Rc::new(Cell::new(0)),
    };
    let mut quit_at: Option<Instant> = None;
    if let Some(settings) = WebViewExt::settings(&view) {
        settings.set_media_playback_requires_user_gesture(false);
        settings.set_enable_developer_extras(false);
        // accelerated compositing breaks off-screen snapshots
        settings.set_hardware_acceleration_policy(webkit2gtk::HardwareAccelerationPolicy::Never);
        settings.set_user_agent(Some(CHROME_UA));
    }
    view.set_zoom_level(zoom);
    win.add(&view);
    win.set_default_size(size0.0, size0.1);
    view.set_size_request(size0.0, size0.1);
    win.show_all();

    match &initial {
        Cmd::Navigate(u) => view.load_uri(u),
        Cmd::NavigateHtml(h) => view.load_html(h, None),
        _ => {}
    }
    *status.lock().unwrap() = Status::Ready;

    let capturing = Rc::new(Cell::new(false));
    let mut last_cap = Instant::now() - Duration::from_secs(1);
    let mut last_extract = Instant::now() - Duration::from_secs(1);
    let overlay = Rc::new(Cell::new(false));

    gtk::glib::timeout_add_local(Duration::from_millis(15), move || {
        loop {
            match rx.try_recv() {
                Ok(cmd) => handle_cmd(cmd, &win, &view, &overlay, &frame, &texts, &privacy),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Give queued store operations (clear-on-exit) time to land.
                    let deadline =
                        *quit_at.get_or_insert_with(|| Instant::now() + Duration::from_secs(2));
                    if privacy.inflight.get() == 0 || Instant::now() >= deadline {
                        gtk::main_quit();
                        return gtk::glib::ControlFlow::Break;
                    }
                    return gtk::glib::ControlFlow::Continue;
                }
            }
        }
        // In-page navigations (link clicks) don't go through Cmd::Navigate.
        {
            let url = view.uri().map(|s| s.to_string()).unwrap_or_default();
            let title = view.title().map(|s| s.to_string()).unwrap_or_default();
            let mut i = info.lock().unwrap();
            if !url.is_empty() && !i.url.is_empty() && i.url != url {
                clear_page_state(&frame, &texts);
            }
            if !url.is_empty() {
                i.url = url;
            }
            i.title = title;
        }
        if overlay.get() && last_extract.elapsed() >= Duration::from_millis(300) {
            last_extract = Instant::now();
            let texts_cb = texts.clone();
            view.run_javascript(EXTRACT_JS, None::<&gtk::gio::Cancellable>, move |res| {
                if let Ok(jsres) = res {
                    if let Some(value) = jsres.js_value() {
                        let s = value.to_str();
                        *texts_cb.lock().unwrap() = parse_text_runs(s.as_str());
                    }
                }
            });
        }
        if !capturing.get() && last_cap.elapsed() >= Duration::from_millis(33) {
            capturing.set(true);
            last_cap = Instant::now();
            log("snap start");
            let frame = frame.clone();
            let capturing_cb = capturing.clone();
            let gen = super::page_gen();
            view.snapshot(
                SnapshotRegion::Visible,
                SnapshotOptions::NONE,
                None::<&gtk::gio::Cancellable>,
                move |res| {
                    if super::page_gen() == gen {
                        match res {
                            Ok(surface) => {
                                if let Some(f) = surface_to_frame(&surface) {
                                    log(&format!("snap ok {}x{}", f.w, f.h));
                                    *frame.lock().unwrap() = Some(Arc::new(f));
                                } else {
                                    log("snap ok but no image surface");
                                }
                            }
                            Err(e) => log(&format!("snap err: {e}")),
                        }
                    }
                    capturing_cb.set(false);
                },
            );
        }
        gtk::glib::ControlFlow::Continue
    });
    gtk::main();
}

fn log(msg: &str) {
    if std::env::var_os("TEXTTOP_ENGINE_LOG").is_some() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/texttop-engine.log")
        {
            let _ = writeln!(f, "{msg}");
        }
    }
}

/// Cookie / site-data state owned by the GTK thread.
struct Privacy {
    ctx: WebContext,
    cookies: Arc<Mutex<Vec<CookieInfo>>>,
    /// Async store operations still running.
    inflight: Rc<Cell<u32>>,
}

fn handle_cmd(
    cmd: Cmd,
    win: &gtk::OffscreenWindow,
    view: &WebView,
    overlay: &Rc<Cell<bool>>,
    frame: &Arc<Mutex<Option<Arc<Frame>>>>,
    texts: &Arc<Mutex<Vec<TextRun>>>,
    privacy: &Privacy,
) {
    match cmd {
        Cmd::TextOverlay(on) => {
            overlay.set(on);
            if !on {
                texts.lock().unwrap().clear();
                view.run_javascript(REMOVE_STYLE_JS, None::<&gtk::gio::Cancellable>, |_| {});
            }
        }
        Cmd::Navigate(u) => {
            clear_page_state(frame, texts);
            view.load_uri(&u);
        }
        Cmd::NavigateHtml(h) => {
            clear_page_state(frame, texts);
            view.load_html(&h, None);
        }
        Cmd::Back => {
            clear_page_state(frame, texts);
            view.go_back();
        }
        Cmd::Forward => {
            clear_page_state(frame, texts);
            view.go_forward();
        }
        Cmd::Reload => {
            clear_page_state(frame, texts);
            view.reload();
        }
        Cmd::ResizePx(w, h) => {
            win.resize(w, h);
            view.set_size_request(w, h);
        }
        Cmd::Zoom(z) => view.set_zoom_level(z),
        Cmd::Script(s) => {
            view.run_javascript(&s, None::<&gtk::gio::Cancellable>, |_| {});
        }
        Cmd::Mouse { kind, x, y } => {
            let z = view.zoom_level().max(0.01);
            let cx = (x as f64 / z).round() as i32;
            let cy = (y as f64 / z).round() as i32;
            let s = mouse_sim_js(kind, cx, cy);
            view.run_javascript(&s, None::<&gtk::gio::Cancellable>, |_| {});
        }
        Cmd::ClearData(kind) => clear_data(privacy, kind),
        Cmd::SetCookiePolicy(accept) => {
            if let Some(cm) = privacy.ctx.cookie_manager() {
                cm.set_accept_policy(if accept {
                    CookieAcceptPolicy::Always
                } else {
                    CookieAcceptPolicy::Never
                });
            }
        }
        Cmd::RefreshCookies => read_cookies(privacy),
    }
}

fn data_types(kind: DataKind) -> WebsiteDataTypes {
    match kind {
        DataKind::Cookies => WebsiteDataTypes::COOKIES,
        DataKind::Cache => {
            WebsiteDataTypes::MEMORY_CACHE
                | WebsiteDataTypes::DISK_CACHE
                | WebsiteDataTypes::DOM_CACHE
        }
        DataKind::All => WebsiteDataTypes::ALL,
    }
}

fn clear_data(privacy: &Privacy, kind: DataKind) {
    let Some(dm) = privacy.ctx.website_data_manager() else {
        return;
    };
    privacy.inflight.set(privacy.inflight.get() + 1);
    // The completion callback must be Send, so it reports back through a
    // channel the GTK thread polls instead of touching the Rc counter.
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let inflight = privacy.inflight.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(20), move || match done_rx.try_recv() {
        Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
        _ => {
            inflight.set(inflight.get().saturating_sub(1));
            gtk::glib::ControlFlow::Break
        }
    });
    let cookies = privacy.cookies.clone();
    // A zero timespan means "everything, regardless of age".
    dm.clear(
        data_types(kind),
        gtk::glib::TimeSpan::from_seconds(0),
        None::<&gtk::gio::Cancellable>,
        move |res| {
            if let Err(e) = res {
                log(&format!("clear data err: {e}"));
            }
            if kind != DataKind::Cache {
                cookies.lock().unwrap().clear();
            }
            let _ = done_tx.send(());
        },
    );
}

/// WebKitGTK lists cookies per URI, so enumerate the domains holding cookies
/// first, then collect each domain's jar.
fn read_cookies(privacy: &Privacy) {
    let (Some(dm), Some(cm)) = (
        privacy.ctx.website_data_manager(),
        privacy.ctx.cookie_manager(),
    ) else {
        return;
    };
    let out = privacy.cookies.clone();
    dm.fetch(
        WebsiteDataTypes::COOKIES,
        None::<&gtk::gio::Cancellable>,
        move |res| {
            let Ok(sites) = res else {
                return;
            };
            let acc: Rc<RefCell<Vec<CookieInfo>>> = Rc::default();
            out.lock().unwrap().clear();
            for site in sites {
                let Some(domain) = site.name() else {
                    continue;
                };
                let acc = acc.clone();
                let out = out.clone();
                let uri = format!("https://{}/", domain.trim_start_matches('.'));
                cm.cookies(&uri, None::<&gtk::gio::Cancellable>, move |res| {
                    let Ok(list) = res else {
                        return;
                    };
                    let mut acc = acc.borrow_mut();
                    for mut c in list {
                        let info = CookieInfo {
                            domain: c.domain().map(|d| d.to_string()).unwrap_or_default(),
                            name: c.name().map(|n| n.to_string()).unwrap_or_default(),
                            secure: c.is_secure(),
                            http_only: c.is_http_only(),
                            session_only: c.expires().is_none(),
                        };
                        let dup = acc
                            .iter()
                            .any(|a| a.domain == info.domain && a.name == info.name);
                        if !dup {
                            acc.push(info);
                        }
                    }
                    acc.sort_by(|a, b| a.domain.cmp(&b.domain).then(a.name.cmp(&b.name)));
                    *out.lock().unwrap() = acc.clone();
                });
            }
        },
    );
}

fn surface_to_frame(surface: &gtk::cairo::Surface) -> Option<Frame> {
    use gtk::cairo::{Context, Format, ImageSurface};
    // measure the snapshot
    let (w, h) = match ImageSurface::try_from(surface.clone()) {
        Ok(img) => (img.width(), img.height()),
        Err(_) => {
            let cr = Context::new(surface).ok()?;
            let (x0, y0, x1, y1) = cr.clip_extents().ok()?;
            ((x1 - x0) as i32, (y1 - y0) as i32)
        }
    };
    if w <= 0 || h <= 0 {
        return None;
    }
    // copy into a surface we own exclusively so the pixel data is readable
    let img = ImageSurface::create(Format::ARgb32, w, h).ok()?;
    {
        let cr = Context::new(&img).ok()?;
        cr.set_source_surface(surface, 0.0, 0.0).ok()?;
        cr.paint().ok()?;
    }
    let stride = img.stride();
    let data = img.take_data().ok()?;
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        let row = (y * stride) as usize;
        for x in 0..w {
            let i = row + (x * 4) as usize;
            if i + 3 < data.len() {
                // cairo ARgb32 little-endian, premultiplied: B, G, R, A.
                // Composite over white so transparency isn't black.
                let a = data[i + 3] as u32;
                let white = 255 - a;
                rgb.push((data[i + 2] as u32 + white).min(255) as u8);
                rgb.push((data[i + 1] as u32 + white).min(255) as u8);
                rgb.push((data[i] as u32 + white).min(255) as u8);
            } else {
                rgb.extend_from_slice(&[255, 255, 255]);
            }
        }
    }
    Some(Frame {
        w: w as u32,
        h: h as u32,
        rgb,
        seq: super::next_seq(),
    })
}
