//! Off-screen WebKitGTK engine for Linux. A GTK offscreen window hosts a
//! WebView on a dedicated thread running the GLib main loop by hand;
//! snapshots are converted from cairo surfaces into RGB frames.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use javascriptcore::ValueExt;
use webkit2gtk::{SettingsExt, SnapshotOptions, SnapshotRegion, WebView, WebViewExt};

use super::{
    clear_page_state, mouse_sim_js, parse_text_runs, Cmd, Frame, PageInfo, Status, TextRun,
    EXTRACT_JS, REMOVE_STYLE_JS,
};

pub struct Engine {
    tx: Sender<Cmd>,
    frame: Arc<Mutex<Option<Arc<Frame>>>>,
    info: Arc<Mutex<PageInfo>>,
    status: Arc<Mutex<Status>>,
    texts: Arc<Mutex<Vec<TextRun>>>,
}

impl Engine {
    pub fn start(initial: Cmd, zoom: f64, w: i32, h: i32, _private: bool) -> Engine {
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

    /// WebKitGTK cookie management is not wired up here; the browser shows an
    /// empty inventory rather than pretending.
    pub fn cookies(&self) -> Vec<super::CookieInfo> {
        Vec::new()
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
    if gtk::init().is_err() {
        *status.lock().unwrap() = Status::Error(
            "cannot initialize GTK — no display found. The web/video engine needs a \
             graphical session (X11 or Wayland; on servers try xvfb-run)."
                .into(),
        );
        return;
    }

    let win = gtk::OffscreenWindow::new();
    let view = WebView::new();
    if let Some(settings) = WebViewExt::settings(&view) {
        settings.set_media_playback_requires_user_gesture(false);
        settings.set_enable_developer_extras(false);
        // accelerated compositing breaks off-screen snapshots
        settings.set_hardware_acceleration_policy(webkit2gtk::HardwareAccelerationPolicy::Never);
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
                Ok(cmd) => handle_cmd(cmd, &win, &view, &overlay, &frame, &texts),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    gtk::main_quit();
                    return gtk::glib::ControlFlow::Break;
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

fn handle_cmd(
    cmd: Cmd,
    win: &gtk::OffscreenWindow,
    view: &WebView,
    overlay: &Rc<Cell<bool>>,
    frame: &Arc<Mutex<Option<Arc<Frame>>>>,
    texts: &Arc<Mutex<Vec<TextRun>>>,
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
        // Cookie and site-data management are macOS-only so far. The browser
        // reports that rather than silently doing nothing.
        Cmd::ClearData(_) | Cmd::SetCookiePolicy(_) | Cmd::RefreshCookies => {}
    }
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
