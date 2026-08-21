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

use super::{Action, AppWindow};
use crate::engine::{js_escape, Cmd, Engine, MouseKind, Status};
use crate::gfx;

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
</style></head><body><h1>texttop</h1><p>click a tile — or press Ctrl+L and type a URL</p><div>
<a href="https://lite.duckduckgo.com/lite/">🔎 DuckDuckGo</a>
<a href="https://en.wikipedia.org">📚 Wikipedia</a>
<a href="https://news.ycombinator.com">🗞 Hacker News</a>
<a href="https://www.youtube.com">🎬 YouTube</a>
</div></body></html>"#;

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
}

impl WebBrowser {
    pub fn new(initial: Option<String>) -> Self {
        let init_cmd = match initial {
            Some(u) => Cmd::Navigate(normalize_url(&u)),
            None => Cmd::NavigateHtml(HOME_HTML.to_string()),
        };
        let engine = Engine::start(init_cmd, 1.0, 720, 480);
        engine.send(Cmd::TextOverlay(true));
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
        }
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
        let bar_bg = if self.editing {
            Color::Rgb(0, 70, 110)
        } else {
            Color::Rgb(26, 32, 56)
        };
        let shown = if self.editing {
            format!(" 🌐 {}▏", self.url_input)
        } else {
            let url = if info.url.is_empty() || info.url.starts_with("about:") {
                "─ F4 for URL ─".to_string()
            } else {
                info.url.clone()
            };
            format!(" 🌐 {url}")
        };
        f.render_widget(
            Paragraph::new(Span::styled(shown, Style::default().fg(Color::White).bg(bar_bg)))
                .style(Style::default().bg(bar_bg)),
            url_bar,
        );

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

        // status bar
        let hint = if self.editing {
            " Enter go · Esc cancel ".to_string()
        } else {
            format!(
                " zoom {:.2} │ F4 url · F3 text mode · F5 reload · Ctrl+B/F back/fwd · F9/F10 zoom · click & type ",
                self.zoom
            )
        };
        let left = if !self.note.is_empty() {
            self.note.clone()
        } else {
            info.title
        };
        let dim = if focused {
            Color::Rgb(140, 150, 175)
        } else {
            Color::Rgb(100, 105, 125)
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" {}  │{}", left.chars().take(40).collect::<String>(), hint),
                Style::default().fg(dim).bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            status_bar,
        );
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

        match k.code {
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
            KeyCode::Char('b') if ctrl => self.engine.send(Cmd::Back),
            KeyCode::Char('f') if ctrl => self.engine.send(Cmd::Forward),
            KeyCode::F(5) => self.engine.send(Cmd::Reload),
            KeyCode::F(3) => {
                self.overlay = !self.overlay;
                self.engine.send(Cmd::TextOverlay(self.overlay));
                self.note = if self.overlay {
                    "crisp text overlay ON".into()
                } else {
                    "crisp text overlay OFF (pure pixels)".into()
                };
            }
            KeyCode::F(9) => {
                self.zoom = (self.zoom - 0.25).max(0.5);
                self.cache.clear();
                self.engine.send(Cmd::Zoom(self.zoom));
            }
            KeyCode::F(10) => {
                self.zoom = (self.zoom + 0.25).min(5.0);
                self.cache.clear();
                self.engine.send(Cmd::Zoom(self.zoom));
            }
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
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if m.row == area.y {
                    // click on url bar
                    self.editing = true;
                    self.url_input.clear();
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
                if let Some((x, y)) = content_px(&m, content, false) {
                    self.engine
                        .send(Cmd::Mouse { kind: MouseKind::Move, x, y });
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
