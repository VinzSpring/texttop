use std::path::PathBuf;
use std::time::Instant;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear};
use ratatui::Frame;

use crate::apps::{
    about::About, audioplayer::AudioPlayer, editor::Editor, filebrowser::FileBrowser,
    imageviewer::ImageViewer, launcher::AppLauncher, terminal::Terminal,
    videoplayer::VideoPlayer, webbrowser::WebBrowser, Action, AppWindow, Launch,
};
use crate::gfx;

const TASKBAR_BG: Color = Color::Rgb(14, 16, 28);
const ACCENT: Color = Color::Rgb(0, 190, 255);
const MENU_BG: Color = Color::Rgb(18, 22, 38);
const WIN_BG: Color = Color::Rgb(12, 14, 22);

const THEMES: &[((u8, u8, u8), (u8, u8, u8), &str)] = &[
    ((10, 14, 44), (66, 14, 78), "midnight"),
    ((3, 30, 52), (0, 96, 116), "ocean"),
    ((34, 8, 10), (140, 66, 12), "sunset"),
    ((6, 30, 18), (22, 88, 54), "forest"),
];

struct Win {
    id: u64,
    rect: Rect,
    minimized: bool,
    restore: Option<Rect>,
    icon: &'static str,
    app: Box<dyn AppWindow>,
}

enum Drag {
    Move { id: u64, dx: u16 },
    Resize { id: u64 },
}

#[derive(Clone, Copy, PartialEq)]
enum MenuAction {
    Files,
    Web,
    Terminal,
    Editor,
    Wallpaper,
    About,
    Quit,
}

const MENU: &[(&str, MenuAction)] = &[
    ("🗂  File Manager", MenuAction::Files),
    ("🌐  Web Browser", MenuAction::Web),
    ("🖥  Terminal", MenuAction::Terminal),
    ("📝  Text Editor", MenuAction::Editor),
    ("🎨  Wallpaper", MenuAction::Wallpaper),
    ("✨  About & Help", MenuAction::About),
    ("⏻  Exit texttop", MenuAction::Quit),
];

const ICONS: &[(&str, &str, MenuAction)] = &[
    ("🗂", "Files", MenuAction::Files),
    ("🌐", "Web", MenuAction::Web),
    ("🖥", "Terminal", MenuAction::Terminal),
    ("📝", "Editor", MenuAction::Editor),
    ("✨", "About", MenuAction::About),
];

pub struct Desktop {
    pub should_quit: bool,
    windows: Vec<Win>,
    next_id: u64,
    start_open: bool,
    start_sel: usize,
    icon_sel: usize,
    last_click: Option<(Instant, i32)>,
    drag: Option<Drag>,
    screen: Rect,
    menu_rect: Rect,
    start_w: u16,
    taskbar_hits: Vec<(u16, u16, u64)>,
    theme: usize,
}

impl Desktop {
    pub fn new() -> Self {
        let mut d = Desktop {
            should_quit: false,
            windows: Vec::new(),
            next_id: 0,
            start_open: false,
            start_sel: 0,
            icon_sel: 0,
            last_click: None,
            drag: None,
            screen: Rect::new(0, 0, 120, 36),
            menu_rect: Rect::default(),
            start_w: 9,
            taskbar_hits: Vec::new(),
            theme: 0,
        };
        d.spawn(Launch::About);
        d
    }

    // ---------------------------------------------------------- rendering

