//! App Host: mirror a real desktop application into a texttop window.
//! The target window is parked off-screen, its client area is captured
//! with PrintWindow (~10 fps) and quadrant-rendered into cells; mouse and
//! keyboard input are posted back as Win32 messages. Windows only.

#[cfg(all(windows, feature = "apphost"))]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;
    use ratatui::Frame;

    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, MapWindowPoints,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_VSC};
    use windows::Win32::UI::WindowsAndMessaging::{
        ChildWindowFromPointEx, EnumWindows, GetClientRect, GetGUIThreadInfo, GetWindowLongW,
        GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
        IsWindowVisible, PostMessageW, SetWindowPos, ShowWindow, CWP_SKIPDISABLED,
        CWP_SKIPINVISIBLE, GUITHREADINFO, GWL_EXSTYLE, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWNOACTIVATE, WM_CHAR, WM_KEYDOWN, WM_KEYUP,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WS_EX_TOOLWINDOW,
    };
    use windows_core::BOOL;

    use crate::apps::ui::{Controls, Item, Look};
    use crate::apps::{Action, AppWindow};
    use crate::engine::{next_seq, Frame as PixFrame, MouseKind};
    use crate::gfx;

    const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    /// Opt-in debug log (`TEXTTOP_ENGINE_LOG=1`) → %TEMP%\texttop-apphost.log
    fn log_host(msg: &str) {
        if std::env::var_os("TEXTTOP_ENGINE_LOG").is_none() {
            return;
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("texttop-apphost.log"))
        {
            let _ = writeln!(f, "{msg}");
        }
    }

    // terminal cell ≈ 8×16 px (same scale the web browser uses)
    const CELL_PX_W: i32 = 8;
    const CELL_PX_H: i32 = 16;

    /// Win32 MK_LBUTTON — left button held during mouse messages.
    const MK_LBUTTON: usize = 0x0001;
    /// PW_CLIENTONLY | PW_RENDERFULLCONTENT — capture the client area of
    /// DWM-composited windows even while they are occluded / off-screen.
    const PW_FULL: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(0x1 | 0x2);

    // ── window enumeration ──────────────────────────────────────────────

    #[derive(Clone)]
    pub struct WinEntry {
        pub hwnd: isize,
        pub title: String,
        pub exe: String,
        pub pid: u32,
    }

    fn list_windows() -> Vec<WinEntry> {
        let mut v: Vec<WinEntry> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(enum_cb), LPARAM(&mut v as *mut _ as isize));
        }
        v
    }

    extern "system" fn enum_cb(hwnd: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(lp.0 as *mut Vec<WinEntry>);
            if !IsWindowVisible(hwnd).as_bool() {
                return true.into();
            }
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 {
                return true.into();
            }
            // skip cloaked windows (suspended UWP shells etc.)
            let mut cloaked: u32 = 0;
            let _ = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloaked as *mut u32 as *mut _,
                std::mem::size_of::<u32>() as u32,
            );
            if cloaked != 0 {
                return true.into();
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(hwnd, &mut buf);
            if n <= 0 {
                return true.into();
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == GetCurrentProcessId() {
                return true.into();
            }
            out.push(WinEntry {
                hwnd: hwnd.0 as isize,
                title: String::from_utf16_lossy(&buf[..n as usize]),
                exe: exe_name(pid),
                pid,
            });
            true.into()
        }
    }

    fn exe_name(pid: u32) -> String {
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return String::new();
            };
            let mut buf = [0u16; 512];
            let mut len = buf.len() as u32;
            let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
            let _ = CloseHandle(h);
            if r.is_err() {
                return String::new();
            }
            String::from_utf16_lossy(&buf[..len as usize])
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or("")
                .trim_end_matches(".exe")
                .to_string()
        }
    }

    // ── capture / input backend (runs on its own thread) ────────────────

    enum HostCmd {
        ResizePx(i32, i32),
        Mouse { kind: MouseKind, x: i32, y: i32 },
        Wheel { delta: i32, x: i32, y: i32 },
        Char(char),
        Key(u32),
        Stop,
    }

    struct Host {
        tx: Sender<HostCmd>,
        frame: Arc<Mutex<Option<Arc<PixFrame>>>>,
        title: Arc<Mutex<String>>,
        gone: Arc<AtomicBool>,
        hwnd: isize,
        orig: Option<RECT>,
        spot: (i32, i32),
        parked: bool,
    }

    impl Host {
        fn start(hwnd: isize, title0: String) -> Host {
            // park from this thread, before capture starts, so Drop can
            // restore synchronously — a detached thread may not get to run
            // its cleanup when the whole process is quitting
            let spot = park_spot();
            let orig = unsafe { park(HWND(hwnd as *mut _), spot) };
            let (tx, rx) = mpsc::channel();
            let frame: Arc<Mutex<Option<Arc<PixFrame>>>> = Arc::new(Mutex::new(None));
            let title = Arc::new(Mutex::new(title0));
            let gone = Arc::new(AtomicBool::new(false));
            {
                let frame = frame.clone();
                let title = title.clone();
                let gone = gone.clone();
                std::thread::spawn(move || run_host(hwnd, rx, frame, title, gone));
            }
            Host {
                tx,
                frame,
                title,
                gone,
                hwnd,
                orig,
                spot,
                parked: true,
            }
        }

        fn send(&self, c: HostCmd) {
            let _ = self.tx.send(c);
        }

        fn frame(&self) -> Option<Arc<PixFrame>> {
            self.frame.lock().unwrap().clone()
        }

        fn title(&self) -> String {
            self.title.lock().unwrap().clone()
        }

        fn gone(&self) -> bool {
            self.gone.load(Ordering::Relaxed)
        }

        /// Toggle between living off-screen and showing on the real desktop.
        fn set_parked(&mut self, want: bool) {
            if want == self.parked {
                return;
            }
            let hwnd = HWND(self.hwnd as *mut _);
            let (x, y) = if want {
                self.spot
            } else {
                match &self.orig {
                    Some(r) => (r.left, r.top),
                    None => return,
                }
            };
            unsafe {
                let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            self.parked = want;
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = self.tx.send(HostCmd::Stop);
            if !self.gone() && self.parked {
                if let Some(r) = self.orig {
                    unsafe { unpark(HWND(self.hwnd as *mut _), r) };
                }
            }
        }
    }

    fn run_host(
        hwnd_raw: isize,
        rx: Receiver<HostCmd>,
        frame: Arc<Mutex<Option<Arc<PixFrame>>>>,
        title: Arc<Mutex<String>>,
        gone: Arc<AtomicBool>,
    ) {
        let hwnd = HWND(hwnd_raw as *mut _);
        let mut last_cap = Instant::now() - Duration::from_secs(1);
        let mut last_title = Instant::now();
        'outer: loop {
            loop {
                match rx.try_recv() {
                    Ok(HostCmd::Stop) | Err(TryRecvError::Disconnected) => break 'outer,
                    Ok(cmd) => unsafe { handle_cmd(hwnd, cmd) },
                    Err(TryRecvError::Empty) => break,
                }
            }
            unsafe {
                if !IsWindow(Some(hwnd)).as_bool() {
                    gone.store(true, Ordering::Relaxed);
                    break;
                }
                if last_cap.elapsed() >= Duration::from_millis(100) {
                    last_cap = Instant::now();
                    if let Some((rgb, w, h)) = capture(hwnd) {
                        *frame.lock().unwrap() = Some(Arc::new(PixFrame {
                            w,
                            h,
                            rgb,
                            seq: next_seq(),
                        }));
                    }
                }
                if last_title.elapsed() >= Duration::from_secs(1) {
                    last_title = Instant::now();
                    let mut buf = [0u16; 256];
                    let n = GetWindowTextW(hwnd, &mut buf);
                    if n > 0 {
                        *title.lock().unwrap() = String::from_utf16_lossy(&buf[..n as usize]);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    unsafe fn handle_cmd(hwnd: HWND, cmd: HostCmd) {
        match cmd {
            HostCmd::ResizePx(w, h) => {
                let mut wr = RECT::default();
                let mut cr = RECT::default();
                if GetWindowRect(hwnd, &mut wr).is_ok() && GetClientRect(hwnd, &mut cr).is_ok() {
                    let dw = (wr.right - wr.left) - cr.right;
                    let dh = (wr.bottom - wr.top) - cr.bottom;
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        0,
                        0,
                        w + dw,
                        h + dh,
                        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            HostCmd::Mouse { kind, x, y } => send_mouse(hwnd, kind, x, y),
            HostCmd::Wheel { delta, x, y } => send_wheel(hwnd, delta, x, y),
            HostCmd::Char(c) => send_char(hwnd, c),
            // key down/up only — the app's own TranslateMessage produces the
            // WM_CHAR, so posting one ourselves would double the input
            HostCmd::Key(vk) => {
                let target = focus_target(hwnd);
                post_key(target, vk, true);
                post_key(target, vk, false);
            }
            HostCmd::Stop => {}
        }
    }

    /// Parking spot for a hosted window. Each host gets its own coordinates:
    /// windows stacked at one point break WindowFromPoint-based click
    /// validation (WinForms), and -32000 is already crowded — minimized
    /// windows and the WebView2 engine window live there.
    fn park_spot() -> (i32, i32) {
        static SLOT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
        let n = SLOT.fetch_add(1, Ordering::Relaxed).rem_euclid(8);
        (-24000 - n * 4000, -24000)
    }

    /// Remember the window's placement, un-minimize if needed, and move it
    /// off-screen so it lives inside the terminal instead of the desktop.
    unsafe fn park(hwnd: HWND, spot: (i32, i32)) -> Option<RECT> {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).ok()?;
        let _ = SetWindowPos(
            hwnd,
            None,
            spot.0,
            spot.1,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        Some(r)
    }

    unsafe fn unpark(hwnd: HWND, r: RECT) {
        let _ = SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }

    /// PrintWindow the client area into a top-down 32-bit DIB → RGB24.
    unsafe fn capture(hwnd: HWND) -> Option<(Vec<u8>, u32, u32)> {
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc).ok()?;
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w < 8 || h < 8 {
            return None;
        }
        let mdc = CreateCompatibleDC(None);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let bmp = match CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(_) => {
                let _ = DeleteDC(mdc);
                return None;
            }
        };
        let old = SelectObject(mdc, bmp.into());
        let ok = PrintWindow(hwnd, mdc, PW_FULL).as_bool();
        let _ = GdiFlush();
        let mut out = None;
        if ok && !bits.is_null() {
            let n = (w * h) as usize;
            let src = std::slice::from_raw_parts(bits as *const u8, n * 4);
            let mut rgb = Vec::with_capacity(n * 3);
            for px in src.chunks_exact(4) {
                rgb.extend_from_slice(&[px[2], px[1], px[0]]); // BGRA → RGB
            }
            out = Some((rgb, w as u32, h as u32));
        }
        SelectObject(mdc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mdc);
        out
    }

    /// Walk down to the innermost child under a client-area point so input
    /// lands on the control that owns it, not the top-level frame.
    unsafe fn deepest_child(hwnd: HWND, mut pt: POINT) -> (HWND, POINT) {
        let mut cur = hwnd;
        for _ in 0..16 {
            let child = ChildWindowFromPointEx(cur, pt, CWP_SKIPINVISIBLE | CWP_SKIPDISABLED);
            if child.is_invalid() || child == cur {
                break;
            }
            let mut pts = [pt];
            MapWindowPoints(Some(cur), Some(child), &mut pts);
            pt = pts[0];
            cur = child;
        }
        (cur, pt)
    }

    fn make_lparam(x: i32, y: i32) -> LPARAM {
        LPARAM((((y as i16 as u16 as u32) << 16) | (x as i16 as u16 as u32)) as isize)
    }

    unsafe fn send_mouse(hwnd: HWND, kind: MouseKind, x: i32, y: i32) {
        let (target, pt) = deepest_child(hwnd, POINT { x, y });
        let lp = make_lparam(pt.x, pt.y);
        log_host(&format!(
            "mouse kind={} src=({x},{y}) target={:?} pt=({},{})",
            match kind {
                MouseKind::Move => "move",
                MouseKind::Drag => "drag",
                MouseKind::Down => "down",
                MouseKind::Up => "up",
            },
            target.0,
            pt.x,
            pt.y
        ));
        let (msg, wp) = match kind {
            MouseKind::Move => (WM_MOUSEMOVE, WPARAM(0)),
            MouseKind::Drag => (WM_MOUSEMOVE, WPARAM(MK_LBUTTON)),
            MouseKind::Down => {
                // hover state first — controls (WinForms etc.) only honor a
                // click after they have seen the pointer over them
                let _ = PostMessageW(Some(target), WM_MOUSEMOVE, WPARAM(0), lp);
                (WM_LBUTTONDOWN, WPARAM(MK_LBUTTON))
            }
            MouseKind::Up => (WM_LBUTTONUP, WPARAM(0)),
        };
        let _ = PostMessageW(Some(target), msg, wp, lp);
    }

    unsafe fn send_wheel(hwnd: HWND, delta: i32, x: i32, y: i32) {
        // WM_MOUSEWHEEL wants screen coordinates
        let (target, _) = deepest_child(hwnd, POINT { x, y });
        let mut wr = RECT::default();
        let _ = GetWindowRect(hwnd, &mut wr);
        let (sx, sy) = (wr.left + x, wr.top + y);
        let wp = WPARAM(((delta as i16 as u16 as usize) << 16) as usize);
        let _ = PostMessageW(Some(target), WM_MOUSEWHEEL, wp, make_lparam(sx, sy));
    }

    /// Keyboard goes to whatever child window has the target thread's focus —
    /// GetGUIThreadInfo works even while the app is in the background.
    unsafe fn focus_target(hwnd: HWND) -> HWND {
        let tid = GetWindowThreadProcessId(hwnd, None);
        let mut gti = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(tid, &mut gti).is_ok() && !gti.hwndFocus.is_invalid() {
            gti.hwndFocus
        } else {
            hwnd
        }
    }

    unsafe fn post_key(target: HWND, vk: u32, down: bool) {
        let scan = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) as isize;
        let mut lp = 1isize | (scan << 16);
        if !down {
            lp |= 3 << 30; // previous-state + transition bits
        }
        let msg = if down { WM_KEYDOWN } else { WM_KEYUP };
        let _ = PostMessageW(Some(target), msg, WPARAM(vk as usize), LPARAM(lp));
    }

    /// Printable characters go as bare WM_CHAR — pairing them with a
    /// keydown would double the character once the app translates it.
    unsafe fn send_char(hwnd: HWND, c: char) {
        let target = focus_target(hwnd);
        let mut units = [0u16; 2];
        for u in c.encode_utf16(&mut units).iter() {
            let _ = PostMessageW(Some(target), WM_CHAR, WPARAM(*u as usize), LPARAM(1));
        }
    }

    // ── the TUI app ─────────────────────────────────────────────────────

    /// Split "C:\Program Files\app.exe --flag" into program and args.
    /// Quoted programs are honored; otherwise the longest whitespace-prefix
    /// that names an existing file wins (paths with spaces, unquoted).
    fn parse_cmd(s: &str) -> Option<(std::path::PathBuf, Vec<String>)> {
        let t = s.trim();
        if t.is_empty() {
            return None;
        }
        let words = |r: &str| r.split_whitespace().map(String::from).collect::<Vec<_>>();
        if let Some(rest) = t.strip_prefix('"') {
            if let Some(end) = rest.find('"') {
                return Some(((&rest[..end]).into(), words(&rest[end + 1..])));
            }
        }
        let p = std::path::Path::new(t);
        if p.is_file() {
            return Some((p.into(), Vec::new()));
        }
        let mut best = None;
        for (i, ch) in t.char_indices() {
            if ch.is_whitespace() && std::path::Path::new(&t[..i]).is_file() {
                best = Some(i);
            }
        }
        if let Some(i) = best {
            return Some(((&t[..i]).into(), words(&t[i..])));
        }
        let mut it = t.split_whitespace();
        let exe = it.next()?.to_string();
        Some((exe.into(), it.map(String::from).collect()))
    }

    enum State {
        Pick {
            list: Vec<WinEntry>,
            sel: usize,
            off: usize,
            /// launch-command input line (`l` key), None when browsing
            input: Option<String>,
        },
        /// Launched a program; waiting for its first window to appear.
        Wait {
            exe_base: String,
            pid: u32,
            since: Instant,
            last_poll: Instant,
            /// windows that existed before the launch — never auto-adopted
            prev: Vec<isize>,
        },
        Host {
            host: Host,
            last_px: (i32, i32),
            zoom: f64,
            /// crop center in source pixels (zoom > 1 only)
            focus: (f64, f64),
            autosize: bool,
            /// last rendered mapping: (dest cells, crop x/y/w/h in source px)
            view: Option<(Rect, (u32, u32, u32, u32))>,
            crop_buf: Vec<u8>,
        },
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Btn {
        Rescan,
        LaunchPrompt,
        LaunchOk,
        LaunchCancel,
        Adopt,
        Close,
        StopWaiting,
        Windows,
        Park,
        Autosize,
        ZoomOut,
        ZoomIn,
    }

    pub struct AppHost {
        state: State,
        t: u64,
        cache: gfx::RgbCache,
        note: String,
        ui: Controls<Btn>,
    }

    impl AppHost {
        /// Open the picker. A restored session command is shown in the launch
        /// field so the user confirms before anything is spawned.
        pub fn with_cmd(cmd: Option<String>) -> Self {
            let (input, note) = match cmd {
                Some(c) if !c.is_empty() => (
                    Some(c),
                    "session restore — click Launch, or pick a window".into(),
                ),
                _ => (None, String::new()),
            };
            AppHost {
                state: State::Pick {
                    list: list_windows(),
                    sel: 0,
                    off: 0,
                    input,
                },
                t: 0,
                cache: gfx::RgbCache::default(),
                note,
                ui: Controls::new(),
            }
        }

        fn run(&mut self, b: Btn) -> Action {
            match (&mut self.state, b) {
                (State::Pick { list, sel, .. }, Btn::Rescan) => {
                    *list = list_windows();
                    *sel = (*sel).min(list.len().saturating_sub(1));
                }
                (State::Pick { input, .. }, Btn::LaunchPrompt) => *input = Some(String::new()),
                (State::Pick { input, .. }, Btn::LaunchOk) => {
                    if let Some(cmd) = input.take() {
                        self.launch(&cmd);
                    }
                }
                (State::Pick { input, .. }, Btn::LaunchCancel) => *input = None,
                (State::Pick { list, sel, .. }, Btn::Adopt) => {
                    if let Some(e) = list.get(*sel).cloned() {
                        self.adopt(&e);
                    }
                }
                (_, Btn::Close) => return Action::Close,
                (State::Wait { .. }, Btn::StopWaiting) | (State::Host { .. }, Btn::Windows) => {
                    self.release()
                }
                (State::Host { host, .. }, Btn::Park) => {
                    let want = !host.parked;
                    host.set_parked(want);
                    self.note = if want {
                        "window hidden from desktop".into()
                    } else {
                        "window shown on desktop (capture continues)".into()
                    };
                }
                (State::Host { autosize, .. }, Btn::Autosize) => {
                    *autosize = !*autosize;
                    self.note = if *autosize {
                        "auto-resize ON".into()
                    } else {
                        "auto-resize OFF (native size, letterboxed)".into()
                    };
                }
                (State::Host { zoom, .. }, Btn::ZoomOut) => {
                    *zoom = (*zoom - 0.5).max(1.0);
                    self.cache.clear();
                }
                (State::Host { zoom, .. }, Btn::ZoomIn) => {
                    *zoom = (*zoom + 0.5).min(6.0);
                    self.cache.clear();
                }
                _ => {}
            }
            Action::None
        }

        /// Spawn a program and start waiting for its window.
        fn launch(&mut self, cmdline: &str) {
            let Some((exe, args)) = parse_cmd(cmdline) else {
                return;
            };
            let base = exe
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut cmd = std::process::Command::new(&exe);
            cmd.args(&args);
            if let Some(dir) = exe.parent().filter(|d| !d.as_os_str().is_empty()) {
                cmd.current_dir(dir);
            }
            match cmd.spawn() {
                Ok(child) => {
                    self.note.clear();
                    self.state = State::Wait {
                        exe_base: base,
                        pid: child.id(),
                        since: Instant::now(),
                        last_poll: Instant::now() - Duration::from_secs(1),
                        prev: list_windows().iter().map(|e| e.hwnd).collect(),
                    };
                }
                Err(e) => self.note = format!("launch failed: {e}"),
            }
        }

        fn adopt(&mut self, e: &WinEntry) {
            self.cache.clear();
            self.note.clear();
            self.state = State::Host {
                host: Host::start(e.hwnd, e.title.clone()),
                last_px: (0, 0),
                zoom: 1.0,
                focus: (0.0, 0.0),
                autosize: true,
                view: None,
                crop_buf: Vec::new(),
            };
        }

        fn release(&mut self) {
            self.cache.clear();
            self.state = State::Pick {
                list: list_windows(),
                sel: 0,
                off: 0,
                input: None,
            };
        }
    }

    /// Map a mouse event through the letterbox/crop view back to source px.
    fn view_px(
        m: &MouseEvent,
        view: &Option<(Rect, (u32, u32, u32, u32))>,
        clamp: bool,
    ) -> Option<(i32, i32)> {
        let (dest, (cx, cy, cw, ch)) = view.as_ref()?;
        if dest.width == 0 || dest.height == 0 {
            return None;
        }
        let inside = m.column >= dest.x
            && m.column < dest.x + dest.width
            && m.row >= dest.y
            && m.row < dest.y + dest.height;
        if !inside && !clamp {
            return None;
        }
        let col = m.column.clamp(dest.x, dest.x + dest.width - 1);
        let row = m.row.clamp(dest.y, dest.y + dest.height - 1);
        let fx = (col - dest.x) as f64 + 0.5;
        let fy = (row - dest.y) as f64 + 0.5;
        let x = *cx as f64 + fx / dest.width as f64 * *cw as f64;
        let y = *cy as f64 + fy / dest.height as f64 * *ch as f64;
        Some((x as i32, y as i32))
    }

    /// Fit a crop of `cw×ch` source px into `content` preserving the on-screen
    /// aspect ratio (cells are ~8×16 px).
    fn fit_rect(content: Rect, cw: u32, ch: u32) -> Rect {
        let sx = content.width as f64 * CELL_PX_W as f64 / cw as f64;
        let sy = content.height as f64 * CELL_PX_H as f64 / ch as f64;
        let s = sx.min(sy);
        let w = ((cw as f64 * s / CELL_PX_W as f64).round() as u16).clamp(1, content.width);
        let h = ((ch as f64 * s / CELL_PX_H as f64).round() as u16).clamp(1, content.height);
        Rect::new(
            content.x + (content.width - w) / 2,
            content.y + (content.height - h) / 2,
            w,
            h,
        )
    }

    impl AppWindow for AppHost {
        fn title(&self) -> String {
            match &self.state {
                State::Pick { .. } => "App Host".into(),
                State::Wait { exe_base, .. } => format!("App Host · {exe_base}"),
                State::Host { host, .. } => {
                    let t = host.title();
                    if t.is_empty() {
                        "App Host".into()
                    } else {
                        t.chars().take(36).collect()
                    }
                }
            }
        }

        fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
            if area.height < 2 {
                return;
            }
            self.ui.begin();
            let content = Rect::new(area.x, area.y, area.width, area.height - 1);
            let status = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
            let dim = if focused {
                Color::Rgb(140, 150, 175)
            } else {
                Color::Rgb(100, 105, 125)
            };

            match &mut self.state {
                State::Pick { list, sel, off, input } => {
                    gfx::fill(f.buffer_mut(), content, Color::Rgb(12, 14, 22));
                    let buf = f.buffer_mut();
                    self.ui.toolbar(
                        buf,
                        Rect::new(content.x, content.y, content.width, 1),
                        &[Item::Label(" Click a desktop window to show it here".into(), Color::Rgb(150, 200, 255))],
                        &[
                            Item::btn("⟳ Rescan", Btn::Rescan),
                            Item::btn("▶ Launch program…", Btn::LaunchPrompt),
                        ],
                    );
                    let rows = content.height.saturating_sub(2) as usize;
                    if *sel < *off {
                        *off = *sel;
                    }
                    if rows > 0 && *sel >= *off + rows {
                        *off = *sel + 1 - rows;
                    }
                    for (i, e) in list.iter().enumerate().skip(*off).take(rows) {
                        let y = content.y + 2 + (i - *off) as u16;
                        let selected = i == *sel;
                        let (fg, bg) = if selected {
                            (Color::Rgb(240, 245, 255), Color::Rgb(0, 70, 110))
                        } else {
                            (Color::Rgb(185, 190, 205), Color::Rgb(12, 14, 22))
                        };
                        let line = format!(
                            " {:<14} {}",
                            e.exe.chars().take(14).collect::<String>(),
                            e.title
                        );
                        let line: String = line
                            .chars()
                            .take(content.width.saturating_sub(2) as usize)
                            .collect();
                        buf.set_string(
                            content.x + 1,
                            y,
                            format!("{line:<w$}", w = content.width.saturating_sub(2) as usize),
                            Style::default().fg(fg).bg(bg),
                        );
                    }
                    if list.is_empty() {
                        buf.set_string(
                            content.x + 2,
                            content.y + 2,
                            "no capturable windows found — click ⟳ Rescan",
                            Style::default().fg(Color::Rgb(255, 170, 90)).bg(Color::Rgb(12, 14, 22)),
                        );
                    }
                    match input {
                        Some(cmd) => self.ui.input_row(
                            buf,
                            status,
                            "Program",
                            cmd,
                            ("Launch", Btn::LaunchOk),
                            Btn::LaunchCancel,
                        ),
                        None => {
                            self.ui.toolbar(
                                buf,
                                status,
                                &[Item::Label(format!(" {}", self.note), dim)],
                                &[
                                    Item::with("Show here", Btn::Adopt, if list.is_empty() { Look::Disabled } else { Look::Primary }),
                                    Item::with("Close", Btn::Close, Look::Raised),
                                ],
                            );
                        }
                    }
                }
                State::Wait { exe_base, .. } => {
                    gfx::fill(f.buffer_mut(), content, Color::Rgb(12, 14, 22));
                    let spin = SPINNER[(self.t / 2) as usize % SPINNER.len()];
                    let msg = format!("{spin}  waiting for a {exe_base} window …");
                    let y = content.y + content.height / 2;
                    let x =
                        content.x + content.width.saturating_sub(msg.chars().count() as u16) / 2;
                    f.render_widget(
                        Paragraph::new(Span::styled(
                            msg,
                            Style::default()
                                .fg(Color::Rgb(0, 190, 255))
                                .add_modifier(Modifier::BOLD),
                        )),
                        Rect::new(x, y, content.width.saturating_sub(x - content.x), 1),
                    );
                    self.ui.toolbar(
                        f.buffer_mut(),
                        status,
                        &[],
                        &[Item::with("Cancel", Btn::StopWaiting, Look::Raised)],
                    );
                }
                State::Host {
                    host,
                    last_px,
                    zoom,
                    focus,
                    autosize,
                    view,
                    crop_buf,
                } => {
                    gfx::fill(f.buffer_mut(), content, Color::Rgb(4, 4, 10));
                    if host.gone() {
                        f.render_widget(
                            Paragraph::new(vec![
                                Line::default(),
                                Line::from(Span::styled(
                                    "  ✗ the application window was closed",
                                    Style::default().fg(Color::Rgb(255, 110, 110)),
                                )),
                                Line::from(Span::styled(
                                    "    click 🪟 Windows below to pick another one",
                                    Style::default().fg(Color::Rgb(150, 155, 175)),
                                )),
                            ]),
                            content,
                        );
                        *view = None;
                    } else if let Some(fr) = host.frame() {
                        // keep the real window sized to the cell grid
                        if *autosize {
                            let want = (
                                content.width as i32 * CELL_PX_W,
                                content.height as i32 * CELL_PX_H,
                            );
                            if want != *last_px && want.0 >= 200 && want.1 >= 150 {
                                *last_px = want;
                                host.send(HostCmd::ResizePx(want.0, want.1));
                            }
                        }
                        let (iw, ih) = (fr.w, fr.h);
                        let z = zoom.max(1.0);
                        let cw = ((iw as f64 / z) as u32).clamp(8, iw);
                        let ch = ((ih as f64 / z) as u32).clamp(8, ih);
                        if focus.0 <= 0.0 && focus.1 <= 0.0 {
                            *focus = (iw as f64 / 2.0, ih as f64 / 2.0);
                        }
                        let cx = (focus.0 - cw as f64 / 2.0)
                            .clamp(0.0, (iw - cw) as f64) as u32;
                        let cy = (focus.1 - ch as f64 / 2.0)
                            .clamp(0.0, (ih - ch) as f64) as u32;
                        let dest = fit_rect(content, cw, ch);
                        *view = Some((dest, (cx, cy, cw, ch)));
                        if cw == iw && ch == ih {
                            gfx::draw_rgb_cached(
                                f.buffer_mut(),
                                dest,
                                &fr.rgb,
                                iw,
                                ih,
                                fr.seq as usize,
                                &mut self.cache,
                            );
                        } else {
                            // crop for zoom — copy the sub-rect row by row
                            crop_buf.clear();
                            crop_buf.reserve((cw * ch * 3) as usize);
                            for y in cy..cy + ch {
                                let s = ((y * iw + cx) * 3) as usize;
                                let e = s + (cw * 3) as usize;
                                if e <= fr.rgb.len() {
                                    crop_buf.extend_from_slice(&fr.rgb[s..e]);
                                }
                            }
                            let id = fr.seq as usize ^ ((cx as usize) << 20) ^ ((cy as usize) << 40);
                            gfx::draw_rgb_cached(
                                f.buffer_mut(),
                                dest,
                                crop_buf,
                                cw,
                                ch,
                                id,
                                &mut self.cache,
                            );
                        }
                    } else {
                        let spin = SPINNER[(self.t / 2) as usize % SPINNER.len()];
                        let msg = format!("{spin}  capturing window …");
                        let y = content.y + content.height / 2;
                        let x = content.x
                            + content.width.saturating_sub(msg.chars().count() as u16) / 2;
                        f.render_widget(
                            Paragraph::new(Span::styled(
                                msg,
                                Style::default()
                                    .fg(Color::Rgb(0, 190, 255))
                                    .add_modifier(Modifier::BOLD),
                            )),
                            Rect::new(x, y, content.width.saturating_sub(x - content.x), 1),
                        );
                        *view = None;
                    }
                    let left = if !self.note.is_empty() {
                        self.note.clone()
                    } else {
                        host.title()
                    };
                    self.ui.toolbar(
                        f.buffer_mut(),
                        status,
                        &[
                            Item::btn("🪟 Windows", Btn::Windows),
                            Item::with(
                                if host.parked { "👁 Show on desktop" } else { "👁 Hide from desktop" },
                                Btn::Park,
                                Look::on_if(!host.parked),
                            ),
                            Item::with("⤢ Auto-size", Btn::Autosize, Look::on_if(*autosize)),
                            Item::Sep,
                            Item::with("−", Btn::ZoomOut, Look::enabled_if(*zoom > 1.0)),
                            Item::Label(format!("{:.0}%", *zoom * 100.0), dim),
                            Item::with("+", Btn::ZoomIn, Look::enabled_if(*zoom < 6.0)),
                        ],
                        &[Item::Label(left.chars().take(32).collect::<String>(), dim)],
                    );
                }
            }
        }

        fn on_key(&mut self, k: KeyEvent) -> Action {
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            self.note.clear();
            match &mut self.state {
                State::Pick { list, sel, input, .. } => {
                    if let Some(cmd) = input {
                        match k.code {
                            KeyCode::Esc => *input = None,
                            KeyCode::Backspace => {
                                cmd.pop();
                            }
                            KeyCode::Enter => {
                                let cmd = cmd.clone();
                                *input = None;
                                self.launch(&cmd);
                            }
                            KeyCode::Char(c) if !ctrl => cmd.push(c),
                            _ => {}
                        }
                        return Action::None;
                    }
                    match k.code {
                        KeyCode::Esc | KeyCode::Char('q') => return Action::Close,
                        KeyCode::Up => *sel = sel.saturating_sub(1),
                        KeyCode::Down => {
                            if *sel + 1 < list.len() {
                                *sel += 1;
                            }
                        }
                        KeyCode::Char('r') => {
                            *list = list_windows();
                            *sel = (*sel).min(list.len().saturating_sub(1));
                        }
                        KeyCode::Char('l') => *input = Some(String::new()),
                        KeyCode::Enter => {
                            if let Some(e) = list.get(*sel).cloned() {
                                self.adopt(&e);
                            }
                        }
                        _ => {}
                    }
                }
                State::Wait { .. } => {
                    if matches!(k.code, KeyCode::Esc | KeyCode::Char('q')) {
                        self.release();
                    }
                }
                State::Host {
                    host,
                    zoom,
                    autosize,
                    ..
                } => match k.code {
                    KeyCode::F(2) => self.release(),
                    KeyCode::F(3) => {
                        let want = !host.parked;
                        host.set_parked(want);
                        self.note = if want {
                            "window hidden from desktop".into()
                        } else {
                            "window shown on desktop (capture continues)".into()
                        };
                    }
                    // F1/F5/F6/F11/F12 are taken by the desktop itself
                    KeyCode::F(8) => {
                        *autosize = !*autosize;
                        self.note = if *autosize {
                            "auto-resize ON".into()
                        } else {
                            "auto-resize OFF (native size, letterboxed)".into()
                        };
                    }
                    KeyCode::F(9) => {
                        *zoom = (*zoom - 0.5).max(1.0);
                        self.cache.clear();
                    }
                    KeyCode::F(10) => {
                        *zoom = (*zoom + 0.5).min(6.0);
                        self.cache.clear();
                    }
                    KeyCode::Enter => host.send(HostCmd::Key(0x0D)),
                    KeyCode::Backspace => host.send(HostCmd::Key(0x08)),
                    KeyCode::Tab => host.send(HostCmd::Key(0x09)),
                    KeyCode::Esc => host.send(HostCmd::Key(0x1B)),
                    KeyCode::Left => host.send(HostCmd::Key(0x25)),
                    KeyCode::Up => host.send(HostCmd::Key(0x26)),
                    KeyCode::Right => host.send(HostCmd::Key(0x27)),
                    KeyCode::Down => host.send(HostCmd::Key(0x28)),
                    KeyCode::Delete => host.send(HostCmd::Key(0x2E)),
                    KeyCode::Home => host.send(HostCmd::Key(0x24)),
                    KeyCode::End => host.send(HostCmd::Key(0x23)),
                    KeyCode::PageUp => host.send(HostCmd::Key(0x21)),
                    KeyCode::PageDown => host.send(HostCmd::Key(0x22)),
                    KeyCode::Char(c) if ctrl && c.is_ascii_alphabetic() => {
                        // control code via WM_CHAR (works for classic controls)
                        let code = (c.to_ascii_uppercase() as u8 & 0x1f) as char;
                        host.send(HostCmd::Char(code));
                    }
                    KeyCode::Char(c) => host.send(HostCmd::Char(c)),
                    _ => {}
                },
            }
            Action::None
        }

        fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
            if let Some(hit) = self.ui.route(&m) {
                return hit.map_or(Action::None, |b| self.run(b));
            }
            match &mut self.state {
                State::Wait { .. } => {}
                State::Pick { list, sel, off, .. } => match m.kind {
                    // the highlight follows the pointer
                    MouseEventKind::Moved => {
                        let content_y = area.y + 2;
                        if m.row >= content_y && m.row + 1 < area.y + area.height {
                            let idx = *off + (m.row - content_y) as usize;
                            if idx < list.len() {
                                *sel = idx;
                            }
                        }
                    }
                    MouseEventKind::ScrollUp => *sel = sel.saturating_sub(1),
                    MouseEventKind::ScrollDown => {
                        if *sel + 1 < list.len() {
                            *sel += 1;
                        }
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        let content_y = area.y + 2;
                        if m.row >= content_y {
                            let idx = *off + (m.row - content_y) as usize;
                            if m.row + 1 < area.y + area.height {
                                if let Some(e) = list.get(idx).cloned() {
                                    self.adopt(&e);
                                }
                            }
                        }
                    }
                    _ => {}
                },
                State::Host {
                    host, view, focus, zoom, ..
                } => match m.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        log_host(&format!(
                            "ui down cell=({},{}) view={:?}",
                            m.column,
                            m.row,
                            view.as_ref().map(|(d, c)| (d.x, d.y, d.width, d.height, *c))
                        ));
                        if let Some((x, y)) = view_px(&m, view, false) {
                            if *zoom > 1.01 {
                                *focus = (x as f64, y as f64);
                            }
                            host.send(HostCmd::Mouse { kind: MouseKind::Down, x, y });
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        if let Some((x, y)) = view_px(&m, view, true) {
                            host.send(HostCmd::Mouse { kind: MouseKind::Up, x, y });
                        }
                    }
                    MouseEventKind::Drag(MouseButton::Left) => {
                        if let Some((x, y)) = view_px(&m, view, true) {
                            host.send(HostCmd::Mouse { kind: MouseKind::Drag, x, y });
                        }
                    }
                    MouseEventKind::Moved => {
                        if let Some((x, y)) = view_px(&m, view, false) {
                            host.send(HostCmd::Mouse { kind: MouseKind::Move, x, y });
                        }
                    }
                    MouseEventKind::ScrollUp => {
                        if let Some((x, y)) = view_px(&m, view, true) {
                            host.send(HostCmd::Wheel { delta: 120, x, y });
                        }
                    }
                    MouseEventKind::ScrollDown => {
                        if let Some((x, y)) = view_px(&m, view, true) {
                            host.send(HostCmd::Wheel { delta: -120, x, y });
                        }
                    }
                    _ => {}
                },
            }
            Action::None
        }

        fn on_tick(&mut self) {
            self.t += 1;
            // launched program: adopt its first new window once it appears
            let mut found: Option<WinEntry> = None;
            let mut timed_out = false;
            if let State::Wait {
                exe_base,
                pid,
                since,
                last_poll,
                prev,
            } = &mut self.state
            {
                if last_poll.elapsed() >= Duration::from_millis(400) {
                    *last_poll = Instant::now();
                    let wins = list_windows();
                    found = wins.iter().find(|e| e.pid == *pid).cloned();
                    // some apps (Chromium/Electron single-instance) hand off
                    // to an existing process — accept a new window of the
                    // same program after a grace period
                    if found.is_none() && since.elapsed() >= Duration::from_secs(5) {
                        found = wins
                            .iter()
                            .find(|e| {
                                e.exe.eq_ignore_ascii_case(exe_base) && !prev.contains(&e.hwnd)
                            })
                            .cloned();
                    }
                    timed_out = found.is_none() && since.elapsed() >= Duration::from_secs(25);
                }
            }
            if let Some(e) = found {
                self.adopt(&e);
            } else if timed_out {
                self.release();
                self.note = "no window appeared — is the program running?".into();
            }
        }
    }
}

#[cfg(not(all(windows, feature = "apphost")))]
mod imp {
    //! Stub shown on non-Windows platforms / builds without `apphost`.
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;
    use ratatui::Frame;

    use crate::apps::{Action, AppWindow};

    pub struct AppHost;

    impl AppHost {
        pub fn with_cmd(_cmd: Option<String>) -> Self {
            AppHost
        }
    }

    impl AppWindow for AppHost {
        fn title(&self) -> String {
            "App Host".into()
        }

        fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
            f.render_widget(
                Paragraph::new(vec![
                    Line::default(),
                    Line::from(Span::styled(
                        "  ✗ hosting desktop apps is only available on Windows",
                        Style::default().fg(Color::Rgb(255, 110, 110)),
                    )),
                    Line::from(Span::styled(
                        "    (build with the `apphost` feature enabled)",
                        Style::default().fg(Color::Rgb(150, 155, 175)),
                    )),
                ]),
                area,
            );
        }

        fn on_key(&mut self, k: KeyEvent) -> Action {
            match k.code {
                KeyCode::Char('q') | KeyCode::Esc => Action::Close,
                _ => Action::None,
            }
        }
    }
}

pub use imp::AppHost;
