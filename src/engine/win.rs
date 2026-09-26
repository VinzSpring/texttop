//! Off-screen WebView2 (Edge) engine for Windows. Runs on its own STA
//! thread with a hidden window and a message pump; the TUI talks to it
//! through a command channel and shared frame slot.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2Controller,
    ICoreWebView2CookieList, ICoreWebView2Environment10, ICoreWebView2EnvironmentOptions,
    ICoreWebView2Profile2, ICoreWebView2Settings2, ICoreWebView2_13, ICoreWebView2_2,
    COREWEBVIEW2_BROWSING_DATA_KINDS, COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_SITE,
    COREWEBVIEW2_BROWSING_DATA_KINDS_CACHE_STORAGE, COREWEBVIEW2_BROWSING_DATA_KINDS_COOKIES,
    COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE, COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG,
};
use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler, CapturePreviewCompletedHandler,
    ClearBrowsingDataCompletedHandler,
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, ExecuteScriptCompletedHandler,
    GetCookiesCompletedHandler, NavigationCompletedEventHandler, NewWindowRequestedEventHandler,
};
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HGLOBAL, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, IStream, COINIT_APARTMENTTHREADED, STREAM_SEEK_SET,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumChildWindows,
    GetClassNameW, MoveWindow, PeekMessageW, PostMessageW, RegisterClassW, ShowWindow,
    TranslateMessage, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows_core::BOOL;

/// Win32 MK_LBUTTON — left button held during WM_MOUSEMOVE / button messages.
const MK_LBUTTON: usize = 0x0001;

use super::{
    clear_page_state, mouse_sim_js, parse_text_runs, Cmd, CookieInfo, DataKind, Frame, MouseKind,
    PageInfo, Status, TextRun, CHROME_UA, EXTRACT_JS, REMOVE_STYLE_JS, VISIBILITY_JS,
};

pub struct Engine {
    tx: Sender<Cmd>,
    frame: Arc<Mutex<Option<Arc<Frame>>>>,
    info: Arc<Mutex<PageInfo>>,
    status: Arc<Mutex<Status>>,
    texts: Arc<Mutex<Vec<TextRun>>>,
    cookies: Arc<Mutex<Vec<CookieInfo>>>,
}

/// Cookie / site-data state owned by the engine thread.
struct Privacy {
    cookies: Arc<Mutex<Vec<CookieInfo>>>,
    /// WebView2 has no "refuse cookies" switch, so a refusing policy is
    /// enforced by wiping the jar after every navigation and periodically.
    block: Rc<Cell<bool>>,
    /// Async store operations still running; shutdown waits for them so a
    /// clear-on-exit actually lands.
    inflight: Rc<Cell<u32>>,
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
    /// answers asynchronously on the engine thread.
    pub fn cookies(&self) -> Vec<CookieInfo> {
        self.cookies.lock().unwrap().clone()
    }