    pub fn render(&mut self, f: &mut Frame) {
        let area = f.area();
        self.screen = area;
        self.sanitize();

        let taskbar_y = area.height.saturating_sub(1);
        let desk = Rect::new(0, 0, area.width, taskbar_y);
        let th = THEMES[self.theme % THEMES.len()];

        {
            let buf = f.buffer_mut();
            gfx::gradient(buf, desk, th.0, th.1);

            // watermark
            if desk.width > 46 && desk.height > 12 {
                let wm = "· t e x t t o p ·";
                let sub = "your terminal, but cozier";
                let x = desk.width.saturating_sub(wm.len() as u16) / 2;
                let y = desk.height / 2;
                buf.set_string(
                    x,
                    y,
                    wm,
                    Style::default()
                        .fg(Color::Rgb(
                            gfx::lerp(th.0 .0, 255, 0.30),
                            gfx::lerp(th.0 .1, 255, 0.30),
                            gfx::lerp(th.0 .2, 255, 0.30),
                        ))
                        .add_modifier(Modifier::BOLD),
                );
                let x2 = desk.width.saturating_sub(sub.len() as u16) / 2;
                buf.set_string(
                    x2,
                    y + 1,
                    sub,
                    Style::default().fg(Color::Rgb(
                        gfx::lerp(th.0 .0, 255, 0.18),
                        gfx::lerp(th.0 .1, 255, 0.18),
                        gfx::lerp(th.0 .2, 255, 0.18),
                    )),
                );
            }

            // desktop icons
            for (i, (emoji, label, _)) in ICONS.iter().enumerate() {
                let r = self.icon_rect(i);
                if r.y + r.height > desk.height {
                    break;
                }
                let selected = i == self.icon_sel && self.top_idx().is_none();
                if selected {
                    for y in r.y..r.y + r.height {
                        for x in r.x..r.x + r.width {
                            if let Some(c) = buf.cell_mut((x, y)) {
                                c.set_bg(Color::Rgb(50, 70, 110));
                            }
                        }
                    }
                }
                buf.set_string(
                    r.x + r.width / 2 - 1,
                    r.y,
                    *emoji,
                    Style::default().fg(Color::White),
                );
                let lx = r.x + (r.width.saturating_sub(label.len() as u16)) / 2;
                buf.set_string(
                    lx,
                    r.y + 2,
                    *label,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(if selected { Modifier::BOLD } else { Modifier::empty() }),
                );
            }
        }

        // windows, bottom to top
        let top = self.top_idx();
        for i in 0..self.windows.len() {
            if self.windows[i].minimized {
                continue;
            }
            let rect = self.windows[i].rect;
            let focused = Some(i) == top;
            gfx::shadow(f.buffer_mut(), rect, desk);
            f.render_widget(Clear, rect);

            let border_style = if focused {
                Style::default().fg(ACCENT)
            } else {
                Style::default().fg(Color::Rgb(82, 86, 112))
            };
            let title_style = if focused {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(150, 152, 172))
            };
            let title = Line::from(vec![Span::styled(
                format!(" {} {} ", self.windows[i].icon, self.windows[i].app.title()),
                title_style,
            )]);
            let buttons = Line::from(vec![
                Span::styled("─", Style::default().fg(Color::Rgb(255, 200, 80))),
                Span::raw(" "),
                Span::styled("□", Style::default().fg(Color::Rgb(120, 220, 130))),
                Span::raw(" "),
                Span::styled("✕", Style::default().fg(Color::Rgb(255, 95, 95))),
                Span::raw(" "),
            ])
            .right_aligned();
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .title(title)
                .title_top(buttons)
                .style(Style::default().bg(WIN_BG));
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            self.windows[i].app.render(f, inner, focused);

            // resize grip
            let gx = rect.x + rect.width - 1;
            let gy = rect.y + rect.height - 1;
            if let Some(c) = f.buffer_mut().cell_mut((gx, gy)) {
                c.set_symbol("◢");
                c.set_fg(if focused { ACCENT } else { Color::Rgb(82, 86, 112) });
            }
        }

        self.render_taskbar(f, taskbar_y);

