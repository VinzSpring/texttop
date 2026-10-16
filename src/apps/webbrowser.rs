//! Graphical web browser: a real Edge (WebView2) engine rendered into
//! terminal cells. JS, CSS, and media all work; mouse move/click/drag and
//! typing are forwarded into the page (native input on Windows, simulated
//! pointer events on Linux).

use std::path::PathBuf;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::scroll::ScrollState;
use super::ui::{Controls, Item, Look};
use super::webdata::{self, Bookmarks, History, Settings};
use super::{Action, AppWindow};

/// Controls in the status bar.
#[derive(Clone, Copy, PartialEq)]
enum Sb {
    TextMode,
    ZoomOut,
    ZoomIn,
    Go,
    CancelUrl,
}
use crate::engine::{js_escape, Cmd, CookieInfo, DataKind, Engine, MouseKind, Status};
use crate::gfx;

/// A clickable control in the toolbar row. Every browser action has one —
/// the key bindings are accelerators, not the way in.
#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Back,
    Forward,
    Reload,
    Bookmark,
    History,
    Bookmarks,
    Cookies,
    Privacy,
}

/// Navigation controls, pinned to the left of the URL.
const NAV_TOOLS: [(Tool, &str); 3] = [
    (Tool::Back, "◀"),
    (Tool::Forward, "▶"),
    (Tool::Reload, "⟳"),
];

/// Everything else, right-aligned. Ordered least- to most-droppable: a narrow
/// window keeps the ones nearest the URL and sheds the tail.
const MENU_TOOLS: [(Tool, &str); 4] = [
    (Tool::History, "History"),
    (Tool::Bookmarks, "Bookmarks"),
    (Tool::Cookies, "Cookies"),
    (Tool::Privacy, "Privacy"),
];

impl Tool {
    /// Shown in the status bar on hover, like a tooltip.
    fn hint(self) -> &'static str {
        match self {
            Tool::Back => "Back",
            Tool::Forward => "Forward",
            Tool::Reload => "Reload",
            Tool::Bookmark => "Bookmark this page",
            Tool::History => "History",
            Tool::Bookmarks => "Bookmarks",
            Tool::Cookies => "Cookies",
            Tool::Privacy => "Privacy & data",
        }
    }
}

/// Background for the history / bookmarks / cookies / privacy overlays.
const PANEL_BG: Color = Color::Rgb(16, 19, 32);

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// terminal cell ≈ 8×16 px; captures use that scale so aspect matches
const CELL_PX_W: i32 = 8;
const CELL_PX_H: i32 = 16;