    pub fn texts(&self) -> Vec<TextRun> {
        self.texts.lock().unwrap().clone()
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Opt-in debug log (`TEXTTOP_ENGINE_LOG=1`) → %TEMP%\texttop-engine.log
fn log_engine(msg: &str) {
    if std::env::var_os("TEXTTOP_ENGINE_LOG").is_none() {
        return;
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("texttop-engine.log"))
    {
        let _ = writeln!(f, "{msg}");
    }
}

unsafe fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while *p.0.add(len) != 0 {
        len += 1;
    }
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(p.0, len));
    CoTaskMemFree(Some(p.0 as _));
    s
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, w, l)
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
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let privacy = Privacy {
        cookies,
        block: Rc::new(Cell::new(false)),
        inflight: Rc::new(Cell::new(0)),
    };
    let set_err = |msg: String| {
        *status.lock().unwrap() = Status::Error(msg);
    };

    let hinstance = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => h,
        Err(e) => {
            set_err(format!("engine init failed: {e}"));
            return;
        }
    };
    let cls = wide("texttop_engine_wnd");
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc); // second registration fails harmlessly
    }
    let hwnd = match unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            PCWSTR(cls.as_ptr()),
            PCWSTR(cls.as_ptr()),
            WS_POPUP,
            -32000,
            -32000,
            size0.0,
            size0.1,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    } {
        Ok(h) => h,
        Err(e) => {
            set_err(format!("engine window failed: {e}"));
            return;
        }
    };
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }

    let controller_slot: Rc<RefCell<Option<ICoreWebView2Controller>>> = Rc::default();
    let webview_slot: Rc<RefCell<Option<ICoreWebView2>>> = Rc::default();

    // "-sw2" suffix: browser args (--disable-gpu etc.) only apply to a freshly
    // spawned browser process, and processes are shared per data folder —
    // a distinct folder guarantees processes started with older flag sets
    // are never reused
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("texttop")
        .join("webview2-sw2");
    let _ = std::fs::create_dir_all(&data_dir);
    let data_w = wide(&data_dir.to_string_lossy());

    let opts = CoreWebView2EnvironmentOptions::default();
    unsafe {
        // --disable-gpu: GPU-composited layers come back transparent/black
        // from CapturePreview on some machines; software rendering is
        // reliable and plenty fast for ~10 fps captures.
        //
        // The host window lives fully off-screen (-32000,-32000). Chromium's
        // native occlusion tracker treats such windows as occluded and stops
        // rasterizing, which makes CapturePreview return blank/white frames
        // (text extraction still works, so pages showed text on white).
        // Disable occlusion detection and background throttling so the page
        // keeps painting.
        opts.set_additional_browser_arguments(
            "--autoplay-policy=no-user-gesture-required --disable-gpu \
             --disable-features=CalculateNativeWinOcclusion \
             --disable-backgrounding-occluded-windows \
             --disable-renderer-backgrounding"
                .to_string(),
        );
    }
    let iopts: ICoreWebView2EnvironmentOptions = opts.into();

    {
        let controller_slot = controller_slot.clone();
        let webview_slot = webview_slot.clone();
        let status_env = status.clone();
        let block_env = privacy.block.clone();
        let mut initial = Some(initial);
        let env_handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |res: windows_core::Result<()>, env| {
                let env = match (res, env) {
                    (Ok(()), Some(e)) => e,
                    (Err(e), _) => {
                        *status_env.lock().unwrap() = Status::Error(format!(
                            "WebView2 runtime unavailable ({e}). It ships with Windows 10/11; \
                             if missing, install 'Microsoft Edge WebView2 Runtime'."
                        ));
                        return Ok(());
                    }
                    _ => {
                        *status_env.lock().unwrap() =
                            Status::Error("WebView2 environment creation failed".into());
                        return Ok(());
                    }
                };
                let status_ctrl = status_env.clone();
                let controller_slot = controller_slot.clone();
                let webview_slot = webview_slot.clone();
                let block_ctrl = block_env.clone();
                let mut initial = initial.take();
                let ctrl_handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                    move |res: windows_core::Result<()>, controller| {
                        let controller = match (res, controller) {
                            (Ok(()), Some(c)) => c,
                            _ => {
                                *status_ctrl.lock().unwrap() =
                                    Status::Error("browser controller creation failed".into());
                                return Ok(());
                            }
                        };
                        unsafe {
                            let _ = controller.SetBounds(RECT {
                                left: 0,
                                top: 0,
                                right: size0.0,
                                bottom: size0.1,
                            });
                            let _ = controller.SetZoomFactor(zoom);
                            let _ = controller.SetIsVisible(true);
                        }
                        let webview = match unsafe { controller.CoreWebView2() } {
                            Ok(w) => w,
                            Err(e) => {
                                *status_ctrl.lock().unwrap() =
                                    Status::Error(format!("webview unavailable: {e}"));
                                return Ok(());
                            }
                        };
                        unsafe {
                            if let Ok(settings) = webview.Settings() {
                                if let Ok(s2) = settings.cast::<ICoreWebView2Settings2>() {
                                    let ua = wide(CHROME_UA);
                                    let _ = s2.SetUserAgent(PCWSTR(ua.as_ptr()));
                                }
                            }
                        }
                        // keep popups/new tabs in the same view
                        unsafe {
                            let mut token: i64 = 0;
                            let _ = webview.add_NewWindowRequested(
                                &NewWindowRequestedEventHandler::create(Box::new(
                                    |sender, args| {
                                        if let (Some(sender), Some(args)) = (sender, args) {
                                            let _ = args.SetHandled(true);
                                            let mut uri = PWSTR::null();
                                            if args.Uri(&mut uri).is_ok() {
                                                let u = take_pwstr(uri);
                                                let uw = wide(&u);
                                                let _ = sender.Navigate(PCWSTR(uw.as_ptr()));
                                            }
                                        }
                                        Ok(())
                                    },
                                )),
                                &mut token,
                            );
                        }
                        // The host window is off-screen; make pages treat
                        // themselves as a visible, focused tab (see VISIBILITY_JS).
                        unsafe {
                            let js = wide(VISIBILITY_JS);
                            let _ = webview.AddScriptToExecuteOnDocumentCreated(
                                PCWSTR(js.as_ptr()),
                                &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(
                                    Box::new(|_, _| Ok(())),
                                ),
                            );
                        }
                        // A refusing cookie policy: drop whatever the page
                        // just stored once each navigation settles.
                        unsafe {
                            let block = block_ctrl.clone();
                            let mut token: i64 = 0;
                            let _ = webview.add_NavigationCompleted(
                                &NavigationCompletedEventHandler::create(Box::new(
                                    move |sender, _args| {
                                        if block.get() {
                                            if let Some(w) = sender {
                                                delete_all_cookies(&w);
                                            }
                                        }
                                        Ok(())
                                    },
                                )),
                                &mut token,
                            );
                        }
                        if let Some(init) = initial.take() {
                            unsafe {
                                match init {
                                    Cmd::Navigate(u) => {
                                        let uw = wide(&u);
                                        let _ = webview.Navigate(PCWSTR(uw.as_ptr()));
                                    }
                                    Cmd::NavigateHtml(h) => {
                                        let hw = wide(&h);
                                        let _ = webview.NavigateToString(PCWSTR(hw.as_ptr()));
                                    }
                                    _ => {}
                                }
                            }
                        }
                        *controller_slot.borrow_mut() = Some(controller);
                        *webview_slot.borrow_mut() = Some(webview);
                        *status_ctrl.lock().unwrap() = Status::Ready;
                        Ok(())
                    },
                ));
                // Private windows get an InPrivate profile: cookies, cache and
                // storage live in memory and die with the view.
                let private_opts = if private {
                    env.cast::<ICoreWebView2Environment10>().ok().and_then(|e10| unsafe {
                        let o = e10.CreateCoreWebView2ControllerOptions().ok()?;
                        o.SetIsInPrivateModeEnabled(true).ok()?;
                        Some((e10, o))
                    })
                } else {
                    None
                };
                match private_opts {
                    Some((e10, o)) => unsafe {
                        e10.CreateCoreWebView2ControllerWithOptions(hwnd, &o, &ctrl_handler)
                    }?,
                    None => unsafe { env.CreateCoreWebView2Controller(hwnd, &ctrl_handler) }?,
                }
                Ok(())
            },
        ));
        let created = unsafe {
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                PCWSTR(data_w.as_ptr()),
                &iopts,
                &env_handler,
            )
        };
        if let Err(e) = created {
            set_err(format!(
                "WebView2 runtime not found ({e}). It ships with Windows 10/11; if missing, \
                 install 'Microsoft Edge WebView2 Runtime' from Microsoft."
            ));
        }
    }

    let capturing = Rc::new(Cell::new(false));
    // Identifies the current in-flight capture. CapturePreview's completion
    // handler can silently never fire (e.g. when a navigation swaps the
    // renderer process while a capture is pending); the watchdog below bumps
    // this id and clears `capturing` so captures resume. A late completion
    // from an abandoned capture is ignored via the id mismatch.
    let cap_id = Rc::new(Cell::new(0u64));
    let mut cap_started = Instant::now();
    let render_hwnd = Rc::new(Cell::new(HWND::default()));
    let mouse_down = Rc::new(Cell::new(false));
    let mut last_cap = Instant::now() - Duration::from_secs(1);
    let mut last_extract = Instant::now() - Duration::from_secs(1);
    let mut overlay = false;
    let mut pending: Vec<Cmd> = Vec::new();
    let mut last_wipe = Instant::now();
    'outer: loop {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let ready = webview_slot.borrow().is_some();
        if ready && !pending.is_empty() {
            for cmd in pending.drain(..) {
                handle_cmd(
                    cmd,
                    &controller_slot,
                    &webview_slot,
                    hwnd,
                    &mut overlay,
                    &frame,
                    &texts,
                    &render_hwnd,
                    &mouse_down,
                    &privacy,
                );
            }
        }
        loop {
            match rx.try_recv() {
                Ok(cmd) => {
                    if ready {
                        handle_cmd(
                            cmd,
                            &controller_slot,
                            &webview_slot,
                            hwnd,
                            &mut overlay,
                            &frame,
                            &texts,
                            &render_hwnd,
                            &mouse_down,
                            &privacy,
                        );
                    } else {
                        pending.push(cmd);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'outer,
            }
        }
        // Detect in-page navigations (link clicks) that bypass Cmd::Navigate.
        if let Some(webview) = webview_slot.borrow().clone() {
            unsafe {
                poll_navigation(&webview, &info, &frame, &texts);
            }
        }
        if capturing.get() && cap_started.elapsed() >= Duration::from_millis(1500) {
            // watchdog: abandon a capture whose completion never fired
            log_engine("capture watchdog: abandoning hung capture");
            cap_id.set(cap_id.get() + 1);
            capturing.set(false);
        }
        if !capturing.get() && last_cap.elapsed() >= Duration::from_millis(30) {
            let webview = webview_slot.borrow().clone();
            if let Some(webview) = webview {
                capturing.set(true);
                cap_id.set(cap_id.get() + 1);
                cap_started = Instant::now();
                last_cap = Instant::now();
                start_capture(
                    &webview,
                    frame.clone(),
                    info.clone(),
                    capturing.clone(),
                    cap_id.clone(),
                    cap_id.get(),
                    super::page_gen(),
                );
            }
        }
        // Script-set cookies (after load) are caught by a periodic sweep.
        if privacy.block.get() && last_wipe.elapsed() >= Duration::from_secs(2) {
            last_wipe = Instant::now();
            if let Some(webview) = webview_slot.borrow().clone() {
                delete_all_cookies(&webview);
            }
        }
        if overlay && last_extract.elapsed() >= Duration::from_millis(300) {
            let webview = webview_slot.borrow().clone();
            if let Some(webview) = webview {
                last_extract = Instant::now();
                start_extract(&webview, texts.clone());
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }

    // Let queued store operations (clear-on-exit) finish before the view goes.
    let deadline = Instant::now() + Duration::from_secs(2);
    while privacy.inflight.get() > 0 && Instant::now() < deadline {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    if let Some(c) = controller_slot.borrow_mut().take() {
        unsafe {
            let _ = c.Close();
        }
    }
    *webview_slot.borrow_mut() = None;
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_cmd(
    cmd: Cmd,
    controller: &Rc<RefCell<Option<ICoreWebView2Controller>>>,
    webview: &Rc<RefCell<Option<ICoreWebView2>>>,
    hwnd: HWND,
    overlay: &mut bool,
    frame: &Arc<Mutex<Option<Arc<Frame>>>>,
    texts: &Arc<Mutex<Vec<TextRun>>>,
    render_hwnd: &Rc<Cell<HWND>>,
    mouse_down: &Rc<Cell<bool>>,
    privacy: &Privacy,
) {
    let controller = controller.borrow();
    let webview = webview.borrow();
    unsafe {
        match cmd {
            Cmd::Navigate(u) => {
                clear_page_state(frame, texts);
                if let Some(w) = &*webview {
                    let uw = wide(&u);
                    let _ = w.Navigate(PCWSTR(uw.as_ptr()));
                }
            }
            Cmd::NavigateHtml(h) => {
                clear_page_state(frame, texts);
                if let Some(w) = &*webview {
                    let hw = wide(&h);
                    let _ = w.NavigateToString(PCWSTR(hw.as_ptr()));
                }
            }
            Cmd::Back => {
                clear_page_state(frame, texts);
                if let Some(w) = &*webview {
                    let _ = w.GoBack();
                }
            }
            Cmd::Forward => {
                clear_page_state(frame, texts);
                if let Some(w) = &*webview {
                    let _ = w.GoForward();
                }
            }
            Cmd::Reload => {
                clear_page_state(frame, texts);
                if let Some(w) = &*webview {
                    let _ = w.Reload();
                }
            }
            Cmd::ResizePx(px, py) => {
                let _ = MoveWindow(hwnd, -32000, -32000, px, py, false);
                if let Some(c) = &*controller {
                    let _ = c.SetBounds(RECT {
                        left: 0,
                        top: 0,
                        right: px,
                        bottom: py,
                    });
                }
                // child hierarchy can rebuild across resizes
                render_hwnd.set(HWND::default());
            }
            Cmd::Zoom(z) => {
                if let Some(c) = &*controller {
                    let _ = c.SetZoomFactor(z);
                }
            }
            Cmd::Script(s) => {
                if let Some(w) = &*webview {
                    exec_script(w, &s);
                }
            }
            Cmd::Mouse { kind, x, y } => {
                let posted = post_mouse(hwnd, render_hwnd, mouse_down, kind, x, y);
                if !posted {
                    let mut z = 1.0f64;
                    if let Some(c) = controller.as_ref() {
                        let _ = c.ZoomFactor(&mut z);
                    }
                    let z = z.max(0.01);
                    let cx = (x as f64 / z).round() as i32;
                    let cy = (y as f64 / z).round() as i32;
                    if let Some(w) = &*webview {
                        exec_script(w, &mouse_sim_js(kind, cx, cy));
                    }
                }
            }
            Cmd::TextOverlay(on) => {
                *overlay = on;
                if !on {
                    texts.lock().unwrap().clear();
                    if let Some(w) = &*webview {
                        exec_script(w, REMOVE_STYLE_JS);
                    }
                }
            }
            Cmd::ClearData(kind) => {
                if let Some(w) = &*webview {
                    clear_data(w, kind, privacy);
                }
            }
            Cmd::SetCookiePolicy(accept) => {
                privacy.block.set(!accept);
                if !accept {
                    if let Some(w) = &*webview {
                        delete_all_cookies(w);
                    }
                }
            }
            Cmd::RefreshCookies => {
                if let Some(w) = &*webview {
                    read_cookies(w, privacy);
                }
            }
        }
    }
}

fn delete_all_cookies(webview: &ICoreWebView2) {
    unsafe {
        if let Ok(cm) = webview.cast::<ICoreWebView2_2>().and_then(|w| w.CookieManager()) {
            let _ = cm.DeleteAllCookies();
        }
    }
}

fn browsing_data_kinds(kind: DataKind) -> COREWEBVIEW2_BROWSING_DATA_KINDS {
    match kind {
        DataKind::Cookies => COREWEBVIEW2_BROWSING_DATA_KINDS_COOKIES,
        DataKind::Cache => COREWEBVIEW2_BROWSING_DATA_KINDS(
            COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE.0
                | COREWEBVIEW2_BROWSING_DATA_KINDS_CACHE_STORAGE.0,
        ),
        // Cookies, DOM storage, IndexedDB, service workers… plus the HTTP cache.
        DataKind::All => COREWEBVIEW2_BROWSING_DATA_KINDS(
            COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_SITE.0
                | COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE.0,
        ),
    }
}

fn clear_data(webview: &ICoreWebView2, kind: DataKind, privacy: &Privacy) {
    let profile = unsafe {
        webview
            .cast::<ICoreWebView2_13>()
            .and_then(|w| w.Profile())
            .and_then(|p| p.cast::<ICoreWebView2Profile2>())
    };
    match profile {
        Ok(profile) => {
            let inflight = privacy.inflight.clone();
            let cookies = privacy.cookies.clone();
            inflight.set(inflight.get() + 1);
            let done = inflight.clone();
            let handler = ClearBrowsingDataCompletedHandler::create(Box::new(move |_res| {
                done.set(done.get().saturating_sub(1));
                if kind != DataKind::Cache {
                    cookies.lock().unwrap().clear();
                }
                Ok(())
            }));
            let started =
                unsafe { profile.ClearBrowsingData(browsing_data_kinds(kind), &handler) };
            if let Err(e) = started {
                log_engine(&format!("clear data err: {e}"));
                inflight.set(inflight.get().saturating_sub(1));
            }
        }
        // Older runtimes without the profile API can still drop cookies.
        Err(_) if kind != DataKind::Cache => {
            delete_all_cookies(webview);
            privacy.cookies.lock().unwrap().clear();
        }
        Err(e) => log_engine(&format!("clear cache unsupported: {e}")),
    }
}

fn read_cookies(webview: &ICoreWebView2, privacy: &Privacy) {
    let Ok(cm) = (unsafe { webview.cast::<ICoreWebView2_2>().and_then(|w| w.CookieManager()) })
    else {
        return;
    };
    let out = privacy.cookies.clone();
    let handler = GetCookiesCompletedHandler::create(Box::new(
        move |res: windows_core::Result<()>, list: Option<ICoreWebView2CookieList>| {
            if let (Ok(()), Some(list)) = (res, list) {
                *out.lock().unwrap() = unsafe { cookie_infos(&list) };
            }
            Ok(())
        },
    ));
    // An empty URI lists every cookie in the profile.
    let empty = wide("");
    unsafe {
        let _ = cm.GetCookies(PCWSTR(empty.as_ptr()), &handler);
    }
}

unsafe fn cookie_infos(list: &ICoreWebView2CookieList) -> Vec<CookieInfo> {
    let mut n = 0u32;
    if list.Count(&mut n).is_err() {
        return Vec::new();
    }
    let mut v = Vec::with_capacity(n as usize);
    for i in 0..n {
        let Ok(c) = list.GetValueAtIndex(i) else {
            continue;
        };
        let mut name = PWSTR::null();
        let mut domain = PWSTR::null();
        let _ = c.Name(&mut name);
        let _ = c.Domain(&mut domain);
        let (mut secure, mut http_only, mut session) = (BOOL(0), BOOL(0), BOOL(0));
        let _ = c.IsSecure(&mut secure);
        let _ = c.IsHttpOnly(&mut http_only);
        let _ = c.IsSession(&mut session);
        v.push(CookieInfo {
            domain: take_pwstr(domain),
            name: take_pwstr(name),
            secure: secure.as_bool(),
            http_only: http_only.as_bool(),
            session_only: session.as_bool(),
        });
    }
    v.sort_by(|a, b| a.domain.cmp(&b.domain).then(a.name.cmp(&b.name)));
    v
}

/// Post a real mouse message into Chromium's render widget. Returns false when
/// the input window can't be found (caller should fall back to JS simulation).
unsafe fn post_mouse(
    host: HWND,
    render_hwnd: &Rc<Cell<HWND>>,
    mouse_down: &Rc<Cell<bool>>,
    kind: MouseKind,
    x: i32,
    y: i32,
) -> bool {
    let mut target = render_hwnd.get();
    if target.is_invalid() {
        match find_render_widget(host) {
            Some(h) => {
                render_hwnd.set(h);
                target = h;
            }
            None => return false,
        }
    }
    let lp = LPARAM((((y as u16 as u32) << 16) | (x as u16 as u32)) as isize);
    let (msg, wp) = match kind {
        MouseKind::Move => {
            let flags = if mouse_down.get() { MK_LBUTTON } else { 0 };
            (WM_MOUSEMOVE, WPARAM(flags))
        }
        MouseKind::Drag => (WM_MOUSEMOVE, WPARAM(MK_LBUTTON)),
        MouseKind::Down => {
            mouse_down.set(true);
            (WM_LBUTTONDOWN, WPARAM(MK_LBUTTON))
        }
        MouseKind::Up => {
            mouse_down.set(false);
            (WM_LBUTTONUP, WPARAM(0))
        }
    };
    if PostMessageW(Some(target), msg, wp, lp).is_err() {
        render_hwnd.set(HWND::default());
        return false;
    }
    true
}

unsafe fn find_render_widget(root: HWND) -> Option<HWND> {
    let mut found = HWND::default();
    unsafe extern "system" fn enum_fn(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let found = &mut *(lparam.0 as *mut HWND);
        if !found.is_invalid() {
            return BOOL(0);
        }
        let mut class = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut class);
        if len > 0 {
            let name = String::from_utf16_lossy(&class[..len as usize]);
            if name == "Chrome_RenderWidgetHostHWND" {
                *found = hwnd;
                return BOOL(0);
            }
        }
        let _ = EnumChildWindows(Some(hwnd), Some(enum_fn), lparam);
        if found.is_invalid() {
            BOOL(1)
        } else {
            BOOL(0)
        }
    }
    let _ = EnumChildWindows(
        Some(root),
        Some(enum_fn),
        LPARAM(&mut found as *mut HWND as isize),
    );
    if found.is_invalid() {
        None
    } else {
        Some(found)
    }
}

unsafe fn poll_navigation(
    webview: &ICoreWebView2,
    info: &Arc<Mutex<PageInfo>>,
    frame: &Arc<Mutex<Option<Arc<Frame>>>>,
    texts: &Arc<Mutex<Vec<TextRun>>>,
) {
    let mut p = PWSTR::null();
    if webview.Source(&mut p).is_err() {
        return;
    }
    let url = take_pwstr(p);
    if url.is_empty() {
        return;
    }
    let mut i = info.lock().unwrap();
    // Skip the very first URL assignment (empty → loaded) so we don't
    // wipe the initial paint; only real navigations clear state.
    if !i.url.is_empty() && i.url != url {
        log_engine(&format!("poll_navigation clear: '{}' -> '{}'", i.url, url));
        clear_page_state(frame, texts);
        i.url = url;
        let mut p2 = PWSTR::null();
        if webview.DocumentTitle(&mut p2).is_ok() {
            i.title = take_pwstr(p2);
        }
    } else if i.url.is_empty() {
        i.url = url;
        let mut p2 = PWSTR::null();
        if webview.DocumentTitle(&mut p2).is_ok() {
            i.title = take_pwstr(p2);
        }
    }
}

fn start_extract(webview: &ICoreWebView2, texts: Arc<Mutex<Vec<TextRun>>>) {
    let sw = wide(EXTRACT_JS);
    let handler = ExecuteScriptCompletedHandler::create(Box::new(
        move |res: windows_core::Result<()>, json: String| {
            if res.is_ok() {
                // the script returns a string, so the result JSON is
                // double-encoded: unwrap the outer string first
                if let Ok(inner) = serde_json::from_str::<String>(&json) {
                    *texts.lock().unwrap() = parse_text_runs(&inner);
                }
            }
            Ok(())
        },
    ));
    unsafe {
        let _ = webview.ExecuteScript(PCWSTR(sw.as_ptr()), &handler);
    }
}

fn exec_script(webview: &ICoreWebView2, script: &str) {
    let sw = wide(script);
    let handler = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
    unsafe {
        let _ = webview.ExecuteScript(PCWSTR(sw.as_ptr()), &handler);
    }
}

#[allow(clippy::too_many_arguments)]
fn start_capture(
    webview: &ICoreWebView2,
    frame: Arc<Mutex<Option<Arc<Frame>>>>,
    info: Arc<Mutex<PageInfo>>,
    capturing: Rc<Cell<bool>>,
    cap_id: Rc<Cell<u64>>,
    my_id: u64,
    gen: u64,
) {
    unsafe {
        // refresh page info while we're here — cheap COM getters
        {
            let mut p = PWSTR::null();
            let src = webview.Source(&mut p);
            if src.is_ok() {
                let url = take_pwstr(p);
                let mut i = info.lock().unwrap();
                // Don't stomp a newer navigation's URL with a stale read.
                if super::page_gen() == gen {
                    i.url = url;
                }
            }
            let mut p2 = PWSTR::null();
            let tit = webview.DocumentTitle(&mut p2);
            if tit.is_ok() {
                let title = take_pwstr(p2);
                if super::page_gen() == gen {
                    info.lock().unwrap().title = title;
                }
            }
            if std::env::var_os("TEXTTOP_ENGINE_LOG").is_some() {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(std::env::temp_dir().join("texttop-engine.log"))
                {
                    let i = info.lock().unwrap();
                    let _ = writeln!(
                        f,
                        "src_ok={} tit_ok={} url={} title={}",
                        src.is_ok(),
                        tit.is_ok(),
                        i.url,
                        i.title
                    );
                }
            }
        }
        let stream: IStream = match CreateStreamOnHGlobal(HGLOBAL::default(), true) {
            Ok(s) => s,
            Err(_) => {
                capturing.set(false);
                return;
            }
        };
        let stream_cb = stream.clone();
        let capturing_cb = capturing.clone();
        let cap_id_cb = cap_id.clone();
        let handler = CapturePreviewCompletedHandler::create(Box::new(
            move |res: windows_core::Result<()>| {
                if let Err(e) = &res {
                    log_engine(&format!("capture failed: {e}"));
                }
                if cap_id_cb.get() != my_id {
                    // watchdog already gave up on this capture; a newer one
                    // owns the `capturing` flag now
                    return Ok(());
                }
                if res.is_ok() && super::page_gen() == gen {
                    if let Some(png) = read_stream(&stream_cb) {
                        if let Ok(img) = image::load_from_memory(&png) {
                            // composite any alpha over white so transparent
                            // regions render as page background, not black
                            let rgba = img.to_rgba8();
                            let (w, h) = rgba.dimensions();
                            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
                            for px in rgba.pixels() {
                                let a = px[3] as u32;
                                for c in 0..3 {
                                    rgb.push(
                                        ((px[c] as u32 * a + 255 * (255 - a)) / 255) as u8,
                                    );
                                }
                            }
                            *frame.lock().unwrap() = Some(Arc::new(Frame {
                                w,
                                h,
                                rgb,
                                seq: super::next_seq(),
                            }));
                        }
                    }
                }
                capturing_cb.set(false);
                Ok(())
            },
        ));
        if webview
            .CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG, &stream, &handler)
            .is_err()
        {
            capturing.set(false);
        }
    }
}

unsafe fn read_stream(s: &IStream) -> Option<Vec<u8>> {
    s.Seek(0, STREAM_SEEK_SET, None).ok()?;
    let mut out = Vec::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let mut read: u32 = 0;
        let hr = s.Read(
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
            Some(&mut read),
        );
        if read > 0 {
            out.extend_from_slice(&buf[..read as usize]);
        }
        if read == 0 || hr.is_err() {
            break;
        }
    }
    (!out.is_empty()).then_some(out)
}