        if self.start_open {
            self.render_start_menu(f, taskbar_y);
        }
    }

    fn render_taskbar(&mut self, f: &mut Frame, y: u16) {
        let area = self.screen;
        let buf = f.buffer_mut();
        for x in 0..area.width {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_symbol(" ");
                c.set_bg(TASKBAR_BG);
            }
        }
        let start_label = " ⊞ Start ";
        self.start_w = Span::raw(start_label).width() as u16;
        let start_style = if self.start_open {
            Style::default()
                .fg(Color::White)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(0, 96, 160))
                .add_modifier(Modifier::BOLD)
        };
        buf.set_string(0, y, start_label, start_style);

        self.taskbar_hits.clear();
        let mut x = self.start_w + 1;
        let top = self.top_idx();
        for (i, w) in self.windows.iter().enumerate() {
            let mut t = w.app.title();
            if t.chars().count() > 14 {
                t = t.chars().take(13).collect::<String>() + "…";
            }
            let label = format!(" {} {} ", w.icon, t);
            let wdt = Span::raw(&label).width() as u16;
            if x + wdt >= area.width.saturating_sub(18) {
                break;
            }
            let style = if Some(i) == top {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(42, 70, 112))
                    .add_modifier(Modifier::BOLD)
            } else if w.minimized {
                Style::default()
                    .fg(Color::Rgb(120, 122, 140))
                    .bg(TASKBAR_BG)
                    .add_modifier(Modifier::ITALIC)
            } else {
                Style::default().fg(Color::Rgb(190, 192, 205)).bg(TASKBAR_BG)
            };
            buf.set_string(x, y, &label, style);
            self.taskbar_hits.push((x, x + wdt, w.id));
            x += wdt + 1;
        }

        let clock = chrono::Local::now().format(" %H:%M  %a %d %b ").to_string();
        let cw = clock.len() as u16;
        if area.width > cw {
            buf.set_string(
                area.width - cw,
                y,
                &clock,
                Style::default().fg(Color::Rgb(200, 212, 235)).bg(TASKBAR_BG),
            );
        }
    }

    fn render_start_menu(&mut self, f: &mut Frame, taskbar_y: u16) {
        let h = MENU.len() as u16 + 2;
        let w = 30u16.min(self.screen.width.saturating_sub(2));
        let rect = Rect::new(1, taskbar_y.saturating_sub(h), w, h);
        self.menu_rect = rect;
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(ACCENT))
            .title(Span::styled(
                " ⊞ texttop ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(MENU_BG));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let buf = f.buffer_mut();
        for (i, (label, action)) in MENU.iter().enumerate() {
            let iy = inner.y + i as u16;
            if iy >= inner.y + inner.height {
                break;
            }
            let mut text = format!(" {label}");
            if *action == MenuAction::Wallpaper {
                text = format!(" {label}  · {}", THEMES[self.theme % THEMES.len()].2);
            }
            let style = if i == self.start_sel {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(0, 110, 170))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(210, 214, 228)).bg(MENU_BG)
            };
            let pad = " ".repeat(inner.width as usize);
            buf.set_string(inner.x, iy, &pad, style);
            buf.set_string(inner.x, iy, &text, style);
        }
    }

    // ---------------------------------------------------------- input

    pub fn on_key(&mut self, k: KeyEvent) {
        if self.start_open {
            match k.code {
                KeyCode::Esc | KeyCode::F(1) => self.start_open = false,
                KeyCode::Up => {
                    self.start_sel = self.start_sel.checked_sub(1).unwrap_or(MENU.len() - 1)
                }
                KeyCode::Down => self.start_sel = (self.start_sel + 1) % MENU.len(),
                KeyCode::Enter => self.activate_menu(MENU[self.start_sel].1),
                _ => {}
            }
            return;
        }

        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::F(1) => {
                self.start_open = true;
                self.start_sel = 0;
                return;
            }
            KeyCode::F(6) => {
                if self.windows.len() > 1 {
                    let w = self.windows.pop().unwrap();
                    self.windows.insert(0, w);
                    if let Some(i) = self.top_idx() {
                        let w = self.windows.remove(i);
                        self.windows.push(w);
                    }
                }
                return;
            }
            KeyCode::F(11) => {
                if let Some(i) = self.top_idx() {
                    self.toggle_max(i);
                }
                return;
            }
            KeyCode::Char('w') if ctrl => {
                if let Some(i) = self.top_idx() {
                    let id = self.windows[i].id;
                    self.close_id(id);
                }
                return;
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if ctrl => {
                if let Some(i) = self.top_idx() {
                    let taskbar_y = self.screen.height.saturating_sub(1);
                    let r = &mut self.windows[i].rect;
                    match k.code {
                        KeyCode::Left => r.x = r.x.saturating_sub(2),
                        KeyCode::Right => {
                            r.x = (r.x + 2).min(self.screen.width.saturating_sub(r.width))
                        }
                        KeyCode::Up => r.y = r.y.saturating_sub(1),
                        KeyCode::Down => r.y = (r.y + 1).min(taskbar_y.saturating_sub(r.height)),
                        _ => {}
                    }
                }
                return;
            }
            _ => {}
        }

        if let Some(i) = self.top_idx() {
            let action = self.windows[i].app.on_key(k);
            let id = self.windows[i].id;
            self.process(id, action);
        } else {
            // desktop icon navigation
            match k.code {
                KeyCode::Up | KeyCode::Left => {
                    self.icon_sel = self.icon_sel.checked_sub(1).unwrap_or(ICONS.len() - 1)
                }
                KeyCode::Down | KeyCode::Right => self.icon_sel = (self.icon_sel + 1) % ICONS.len(),
                KeyCode::Enter => self.activate_menu(ICONS[self.icon_sel].2),
                _ => {}
            }
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        let (col, row) = (m.column, m.row);
        let taskbar_y = self.screen.height.saturating_sub(1);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if row == taskbar_y {
                    if col < self.start_w {
                        self.start_open = !self.start_open;
                        self.start_sel = 0;
                        return;
                    }
                    let hits = self.taskbar_hits.clone();
                    for (x0, x1, id) in hits {
                        if col >= x0 && col < x1 {
                            if let Some(i) = self.windows.iter().position(|w| w.id == id) {
                                if self.windows[i].minimized {
                                    self.windows[i].minimized = false;
                                    let w = self.windows.remove(i);
                                    self.windows.push(w);
                                } else if Some(i) == self.top_idx() {
                                    self.windows[i].minimized = true;
                                } else {
                                    let w = self.windows.remove(i);
                                    self.windows.push(w);
                                }
                            }
                            return;
                        }
                    }
                    self.start_open = false;
                    return;
                }
                if self.start_open {
                    let r = self.menu_rect;
                    if col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height {
                        if row > r.y && row < r.y + r.height - 1 {
                            let idx = (row - r.y - 1) as usize;
                            if idx < MENU.len() {
                                self.start_sel = idx;
                                self.activate_menu(MENU[idx].1);
                            }
                        }
                    } else {
                        self.start_open = false;
                    }
                    return;
                }
                if let Some(i) = self
                    .windows
                    .iter()
                    .rposition(|w| !w.minimized && contains(w.rect, col, row))
                {
                    let w = self.windows.remove(i);
                    self.windows.push(w);
                    let i = self.windows.len() - 1;
                    let rect = self.windows[i].rect;
                    let id = self.windows[i].id;
                    let right = rect.x + rect.width;
                    if row == rect.y {
                        if col >= right.saturating_sub(4) && col < right {
                            self.close_id(id);
                        } else if col >= right.saturating_sub(6) {
                            self.toggle_max(i);
                        } else if col >= right.saturating_sub(8) {
                            self.windows[i].minimized = true;
                        } else {
                            self.drag = Some(Drag::Move {
                                id,
                                dx: col - rect.x,
                            });
                        }
                        return;
                    }
                    if col >= right.saturating_sub(2) && row == rect.y + rect.height - 1 {
                        self.drag = Some(Drag::Resize { id });
                        return;
                    }
                    let inner =
                        Rect::new(rect.x + 1, rect.y + 1, rect.width.saturating_sub(2), rect.height.saturating_sub(2));
                    let action = self.windows[i].app.on_mouse(m, inner);
                    self.process(id, action);
                    return;
                }
                // desktop icons
                for i in 0..ICONS.len() {
                    let r = self.icon_rect(i);
                    if contains(r, col, row) {
                        let dbl = matches!(self.last_click, Some((t, li))
                            if li == i as i32 && t.elapsed().as_millis() < 450);
                        self.icon_sel = i;
                        if dbl {
                            self.activate_menu(ICONS[i].2);
                            self.last_click = None;
                        } else {
                            self.last_click = Some((Instant::now(), i as i32));
                        }
                        return;
                    }
                }
                self.last_click = Some((Instant::now(), -1));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let taskbar_y = self.screen.height.saturating_sub(1);
                match &self.drag {
                    Some(Drag::Move { id, dx }) => {
                        let (id, dx) = (*id, *dx);
                        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                            let max_x = self.screen.width.saturating_sub(w.rect.width);
                            w.rect.x = col.saturating_sub(dx).min(max_x);
                            w.rect.y = row.min(taskbar_y.saturating_sub(w.rect.height));
                            w.restore = None;
                        }
                    }
                    Some(Drag::Resize { id }) => {
                        let id = *id;
                        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
                            let max_w = self.screen.width.saturating_sub(w.rect.x);
                            let max_h = taskbar_y.saturating_sub(w.rect.y);
                            w.rect.width = (col.saturating_sub(w.rect.x) + 1).clamp(24, max_w.max(24));
                            w.rect.height = (row.saturating_sub(w.rect.y) + 1).clamp(6, max_h.max(6));
                            w.restore = None;
                        }
                    }
                    // in-content drag (e.g. browser text selection / scrubbers)
                    None => self.forward_mouse_to_content(m, col, row),
                }
            }
            MouseEventKind::Up(_) => {
                let chrome_drag = self.drag.take().is_some();
                if !chrome_drag {
                    self.forward_mouse_to_content(m, col, row);
                }
            }
            MouseEventKind::Moved
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown => {
                self.forward_mouse_to_content(m, col, row);
            }
            _ => {}
        }
    }

    /// Deliver a mouse event to the topmost window's content area under the
    /// cursor (title-bar / border chrome is ignored so hover doesn't steal
    /// window-management gestures).
    fn forward_mouse_to_content(&mut self, m: MouseEvent, col: u16, row: u16) {
        if let Some(i) = self
            .windows
            .iter()
            .rposition(|w| !w.minimized && contains(w.rect, col, row))
        {
            let rect = self.windows[i].rect;
            // content is inside the frame: skip title row and border cells
            if row <= rect.y || row >= rect.y + rect.height.saturating_sub(1) {
                return;
            }
            if col <= rect.x || col >= rect.x + rect.width.saturating_sub(1) {
                return;
            }
            let inner = Rect::new(
                rect.x + 1,
                rect.y + 1,
                rect.width.saturating_sub(2),
                rect.height.saturating_sub(2),
            );
            let id = self.windows[i].id;
            let action = self.windows[i].app.on_mouse(m, inner);
            self.process(id, action);
        }
    }

    pub fn on_tick(&mut self) {
        for w in &mut self.windows {
            w.app.on_tick();
        }
    }

    // ---------------------------------------------------------- helpers

    fn icon_rect(&self, i: usize) -> Rect {
        Rect::new(2, 1 + i as u16 * 4, 12, 3)
    }

    fn top_idx(&self) -> Option<usize> {
        self.windows.iter().rposition(|w| !w.minimized)
    }

    fn activate_menu(&mut self, action: MenuAction) {
        match action {
            MenuAction::Files => {
                let root = if cfg!(windows) { "C:\\" } else { "/" };
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from(root));
                self.spawn(Launch::Files(home));
                self.start_open = false;
            }
            MenuAction::Web => {
                self.spawn(Launch::Web(None));
                self.start_open = false;
            }
            MenuAction::Terminal => {
                self.spawn(Launch::Terminal {
                    program: None,
                    args: Vec::new(),
                });
                self.start_open = false;
            }
            MenuAction::Editor => {
                self.spawn(Launch::Editor(None));
                self.start_open = false;
            }
            MenuAction::Wallpaper => {
                self.theme = (self.theme + 1) % THEMES.len();
            }
            MenuAction::About => {
                self.spawn(Launch::About);
                self.start_open = false;
            }
            MenuAction::Quit => self.should_quit = true,
        }
    }

    fn spawn(&mut self, launch: Launch) {
        let (app, icon, w, h): (Box<dyn AppWindow>, &'static str, u16, u16) = match launch {
            Launch::Files(p) => (Box::new(FileBrowser::new(p)), "🗂", 66, 24),
            Launch::Web(u) => (Box::new(WebBrowser::new(u)), "🌐", 94, 32),
            Launch::Audio(p) => (Box::new(AudioPlayer::new(p)), "🎵", 58, 17),
            Launch::Image(p) => (Box::new(ImageViewer::new(p)), "🖼", 78, 30),
            Launch::Video(p) => (Box::new(VideoPlayer::new(p)), "🎬", 86, 30),
            Launch::Editor(p) => (Box::new(Editor::new(p)), "📝", 76, 26),
            Launch::Terminal { program, args } => {
                (Box::new(Terminal::new(program, args)), "🖥", 84, 26)
            }
            Launch::AppLauncher(p) => (Box::new(AppLauncher::new(p)), "🧩", 68, 20),
            Launch::About => (Box::new(About::new()), "✨", 68, 24),
        };
        let scr = self.screen;
        let w = w.min(scr.width.saturating_sub(4).max(24));
        let h = h.min(scr.height.saturating_sub(4).max(6));
        let n = self.next_id as u16;
        let max_x = scr.width.saturating_sub(w + 2).max(1);
        let max_y = scr.height.saturating_sub(h + 3).max(1);
        let x = (16 + n * 4) % max_x;
        let y = (1 + n * 2) % max_y;
        self.windows.push(Win {
            id: self.next_id,
            rect: Rect::new(x.max(1), y.max(1), w, h),
            minimized: false,
            restore: None,
            icon,
            app,
        });
        self.next_id += 1;
    }

    fn toggle_max(&mut self, i: usize) {
        let taskbar_y = self.screen.height.saturating_sub(1);
        if let Some(prev) = self.windows[i].restore.take() {
            self.windows[i].rect = prev;
        } else {
            self.windows[i].restore = Some(self.windows[i].rect);
            self.windows[i].rect = Rect::new(0, 0, self.screen.width, taskbar_y);
        }
    }

    fn close_id(&mut self, id: u64) {
        self.windows.retain(|w| w.id != id);
    }

    pub fn open_path(&mut self, p: PathBuf) {
        let launch = if p.is_dir() {
            Launch::Files(p)
        } else {
            crate::apps::launch_for_path(&p)
        };
        self.spawn(launch);
    }

    fn process(&mut self, id: u64, action: Action) {
        match action {
            Action::None => {}
            Action::Close => self.close_id(id),
            Action::Launch(l) => self.spawn(l),
        }
    }

    fn sanitize(&mut self) {
        let scr = self.screen;
        if scr.width < 30 || scr.height < 8 {
            return;
        }
        let taskbar_y = scr.height.saturating_sub(1);
        for w in &mut self.windows {
            w.rect.width = w.rect.width.clamp(24, scr.width);
            w.rect.height = w.rect.height.clamp(6, taskbar_y.max(6));
            if w.rect.x + w.rect.width > scr.width {
                w.rect.x = scr.width.saturating_sub(w.rect.width);
            }
            if w.rect.y + w.rect.height > taskbar_y {
                w.rect.y = taskbar_y.saturating_sub(w.rect.height);
            }
        }
    }
}

fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}