const HOME_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>texttop start</title><style>
body{margin:0;font-family:'Segoe UI',sans-serif;background:linear-gradient(160deg,#0b0e2c,#42104e);color:#eee;display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:100vh}
h1{font-size:72px;margin:0 0 8px 0;background:linear-gradient(90deg,#00bfff,#c08cff);-webkit-background-clip:text;background-clip:text;color:transparent}
p{color:#aab;font-size:26px;margin:6px 0 26px}
a{display:inline-block;margin:10px;padding:20px 34px;border-radius:16px;background:#ffffff14;color:#8fd6ff;font-size:30px;text-decoration:none;border:1px solid #ffffff22}
a:hover{background:#ffffff2e}
</style></head><body><h1>texttop</h1><p>click a tile — or click the address bar and type a URL</p><div>
<a href="https://lite.duckduckgo.com/lite/">🔎 DuckDuckGo</a>
<a href="https://en.wikipedia.org">📚 Wikipedia</a>
<a href="https://news.ycombinator.com">🗞 Hacker News</a>
<a href="https://www.youtube.com">🎬 YouTube</a>
</div></body></html>"#;

/// Which overlay is open over the page, if any.
#[derive(Clone, Copy, PartialEq)]
enum Panel {
    None,
    History,
    Bookmarks,
    Cookies,
    Privacy,
}

impl Panel {
    fn title(self) -> &'static str {
        match self {
            Panel::None => "",
            Panel::History => "🕘 History",
            Panel::Bookmarks => "★ Bookmarks",
            Panel::Cookies => "🍪 Cookies",
            Panel::Privacy => "🛡 Privacy & data",
        }
    }

    /// Whether typing filters the list rather than reaching the page.
    fn filterable(self) -> bool {
        matches!(self, Panel::History | Panel::Bookmarks | Panel::Cookies)
    }
}

/// Rows of the privacy panel, in display order.
#[derive(Clone, Copy)]
enum PrivacyRow {
    Private,
    AcceptCookies,
    ClearOnExit,
    ClearCookies,
    ClearCache,
    ClearAll,
    ClearHistory,
}

const PRIVACY_ROWS: [PrivacyRow; 7] = [
    PrivacyRow::Private,
    PrivacyRow::AcceptCookies,
    PrivacyRow::ClearOnExit,
    PrivacyRow::ClearCookies,
    PrivacyRow::ClearCache,
    PrivacyRow::ClearAll,
    PrivacyRow::ClearHistory,
];

pub struct WebBrowser {
    engine: Engine,
    url_input: String,
    editing: bool,
    last_px: (i32, i32),
    last_url: String,
    zoom: f64,
    overlay: bool,
    t: u64,
    note: String,
    cache: gfx::RgbCache,
    history: History,
    bookmarks: Bookmarks,
    settings: Settings,
    panel: Panel,
    panel_bar: ScrollState,
    /// Selected row within the open panel.
    sel: usize,
    /// Live filter typed while a list panel is open.
    filter: String,
    /// Cookie inventory as of the last refresh; the engine reads it on demand.
    cookies: Vec<CookieInfo>,
    /// Last URL written to history, so a page is recorded once per navigation.
    recorded: String,
    /// Toolbar hit boxes from the last render: (x0, x1 exclusive, tool).
    tool_hits: Vec<(u16, u16, Tool)>,
    /// Status-bar controls (text mode, zoom, URL go / cancel).
    sb: Controls<Sb>,
    /// Tool under the pointer, so the status bar can explain it.
    hover: Option<Tool>,
}

impl WebBrowser {
    pub fn new(initial: Option<String>) -> Self {
        let init_cmd = match initial {
            Some(u) => Cmd::Navigate(normalize_url(&u)),
            None => Cmd::NavigateHtml(HOME_HTML.to_string()),
        };
        let settings = Settings::load();
        let engine = Engine::start(init_cmd, 1.0, 720, 480, settings.private);
        engine.send(Cmd::TextOverlay(true));
        engine.send(Cmd::SetCookiePolicy(settings.accept_cookies));
        WebBrowser {
            engine,
            url_input: String::new(),
            editing: false,
            last_px: (720, 480),
            last_url: String::new(),
            zoom: 1.0,
            overlay: true,
            t: 0,
            note: String::new(),
            cache: gfx::RgbCache::default(),
            history: History::load(),
            bookmarks: Bookmarks::load(),
            settings,
            panel: Panel::None,
            panel_bar: ScrollState::default(),
            sel: 0,
            filter: String::new(),
            cookies: Vec::new(),
            recorded: String::new(),
            tool_hits: Vec::new(),
            sb: Controls::new(),
            hover: None,
        }
    }

    // ── panels ──────────────────────────────────────────────────────────────

    fn open_panel(&mut self, p: Panel) {
        self.panel = if self.panel == p { Panel::None } else { p };
        self.sel = 0;
        self.filter.clear();
        if self.panel == Panel::Cookies {
            self.refresh_cookies();
        }
    }

    fn refresh_cookies(&mut self) {
        self.engine.send(Cmd::RefreshCookies);
        // The engine answers on its own thread; the list fills in on a later
        // tick. Keep whatever we already had until then.
        self.cookies = self.engine.cookies();
    }

    /// Indices into the backing list that survive the current filter.
    fn matches(&self) -> Vec<usize> {
        let q = self.filter.to_lowercase();
        let hit = |hay: &str| q.is_empty() || hay.to_lowercase().contains(&q);
        match self.panel {
            Panel::History => self
                .history
                .visits
                .iter()
                .enumerate()
                .filter(|(_, e)| hit(&e.title) || hit(&e.url))
                .map(|(i, _)| i)
                .collect(),
            Panel::Bookmarks => self
                .bookmarks
                .items
                .iter()
                .enumerate()
                .filter(|(_, e)| hit(&e.title) || hit(&e.url))
                .map(|(i, _)| i)
                .collect(),
            Panel::Cookies => self
                .cookies
                .iter()
                .enumerate()
                .filter(|(_, c)| hit(&c.domain) || hit(&c.name))
                .map(|(i, _)| i)
                .collect(),
            Panel::Privacy => (0..PRIVACY_ROWS.len()).collect(),
            Panel::None => Vec::new(),
        }
    }

    /// One display row: left-hand label and right-hand detail.
    fn row_text(&self, idx: usize) -> (String, String) {
        match self.panel {
            Panel::History | Panel::Bookmarks => {
                let list = if self.panel == Panel::History {
                    &self.history.visits
                } else {
                    &self.bookmarks.items
                };
                match list.get(idx) {
                    Some(e) => (
                        e.label().to_string(),
                        format!("{}  {}", webdata::host_of(&e.url), e.when()),
                    ),
                    None => (String::new(), String::new()),
                }
            }
            Panel::Cookies => match self.cookies.get(idx) {
                Some(c) => {
                    let mut flags = Vec::new();
                    if c.secure {
                        flags.push("secure");
                    }
                    if c.http_only {
                        flags.push("httpOnly");
                    }
                    if c.session_only {
                        flags.push("session");
                    }
                    (
                        format!("{}  ·  {}", c.domain, c.name),
                        flags.join(" · "),
                    )
                }
                None => (String::new(), String::new()),
            },
            Panel::Privacy => {
                let on = |b: bool| if b { "on" } else { "off" }.to_string();
                match PRIVACY_ROWS.get(idx) {
                    Some(PrivacyRow::Private) => (
                        "Private browsing".into(),
                        format!("{} · applies to new windows", on(self.settings.private)),
                    ),
                    Some(PrivacyRow::AcceptCookies) => {
                        ("Accept cookies".into(), on(self.settings.accept_cookies))
                    }
                    Some(PrivacyRow::ClearOnExit) => (
                        "Clear site data when this window closes".into(),
                        on(self.settings.clear_on_exit),
                    ),
                    Some(PrivacyRow::ClearCookies) => {
                        ("Clear cookies now".into(), format!("{} stored", self.cookies.len()))
                    }
                    Some(PrivacyRow::ClearCache) => ("Clear cache now".into(), String::new()),
                    Some(PrivacyRow::ClearAll) => (
                        "Clear all site data now".into(),
                        "cookies, caches, storage".into(),
                    ),
                    Some(PrivacyRow::ClearHistory) => (
                        "Clear history".into(),
                        format!("{} entries", self.history.visits.len()),
                    ),
                    None => (String::new(), String::new()),
                }
            }
            Panel::None => (String::new(), String::new()),
        }
    }

    /// Enter on the selected row.
    fn activate_row(&mut self) {
        let rows = self.matches();
        let Some(&idx) = rows.get(self.sel) else {
            return;
        };
        match self.panel {
            Panel::History | Panel::Bookmarks => {
                let url = if self.panel == Panel::History {
                    self.history.visits.get(idx).map(|e| e.url.clone())
                } else {
                    self.bookmarks.items.get(idx).map(|e| e.url.clone())
                };
                if let Some(url) = url {
                    self.engine.send(Cmd::Navigate(url));
                    self.panel = Panel::None;
                    self.filter.clear();
                }
            }
            // Selecting a cookie shows it; there is nothing to activate.
            Panel::Cookies | Panel::None => {}
            Panel::Privacy => self.run_privacy(idx),
        }
    }

    fn run_privacy(&mut self, idx: usize) {
        match PRIVACY_ROWS.get(idx) {
            Some(PrivacyRow::Private) => {
                self.settings.private = !self.settings.private;
                self.settings.save();
                self.note = if self.settings.private {
                    "private browsing on — open a new browser window to use it".into()
                } else {
                    "private browsing off — applies to new windows".into()
                };
            }
            Some(PrivacyRow::AcceptCookies) => {
                self.settings.accept_cookies = !self.settings.accept_cookies;
                self.settings.save();
                self.engine
                    .send(Cmd::SetCookiePolicy(self.settings.accept_cookies));
                self.note = format!(
                    "cookies {} — existing ones are untouched",
                    if self.settings.accept_cookies { "accepted" } else { "rejected" }
                );
            }
            Some(PrivacyRow::ClearOnExit) => {
                self.settings.clear_on_exit = !self.settings.clear_on_exit;
                self.settings.save();
            }
            Some(PrivacyRow::ClearCookies) => {
                self.engine.send(Cmd::ClearData(DataKind::Cookies));
                self.cookies.clear();
                self.note = "cookies cleared".into();
            }
            Some(PrivacyRow::ClearCache) => {
                self.engine.send(Cmd::ClearData(DataKind::Cache));
                self.note = "cache cleared".into();
            }
            Some(PrivacyRow::ClearAll) => {
                self.engine.send(Cmd::ClearData(DataKind::All));
                self.cookies.clear();
                self.note = "all site data cleared".into();
            }
            Some(PrivacyRow::ClearHistory) => {
                self.history.clear();
                self.note = "history cleared".into();
            }
            None => {}
        }
    }

    /// Delete on the selected row, where that means something.
    fn delete_row(&mut self) {
        let rows = self.matches();
        let Some(&idx) = rows.get(self.sel) else {
            return;
        };
        match self.panel {
            Panel::History => {
                self.history.remove(idx);
                self.note = "removed from history".into();
            }
            Panel::Bookmarks => {
                self.bookmarks.remove(idx);
                self.note = "bookmark removed".into();
            }
            _ => return,
        }
        self.sel = self.sel.min(self.matches().len().saturating_sub(1));
    }

    fn toggle_bookmark(&mut self) {
        let info = self.engine.info();
        if !webdata::is_recordable(&info.url) {
            self.note = "nothing to bookmark on this page".into();
            return;
        }
        self.note = if self.bookmarks.toggle(&info.url, &info.title) {
            format!("★ bookmarked {}", webdata::host_of(&info.url))
        } else {
            "bookmark removed".into()
        };
    }

    /// Keys while a panel is open. Returns false when the panel is closed and
    /// the key should fall through to the page.
    fn panel_key(&mut self, k: KeyEvent) -> bool {
        if self.panel == Panel::None {
            return false;
        }
        let rows = self.matches();
        let n = rows.len();
        match k.code {
            KeyCode::Esc => {
                // Back out of the filter first, then out of the panel.
                if self.filter.is_empty() {
                    self.panel = Panel::None;
                } else {
                    self.filter.clear();
                    self.sel = 0;
                }
            }
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => {
                if n > 0 {
                    self.sel = (self.sel + 1).min(n - 1);
                }
            }
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(n.saturating_sub(1)),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = n.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char(' ') if self.panel == Panel::Privacy => {
                self.activate_row()
            }
            KeyCode::Enter => self.activate_row(),
            KeyCode::Delete => self.delete_row(),
            _ if self.panel.filterable() => {
                if super::text_edit(&mut self.filter, k) {
                    self.sel = 0;
                }
            }
            _ => {}
        }
        true
    }

    /// Draw the toolbar row: navigation, the URL, then the panel buttons.
    /// Records a hit box per drawn control so `on_mouse` can match them —
    /// only controls that actually fit get one.
    fn render_toolbar(&mut self, f: &mut Frame, bar: Rect, info: &crate::engine::PageInfo) {
        let bg = if self.editing {
            Color::Rgb(0, 70, 110)
        } else {
            Color::Rgb(26, 32, 56)
        };
        gfx::fill(f.buffer_mut(), bar, bg);
        self.tool_hits.clear();

        let bookmarked = self.bookmarks.contains(&info.url);
        let star = if bookmarked { "★" } else { "☆" };
        // Right-aligned group, built first so the URL can take what is left.
        let mut right: Vec<(Tool, String)> = vec![(Tool::Bookmark, format!(" {star} "))];
        for (tool, label) in MENU_TOOLS {
            right.push((tool, format!(" {label} ")));
        }

        let buf = f.buffer_mut();
        let mut x = bar.x;
        // ── navigation ──
        for (tool, glyph) in NAV_TOOLS {
            let label = format!(" {glyph} ");
            let w = label.chars().count() as u16;
            if x + w > bar.x + bar.width {
                break;
            }
            let hot = self.hover == Some(tool);
            buf.set_string(
                x,
                bar.y,
                &label,
                Style::default()
                    .fg(if hot { Color::White } else { Color::Rgb(190, 200, 225) })
                    .bg(if hot { Color::Rgb(0, 90, 140) } else { bg })
                    .add_modifier(Modifier::BOLD),
            );
            self.tool_hits.push((x, x + w, tool));
            x += w;
        }

        // ── right-hand buttons, dropping the tail when space runs short ──
        let space = (bar.x + bar.width).saturating_sub(x);
        let mut shown: Vec<(Tool, String)> = Vec::new();
        let mut used = 0u16;
        // Always keep the URL at least this legible before adding buttons.
        const MIN_URL: u16 = 12;
        for item in right {
            let w = item.1.chars().count() as u16;
            if used + w + MIN_URL > space {
                break;
            }
            used += w;
            shown.push(item);
        }
        let mut bx = bar.x + bar.width - used;
        for (tool, label) in &shown {
            let w = label.chars().count() as u16;
            let hot = self.hover == Some(*tool);
            let on = *tool == Tool::Bookmark && bookmarked
                || matches!(
                    (tool, self.panel),
                    (Tool::History, Panel::History)
                        | (Tool::Bookmarks, Panel::Bookmarks)
                        | (Tool::Cookies, Panel::Cookies)
                        | (Tool::Privacy, Panel::Privacy)
                );
            let fg = if on {
                Color::Rgb(255, 215, 120)
            } else if hot {
                Color::White
            } else {
                Color::Rgb(180, 190, 215)
            };
            buf.set_string(
                bx,
                bar.y,
                label,
                Style::default()
                    .fg(fg)
                    .bg(if hot { Color::Rgb(0, 90, 140) } else { bg }),
            );
            self.tool_hits.push((bx, bx + w, *tool));
            bx += w;
        }

        // ── the URL itself, in whatever room is left ──
        let url_w = (bar.x + bar.width).saturating_sub(x + used).saturating_sub(1) as usize;
        let text = if self.editing {
            format!("🌐 {}▏", self.url_input)
        } else {
            let url = if info.url.is_empty() || info.url.starts_with("about:") {
                "click to type a URL".to_string()
            } else {
                info.url.clone()
            };
            let lock = if self.settings.private { "🕶 " } else { "" };
            format!("🌐 {lock}{url}")
        };
        buf.set_string(
            x + 1,
            bar.y,
            text.chars().take(url_w).collect::<String>(),
            Style::default().fg(Color::White).bg(bg),
        );
    }

    fn run_sb(&mut self, c: Sb) {
        match c {
            Sb::TextMode => {
                self.overlay = !self.overlay;
                self.engine.send(Cmd::TextOverlay(self.overlay));
                self.note = if self.overlay {
                    "crisp text overlay ON".into()
                } else {
                    "crisp text overlay OFF (pure pixels)".into()
                };
            }
            Sb::ZoomOut | Sb::ZoomIn => {
                let d = if c == Sb::ZoomIn { 0.25 } else { -0.25 };
                self.zoom = (self.zoom + d).clamp(0.5, 5.0);
                self.cache.clear();
                self.engine.send(Cmd::Zoom(self.zoom));
            }
            Sb::Go => {
                self.on_key(KeyEvent::from(KeyCode::Enter));
            }
            Sb::CancelUrl => {
                self.on_key(KeyEvent::from(KeyCode::Esc));
            }
        }
    }


    /// Run a toolbar control. Same entry point for a click and for the
    /// matching accelerator, so the two can never diverge.
    fn run_tool(&mut self, tool: Tool) {
        match tool {
            Tool::Back => self.engine.send(Cmd::Back),
            Tool::Forward => self.engine.send(Cmd::Forward),
            Tool::Reload => self.engine.send(Cmd::Reload),
            Tool::Bookmark => self.toggle_bookmark(),
            Tool::History => self.open_panel(Panel::History),
            Tool::Bookmarks => self.open_panel(Panel::Bookmarks),
            Tool::Cookies => self.open_panel(Panel::Cookies),
            Tool::Privacy => self.open_panel(Panel::Privacy),
        }
    }

    fn tool_at(&self, col: u16) -> Option<Tool> {
        self.tool_hits
            .iter()
            .find(|(x0, x1, _)| col >= *x0 && col < *x1)
            .map(|(_, _, t)| *t)
    }

    /// Index into the filtered rows under the pointer, if it is over a row.
    fn panel_row_at(&self, m: MouseEvent, area: Rect) -> Option<usize> {
        let n = self.matches().len();
        row_at(panel_rect(area, n), self.sel, n, m.column, m.row)
    }

    fn render_panel(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.matches();
        let rect = panel_rect(area, rows.len());
        let w = rect.width;
        gfx::fill(f.buffer_mut(), rect, PANEL_BG);

        let head = if self.filter.is_empty() {
            format!(" {} ", self.panel.title())
        } else {
            format!(" {} · /{}▏", self.panel.title(), self.filter)
        };
        let hint = match self.panel {
            Panel::Privacy => " click a row to toggle it ",
            Panel::Cookies => " type to filter · ✕ closes ",
            _ => " click to open · ✕ on a row removes it · type to filter ",
        };
        let deletable = matches!(self.panel, Panel::History | Panel::Bookmarks);
        let buf = f.buffer_mut();
        // Close control, so the panel can be dismissed without knowing Esc.
        if rect.width > DEL_W {
            buf.set_string(
                rect.x + rect.width - DEL_W,
                rect.y,
                " ✕ ",
                Style::default().fg(Color::Rgb(200, 130, 130)).bg(PANEL_BG),
            );
        }
        buf.set_string(
            rect.x + 1,
            rect.y,
            head.chars().take(w.saturating_sub(DEL_W + 2) as usize).collect::<String>(),
            Style::default()
                .fg(Color::Rgb(150, 200, 255))
                .bg(PANEL_BG)
                .add_modifier(Modifier::BOLD),
        );

        let list_h = list_height(rect);
        // Keep the cursor in view without a separate scroll offset: the window
        // is derived from the selection each frame.
        let top = scroll_top(self.sel, list_h);
        if rows.is_empty() {
            buf.set_string(
                rect.x + 2,
                rect.y + 2,
                empty_note(self.panel, !self.filter.is_empty()),
                Style::default().fg(Color::Rgb(120, 130, 160)).bg(PANEL_BG),
            );
        }
        for (line, &idx) in rows.iter().enumerate().skip(top).take(list_h) {
            let y = rect.y + 2 + (line - top) as u16;
            let picked = line == self.sel;
            let (bg, fg) = if picked {
                (Color::Rgb(0, 80, 130), Color::White)
            } else {
                (PANEL_BG, Color::Rgb(210, 215, 235))
            };
            for x in rect.x..rect.x + rect.width {
                if let Some(c) = buf.cell_mut((x, y)) {
                    c.set_symbol(" ");
                    c.set_bg(bg);
                }
            }
            let (left, right) = self.row_text(idx);
            // Reserve the control column first, then split what is left: the
            // detail takes what the label does not need, so a long title never
            // pushes the timestamp off the edge.
            let reserved = if deletable { DEL_W } else { 0 };
            let usable = rect.width.saturating_sub(reserved) as usize;
            let right_w = right.chars().count().min(usable / 2);
            let left_w = usable.saturating_sub(right_w + 5);
            buf.set_string(
                rect.x + 2,
                y,
                left.chars().take(left_w).collect::<String>(),
                Style::default().fg(fg).bg(bg),
            );
            if right_w > 0 {
                let rx = rect.x + rect.width - reserved - 2 - right_w as u16;
                buf.set_string(
                    rx,
                    y,
                    right.chars().take(right_w).collect::<String>(),
                    Style::default().fg(Color::Rgb(130, 145, 180)).bg(bg),
                );
            }
            if deletable && rect.width > DEL_W {
                buf.set_string(
                    rect.x + rect.width - DEL_W,
                    y,
                    " ✕ ",
                    Style::default()
                        .fg(if picked { Color::Rgb(255, 170, 170) } else { Color::Rgb(120, 100, 110) })
                        .bg(bg),
                );
            }
        }
        self.panel_bar.offset = top as u16;
        let bar = (rows.len() > list_h)
            .then(|| Rect::new(rect.x + rect.width - 1, rect.y + 2, 1, list_h as u16));
        self.panel_bar.show_bar(buf, bar, rows.len(), list_h as u16);
        buf.set_string(
            rect.x + 1,
            rect.y + rect.height - 1,
            hint.chars().take(w as usize - 2).collect::<String>(),
            Style::default().fg(Color::Rgb(120, 130, 160)).bg(PANEL_BG),
        );
    }

    fn script(&self, s: String) {
        self.engine.send(Cmd::Script(s));
    }

    fn scroll_by(&self, dy: i32) {
        self.script(format!("window.scrollBy(0,{dy});"));
    }

    /// Draw extracted page text as real terminal characters on top of the
    /// pixel frame — crisp at any zoom. Backgrounds are sampled from the
    /// frame so the text sits on the page's true colors. Words are laid
    /// out per row in reading order with a minimum one-cell gap, since
    /// proportional fonts pack tighter than terminal cells.
    fn draw_text_overlay(&self, f: &mut Frame, content: Rect, frame: Option<&crate::engine::Frame>) {
        use std::collections::BTreeMap;
        use unicode_width::UnicodeWidthChar;
        let zoom = self.zoom;

        // Map CSS pixels → cells via the captured frame size so overlay stays
        // aligned when the engine viewport differs from the TUI cell grid
        // (notably macOS's fixed 800×600 WKWebView).
        let (fw, fh) = match frame {
            Some(fr) if fr.w > 0 && fr.h > 0 => (fr.w as f64, fr.h as f64),
            _ => (
                content.width as f64 * CELL_PX_W as f64,
                content.height as f64 * CELL_PX_H as f64,
            ),
        };
        let cw = content.width.max(1) as f64;
        let ch = content.height.max(1) as f64;

        let mut rows: BTreeMap<i32, Vec<(i32, crate::engine::TextRun)>> = BTreeMap::new();
        for run in self.engine.texts() {
            let col0 = (run.x as f64 * zoom / fw * cw).round() as i32;
            let row = ((run.y + run.h / 2.0) as f64 * zoom / fh * ch) as i32;
            if row < 0 || row >= content.height as i32 {
                continue;
            }
            rows.entry(row).or_default().push((col0, run));
        }

        let buf = f.buffer_mut();
        for (row, mut words) in rows {
            words.sort_by_key(|(c, _)| *c);
            let y = content.y + row as u16;
            let mut last_end = i32::MIN;
            for (col0, run) in words {
                // keep at least one blank cell between consecutive words
                let mut col = if col0 <= last_end + 1 {
                    last_end + 2
                } else {
                    col0
                };
                for ch in run.text.chars() {
                let w = ch.width().unwrap_or(1).max(1) as i32;
                if col >= content.width as i32 {
                    break;
                }
                if col >= 0 {
                    let x = content.x + col as u16;
                    let (br, bg_, bb) = match frame {
                        Some(fr) => gfx::sample(
                            &fr.rgb,
                            fr.w,
                            fr.h,
                            content.width as u32,
                            content.height as u32,
                            col as u32,
                            row as u32,
                        ),
                        None => (250, 250, 250),
                    };
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        let mut s = String::new();
                        s.push(ch);
                        cell.set_symbol(&s);
                        cell.set_fg(ratatui::style::Color::Rgb(
                            run.color.0,
                            run.color.1,
                            run.color.2,
                        ));
                        cell.set_bg(ratatui::style::Color::Rgb(br as u8, bg_ as u8, bb as u8));
                        cell.modifier = if run.bold {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        };
                    }
                    // clear the continuation cell of wide characters
                    if w == 2 && col + 1 < content.width as i32 {
                        if let Some(cell) = buf.cell_mut((x + 1, y)) {
                            cell.set_symbol(" ");
                        }
                    }
                }
                col += w;
                }
                last_end = col - 1;
            }
        }
    }
}

/// Map a terminal mouse event onto the engine's physical viewport pixels.
/// When `clamp` is set, coordinates outside the content rect are snapped to
/// the edge (needed so mouse-up still completes a click).
fn content_px(m: &MouseEvent, content: Rect, clamp: bool) -> Option<(i32, i32)> {
    if content.width == 0 || content.height == 0 {
        return None;
    }
    let inside = m.column >= content.x
        && m.column < content.x + content.width
        && m.row >= content.y
        && m.row < content.y + content.height;
    if !inside && !clamp {
        return None;
    }
    let col = if clamp {
        m.column
            .max(content.x)
            .min(content.x + content.width.saturating_sub(1))
    } else {
        m.column
    };
    let row = if clamp {
        m.row
            .max(content.y)
            .min(content.y + content.height.saturating_sub(1))
    } else {
        m.row
    };
    let px = (col.saturating_sub(content.x)) as i32 * CELL_PX_W + CELL_PX_W / 2;
    let py = (row.saturating_sub(content.y)) as i32 * CELL_PX_H + CELL_PX_H / 2;
    Some((px, py))
}

fn normalize_url(u: &str) -> String {
    let u = u.trim();
    if std::path::Path::new(u).exists() {
        if let Ok(url) = url::Url::from_file_path(std::path::absolute(u).unwrap_or_else(|_| u.into()))
        {
            return url.to_string();
        }
    }
    if u.contains("://") || u.starts_with("about:") || u.starts_with("data:") {
        u.to_string()
    } else if u.contains('.') && !u.contains(' ') {
        format!("https://{u}")
    } else {
        // treat as search query
        format!(
            "https://lite.duckduckgo.com/lite/?q={}",
            u.replace(' ', "+")
        )
    }
}

impl AppWindow for WebBrowser {
    fn title(&self) -> String {
        let info = self.engine.info();
        if !info.title.is_empty() {
            info.title.chars().take(36).collect()
        } else if !info.url.is_empty() {
            info.url.chars().take(36).collect()
        } else {
            "Web Browser".into()
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        if area.height < 3 {
            return;
        }
        let url_bar = Rect::new(area.x, area.y, area.width, 1);
        let content = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(2));
        let status_bar = Rect::new(area.x, area.y + area.height - 1, area.width, 1);

        // keep engine viewport matched to the window
        let want = (
            content.width as i32 * CELL_PX_W,
            content.height as i32 * CELL_PX_H,
        );
        if want != self.last_px && want.0 > 0 && want.1 > 0 {
            self.last_px = want;
            self.engine.send(Cmd::ResizePx(want.0, want.1));
        }

        // url bar
        let info = self.engine.info();
        if info.url != self.last_url {
            self.last_url = info.url.clone();
            self.cache.clear();
        }
        self.render_toolbar(f, url_bar, &info);

        // page frame — always wipe the content area first so no prior
        // page glyphs / overlay words can linger between navigations.
        gfx::fill(f.buffer_mut(), content, Color::Rgb(250, 250, 250));
        match self.engine.status() {
            Status::Error(e) => {
                let mut lines = vec![Line::default()];
                for chunk in e.split(". ") {
                    lines.push(Line::from(Span::styled(
                        format!("  ✗ {chunk}"),
                        Style::default().fg(Color::Rgb(255, 110, 110)),
                    )));
                }
                f.render_widget(Paragraph::new(lines), content);
            }
            _ => {
                let frame = self.engine.frame();
                let texts = self.engine.texts();
                if let Some(fr) = &frame {
                    gfx::draw_rgb_cached(
                        f.buffer_mut(),
                        content,
                        &fr.rgb,
                        fr.w,
                        fr.h,
                        fr.seq as usize,
                        &mut self.cache,
                    );
                    // Only composite crisp text when we actually have runs —
                    // otherwise keep the pixel frame (avoids blank pages).
                    if self.overlay && !texts.is_empty() {
                        self.draw_text_overlay(f, content, Some(fr));
                    }
                } else if self.overlay && !texts.is_empty() {
                    self.draw_text_overlay(f, content, None);
                } else {
                    let spin = SPINNER[(self.t / 2) as usize % SPINNER.len()];
                    let msg = format!("{spin}  starting browser engine …");
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
                }
            }
        }

        if self.panel != Panel::None {
            self.render_panel(f, content);
        }

        // status bar: page title or tooltip, then view controls
        let left = if let Some(tool) = self.hover {
            tool.hint().to_string()
        } else if !self.note.is_empty() {
            self.note.clone()
        } else {
            info.title
        };
        let dim = if focused {
            Color::Rgb(140, 150, 175)
        } else {
            Color::Rgb(100, 105, 125)
        };
        self.sb.begin();
        let label = Item::Label(format!(" {}", left.chars().take(60).collect::<String>()), dim);
        let right = if self.editing {
            vec![
                Item::with("Go", Sb::Go, Look::Primary),
                Item::with("Cancel", Sb::CancelUrl, Look::Raised),
            ]
        } else {
            vec![
                Item::with("Aa Crisp text", Sb::TextMode, Look::on_if(self.overlay)),
                Item::Sep,
                Item::with("−", Sb::ZoomOut, Look::enabled_if(self.zoom > 0.5)),
                Item::Label(format!("{:.0}%", self.zoom * 100.0), dim),
                Item::with("+", Sb::ZoomIn, Look::enabled_if(self.zoom < 5.0)),
            ]
        };
        self.sb.toolbar(f.buffer_mut(), status_bar, &[label], &right);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        self.note.clear();

        if self.editing {
            match k.code {
                KeyCode::Esc => self.editing = false,
                KeyCode::Enter => {
                    let url = normalize_url(&self.url_input);
                    self.engine.send(Cmd::Navigate(url));
                    self.editing = false;
                }
                _ => {
                    super::text_edit(&mut self.url_input, k);
                }
            }
            return Action::None;
        }

        if self.panel_key(k) {
            return Action::None;
        }

        match k.code {
            KeyCode::Char('h') if ctrl => self.run_tool(Tool::History),
            KeyCode::Char('k') if ctrl => self.run_tool(Tool::Bookmarks),
            KeyCode::F(2) => self.run_tool(Tool::Bookmark),
            KeyCode::F(7) => self.run_tool(Tool::Privacy),
            KeyCode::F(8) => self.run_tool(Tool::Cookies),
            // The desktop claims F5 for itself before any window sees it, so
            // reload needs a chord of its own.
            KeyCode::Char('r') if ctrl => self.run_tool(Tool::Reload),
            KeyCode::Char('l') if ctrl => {
                self.editing = true;
                self.url_input = self.engine.info().url;
                if self.url_input.starts_with("about:") || self.url_input.starts_with("data:") {
                    self.url_input.clear();
                }
            }
            KeyCode::F(4) => {
                self.editing = true;
                self.url_input.clear();
            }
            KeyCode::Char('b') if ctrl => self.run_tool(Tool::Back),
            KeyCode::Char('f') if ctrl => self.run_tool(Tool::Forward),
            KeyCode::F(3) => self.run_sb(Sb::TextMode),
            KeyCode::F(9) => self.run_sb(Sb::ZoomOut),
            KeyCode::F(10) => self.run_sb(Sb::ZoomIn),
            KeyCode::Up => self.scroll_by(-100),
            KeyCode::Down => self.scroll_by(100),
            KeyCode::PageUp => self.scroll_by(-600),
            KeyCode::PageDown => self.scroll_by(600),
            KeyCode::Home => self.script("window.scrollTo(0,0);".into()),
            KeyCode::End => {
                self.script("window.scrollTo(0,document.body.scrollHeight);".into())
            }
            KeyCode::Enter => {
                self.script(
                    "(function(){var el=document.activeElement;if(!el)return;\
                     var o={bubbles:true,cancelable:true,key:'Enter',code:'Enter',keyCode:13,which:13};\
                     el.dispatchEvent(new KeyboardEvent('keydown',o));\
                     el.dispatchEvent(new KeyboardEvent('keypress',o));\
                     el.dispatchEvent(new KeyboardEvent('keyup',o));\
                     if(el.form){if(el.form.requestSubmit)el.form.requestSubmit();else el.form.submit();}})();"
                        .into(),
                );
            }
            KeyCode::Backspace => {
                self.script(
                    "(function(){var el=document.activeElement;if(!el)return;\
                     if(el.tagName=='INPUT'||el.tagName=='TEXTAREA'){\
                       var s=el.selectionStart==null?el.value.length:el.selectionStart;\
                       if(s>0){el.value=el.value.slice(0,s-1)+el.value.slice(s);\
                         try{el.selectionStart=el.selectionEnd=s-1}catch(e){}\
                         el.dispatchEvent(new Event('input',{bubbles:true}));}}\
                     else if(el.isContentEditable){document.execCommand('delete');}})();"
                        .into(),
                );
            }
            KeyCode::Char(c) if !ctrl => {
                let esc = js_escape(&c.to_string());
                self.script(format!(
                    "(function(t){{var el=document.activeElement;if(!el)return;\
                     if(el.tagName=='INPUT'||el.tagName=='TEXTAREA'){{\
                       var s=el.selectionStart==null?el.value.length:el.selectionStart;\
                       var e=el.selectionEnd==null?s:el.selectionEnd;\
                       el.value=el.value.slice(0,s)+t+el.value.slice(e);\
                       try{{el.selectionStart=el.selectionEnd=s+t.length}}catch(x){{}}\
                       el.dispatchEvent(new Event('input',{{bubbles:true}}));}}\
                     else if(el.isContentEditable){{document.execCommand('insertText',false,t);}}}})(\"{esc}\");"
                ));
            }
            _ => {}
        }
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        // Only meaningful while the URL bar has focus; Ctrl+L opens it.
        if self.editing {
            super::paste_line(&mut self.url_input, text);
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        let content = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(2));
        if let Some(hit) = self.sb.route(&m) {
            if let Some(c) = hit {
                self.run_sb(c);
            }
            return Action::None;
        }

        // A panel covers the page, so it takes the mouse while it is open —
        // otherwise clicks would land on a page the user cannot see.
        if self.panel != Panel::None {
            self.hover = if m.row == area.y {
                self.tool_at(m.column)
            } else {
                None
            };
            // The toolbar stays live so its buttons can close the panel again.
            if m.row == area.y {
                if let (MouseEventKind::Down(MouseButton::Left), Some(tool)) =
                    (m.kind, self.tool_at(m.column))
                {
                    self.run_tool(tool);
                }
                return Action::None;
            }
            // Scrollbar: the window follows the selection, so dragging moves
            // the selection to the bottom of the view it asks for.
            if !matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
                let n = self.matches().len();
                let list_h = list_height(panel_rect(content, n));
                self.panel_bar.offset = scroll_top(self.sel, list_h) as u16;
                if self.panel_bar.on_mouse(m, n, list_h as u16) {
                    let top = self.panel_bar.offset as usize;
                    self.sel = if top == 0 {
                        self.sel.min(list_h.saturating_sub(1))
                    } else {
                        top + list_h - 1
                    }
                    .min(n.saturating_sub(1));
                    return Action::None;
                }
            }
            match m.kind {
                MouseEventKind::ScrollUp => self.sel = self.sel.saturating_sub(3),
                MouseEventKind::ScrollDown => {
                    let n = self.matches().len();
                    self.sel = (self.sel + 3).min(n.saturating_sub(1));
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let rect = panel_rect(content, self.matches().len());
                    if m.row == rect.y && in_control_col(rect, m.column) {
                        self.panel = Panel::None;
                        return Action::None;
                    }
                    match self.panel_row_at(m, content) {
                        Some(line) => {
                            self.sel = line;
                            let deletable =
                                matches!(self.panel, Panel::History | Panel::Bookmarks);
                            if deletable && in_control_col(rect, m.column) {
                                self.delete_row();
                            } else {
                                self.activate_row();
                            }
                        }
                        // A click outside the panel dismisses it, the way every
                        // menu in this desktop behaves.
                        None => self.panel = Panel::None,
                    }
                }
                _ => {}
            }
            return Action::None;
        }

        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if m.row == area.y {
                    // Toolbar: a control if one is under the pointer, the URL
                    // field otherwise.
                    match self.tool_at(m.column) {
                        Some(tool) => self.run_tool(tool),
                        None => {
                            self.editing = true;
                            self.url_input.clear();
                        }
                    }
                    return Action::None;
                }
                if let Some((x, y)) = content_px(&m, content, false) {
                    self.engine
                        .send(Cmd::Mouse { kind: MouseKind::Down, x, y });
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some((x, y)) = content_px(&m, content, true) {
                    self.engine
                        .send(Cmd::Mouse { kind: MouseKind::Up, x, y });
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((x, y)) = content_px(&m, content, true) {
                    self.engine
                        .send(Cmd::Mouse { kind: MouseKind::Drag, x, y });
                }
            }
            MouseEventKind::Moved => {
                // Hovering a control lights it up and explains it in the
                // status bar; only pass the move to the page otherwise.
                self.hover = if m.row == area.y {
                    self.tool_at(m.column)
                } else {
                    None
                };
                if self.hover.is_none() {
                    if let Some((x, y)) = content_px(&m, content, false) {
                        self.engine
                            .send(Cmd::Mouse { kind: MouseKind::Move, x, y });
                    }
                }
            }
            MouseEventKind::ScrollUp => self.scroll_by(-120),
            MouseEventKind::ScrollDown => self.scroll_by(120),
            _ => {}
        }
        Action::None
    }

    fn on_tick(&mut self) {
        self.t += 1;

        // The engine answers RefreshCookies on its own thread, so the list
        // arrives a tick or two after the panel opens.
        if self.panel == Panel::Cookies && self.t % 8 == 0 {
            self.cookies = self.engine.cookies();
        }

        // Private browsing leaves no trace on disk.
        if self.settings.private {
            return;
        }
        let info = self.engine.info();
        if info.url != self.recorded {
            self.recorded = info.url.clone();
            self.history.record(&info.url, &info.title);
        } else {
            // Titles land after the URL does, so fill it in once it shows up.
            self.history.retitle(&info.url, &info.title);
        }
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    fn on_drop(&mut self, paths: &[PathBuf], _cut: bool) -> Action {
        let Some(p) = paths.first() else {
            return Action::None;
        };
        if p.is_dir() {
            return Action::Launch(super::Launch::Terminal {
                program: None,
                args: Vec::new(),
                cwd: Some(p.clone()),
            });
        }
        // navigate to local file / html
        let url = p.to_string_lossy().to_string();
        self.engine.send(Cmd::Navigate(normalize_url(&url)));
        Action::None
    }
}

impl Drop for WebBrowser {
    fn drop(&mut self) {
        // A private window has nothing on disk to wipe — its store was
        // in-memory to begin with.
        if !self.settings.clear_on_exit || self.settings.private {
            return;
        }
        self.engine.send(Cmd::ClearData(DataKind::All));
        // The engine drains queued commands before it notices the channel is
        // closed, but it runs on its own thread: give it long enough to do the
        // wipe, since after this the process may be on its way out.
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

/// Geometry of the panel overlay inside `area`, sized to `rows` entries.
/// Shared by the renderer and the hit test so the two cannot drift apart.
fn panel_rect(area: Rect, rows: usize) -> Rect {
    // Never larger than the window: a panel that spilled past its edges would
    // paint over neighbouring windows' chrome.
    let w = area.width.saturating_sub(4).min(96).max(1);
    let h = (rows as u16 + 5)
        .max(6)
        .min(area.height.saturating_sub(2).max(1));
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

/// Width of the per-row delete control at the right edge of a list panel.
const DEL_W: u16 = 3;

/// Whether `col` falls in a panel's right-edge control column — the ✕ that
/// closes the panel on the title row, or removes an entry on a list row.
fn in_control_col(rect: Rect, col: u16) -> bool {
    let end = rect.x + rect.width;
    col < end && col + DEL_W >= end
}

/// Rows of list the panel can show: its height less the title and the hint.
fn list_height(rect: Rect) -> usize {
    rect.height.saturating_sub(3) as usize
}

/// First visible row. There is no stored scroll offset — the window follows
/// the selection, which is enough for a list that only moves one row at a time.
fn scroll_top(sel: usize, list_h: usize) -> usize {
    sel.saturating_sub(list_h.saturating_sub(1))
}

/// Which filtered row sits under (`col`, `row`), or None when the pointer is
/// outside the list.
fn row_at(rect: Rect, sel: usize, rows: usize, col: u16, row: u16) -> Option<usize> {
    if col < rect.x || col >= rect.x + rect.width {
        return None;
    }
    let list_h = list_height(rect);
    let first = rect.y + 2;
    if row < first || row >= first + list_h as u16 {
        return None;
    }
    let line = scroll_top(sel, list_h) + (row - first) as usize;
    (line < rows).then_some(line)
}

/// What to say when a panel has nothing to show.
fn empty_note(panel: Panel, filtered: bool) -> &'static str {
    if filtered {
        return "nothing matches that filter";
    }
    match panel {
        Panel::History => "no pages visited yet",
        Panel::Bookmarks => "no bookmarks yet — click ☆ in the toolbar to add the current page",
        Panel::Cookies => "no cookies stored",
        _ => "nothing here",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 2,
        y: 3,
        width: 80,
        height: 30,
    };

    #[test]
    fn the_panel_fits_inside_its_area() {
        for rows in [0usize, 1, 5, 200] {
            let r = panel_rect(AREA, rows);
            assert!(r.x >= AREA.x, "rows {rows}");
            assert!(r.y >= AREA.y, "rows {rows}");
            assert!(r.x + r.width <= AREA.x + AREA.width, "rows {rows}");
            assert!(r.y + r.height <= AREA.y + AREA.height, "rows {rows}");
        }
    }

    /// A window far too small for the panel must still contain it, rather
    /// than painting over whatever is beside it.
    #[test]
    fn a_tiny_window_still_contains_the_panel() {
        for (w, h) in [(10u16, 4u16), (6, 3), (1, 1), (200, 200)] {
            let tiny = Rect { x: 4, y: 4, width: w, height: h };
            let r = panel_rect(tiny, 50);
            assert!(r.x >= tiny.x && r.y >= tiny.y, "{w}x{h}: {r:?}");
            assert!(r.x + r.width <= tiny.x + tiny.width, "{w}x{h}: {r:?}");
            assert!(r.y + r.height <= tiny.y + tiny.height, "{w}x{h}: {r:?}");
        }
    }

    /// Clicking row N must select row N — the renderer and the hit test derive
    /// their positions from the same helpers, and this pins that down.
    #[test]
    fn clicking_a_row_selects_that_row() {
        let rows = 8;
        let rect = panel_rect(AREA, rows);
        let list_h = list_height(rect);
        for line in 0..rows.min(list_h) {
            let y = rect.y + 2 + line as u16;
            assert_eq!(row_at(rect, 0, rows, rect.x + 3, y), Some(line), "line {line}");
        }
    }

    #[test]
    fn clicks_off_the_list_hit_nothing() {
        let rect = panel_rect(AREA, 8);
        // title row, the hint row, and either side of the panel
        assert_eq!(row_at(rect, 0, 8, rect.x + 3, rect.y), None);
        assert_eq!(row_at(rect, 0, 8, rect.x + 3, rect.y + rect.height - 1), None);
        assert_eq!(row_at(rect, 0, 8, rect.x - 1, rect.y + 2), None);
        assert_eq!(row_at(rect, 0, 8, rect.x + rect.width, rect.y + 2), None);
    }

    #[test]
    fn empty_space_below_a_short_list_hits_nothing() {
        let rect = panel_rect(AREA, 3);
        assert_eq!(row_at(rect, 0, 3, rect.x + 3, rect.y + 2 + 3), None);
    }

    #[test]
    fn a_selection_past_the_fold_scrolls_the_window() {
        let rect = panel_rect(AREA, 500);
        let list_h = list_height(rect);
        // Selection at the bottom of a long list puts it on the last line.
        let sel = 400;
        assert_eq!(scroll_top(sel, list_h), sel + 1 - list_h);
        let last_y = rect.y + 2 + list_h as u16 - 1;
        assert_eq!(row_at(rect, sel, 500, rect.x + 3, last_y), Some(sel));
    }

    #[test]
    fn the_control_column_is_the_right_edge_of_the_panel() {
        let rect = panel_rect(AREA, 8);
        let end = rect.x + rect.width;
        for col in end - DEL_W..end {
            assert!(in_control_col(rect, col), "col {col} should be a control");
        }
        for col in [rect.x, rect.x + 2, end - DEL_W - 1, end, end + 5] {
            assert!(!in_control_col(rect, col), "col {col} should not be");
        }
    }

    /// The ✕ sits inside the row, so a click on it must resolve to that row —
    /// the caller then reads the column again to tell delete from open.
    #[test]
    fn a_click_on_the_delete_control_still_names_its_row() {
        let rect = panel_rect(AREA, 8);
        let y = rect.y + 2 + 3;
        let col = rect.x + rect.width - 1;
        assert_eq!(row_at(rect, 0, 8, col, y), Some(3));
        assert!(in_control_col(rect, col));
    }

    #[test]
    fn a_short_list_never_scrolls() {
        let rect = panel_rect(AREA, 4);
        assert_eq!(scroll_top(0, list_height(rect)), 0);
    }
}
