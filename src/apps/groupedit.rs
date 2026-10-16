//! App Groups settings: create, edit and delete app groups — their name,
//! icon, start-menu section, and the frames they are made of — with a live
//! sketch of the arrangement.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::group::{self, arrange, Dock, FrameDef, GroupDef, FRAME_APPS, IDE};
use super::ui::{self, Controls, Item, Look};
use super::{paste_line, text_edit, Action, AppWindow, Launch};

const BG: Color = Color::Rgb(12, 14, 22);
const ACCENT: Color = Color::Rgb(0, 190, 255);
const WARN: Color = Color::Rgb(255, 170, 110);
const PANE_COLORS: [Color; 5] = [
    Color::Rgb(36, 66, 116),
    Color::Rgb(70, 48, 110),
    Color::Rgb(28, 88, 78),
    Color::Rgb(104, 62, 44),
    Color::Rgb(84, 84, 36),
];

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Name,
    Emoji,
    Section,
    Param(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    PickGroup,
    Group(usize),
    NewGroup,
    Duplicate,
    Delete,
    Reset,
    Edit(Field),
    ToggleTabs,
    App(usize),
    SetApp(usize, usize),
    Dock(usize),
    SetDock(usize, usize),
    Smaller(usize),
    Bigger(usize),
    Up(usize),
    Remove(usize),
    Add,
    Save,
    Try,
    Close,
    EditOk,
    EditCancel,
}

pub struct GroupEditor {
    groups: Vec<GroupDef>,
    cur: usize,
    editing: Option<(Field, String)>,
    dirty: bool,
    /// Close was clicked once with unsaved changes.
    close_armed: bool,
    status: String,
    ui: Controls<Btn>,
}

impl GroupEditor {
    pub fn new(select: Option<String>) -> Self {
        let mut groups = group::all();
        if let Some(id) = &select {
            // A running group whose definition was deleted can be recreated.
            if !groups.iter().any(|g| &g.id == id) {
                if let Some(g) = group::get(id) {
                    groups.push(g);
                }
            }
        }
        let cur = select
            .and_then(|id| groups.iter().position(|g| g.id == id))
            .unwrap_or(0);
        GroupEditor {
            groups,
            cur,
            editing: None,
            dirty: false,
            close_armed: false,
            status: String::new(),
            ui: Controls::new(),
        }
    }

    fn g(&mut self) -> Option<&mut GroupDef> {
        self.groups.get_mut(self.cur)
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.close_armed = false;
        self.status.clear();
    }

    fn save(&mut self) -> bool {
        match group::save_all(self.groups.clone()) {
            Ok(()) => {
                self.dirty = false;
                self.status = "✓ saved".into();
                true
            }
            Err(e) => {
                self.status = format!("✗ {e}");
                false
            }
        }
    }

    fn commit_edit(&mut self) {
        let Some((field, text)) = self.editing.take() else {
            return;
        };
        let text = text.trim().to_string();
        let Some(g) = self.g() else { return };
        match field {
            Field::Name if !text.is_empty() => g.name = text,
            Field::Emoji if !text.is_empty() => g.emoji = text.chars().take(2).collect(),
            Field::Section if !text.is_empty() => g.section = text,
            Field::Param(i) => {
                if let Some(f) = g.frames.get_mut(i) {
                    f.param = text;
                }
            }
            _ => return,
        }
        self.changed();
    }

    fn press(&mut self, b: Btn) -> Action {
        if b != Btn::Close {
            self.close_armed = false;
        }
        match b {
            Btn::PickGroup => {
                let mut items: Vec<(String, Option<Btn>)> = self
                    .groups
                    .iter()
                    .enumerate()
                    .map(|(i, g)| (format!("{} {}", g.emoji, g.name), Some(Btn::Group(i))))
                    .collect();
                items.push((String::new(), None));
                items.push(("+ New group".into(), Some(Btn::NewGroup)));
                self.ui.open_menu_under(Btn::PickGroup, items);
            }
            Btn::Group(i) => {
                self.cur = i.min(self.groups.len().saturating_sub(1));
                self.editing = None;
            }
            Btn::NewGroup => {
                self.groups.push(GroupDef::blank());
                self.cur = self.groups.len() - 1;
                self.changed();
                self.editing = Some((Field::Name, String::new()));
            }
            Btn::Duplicate => {
                if let Some(g) = self.groups.get(self.cur) {
                    let mut copy = g.clone();
                    copy.id = group::new_id();
                    copy.name = format!("{} copy", g.name);
                    self.groups.push(copy);
                    self.cur = self.groups.len() - 1;
                    self.changed();
                }
            }
            Btn::Delete => {
                if self.cur < self.groups.len() {
                    let g = self.groups.remove(self.cur);
                    self.cur = self.cur.min(self.groups.len().saturating_sub(1));
                    self.changed();
                    self.status = format!("deleted {} — Save to confirm", g.name);
                }
            }
            Btn::Reset => {
                if let Some(g) = self.g().filter(|g| g.id == IDE) {
                    *g = group::default_ide();
                    self.changed();
                }
            }
            Btn::Edit(field) => {
                let Some(g) = self.groups.get(self.cur) else {
                    return Action::None;
                };
                let text = match field {
                    Field::Name => g.name.clone(),
                    Field::Emoji => g.emoji.clone(),
                    Field::Section => g.section.clone(),
                    Field::Param(i) => g.frames.get(i).map(|f| f.param.clone()).unwrap_or_default(),
                };
                self.editing = Some((field, text));
            }
            Btn::ToggleTabs => {
                if let Some(g) = self.g() {
                    g.tabs = !g.tabs;
                    self.changed();
                }
            }
            Btn::App(i) => {
                let items = FRAME_APPS
                    .iter()
                    .enumerate()
                    .map(|(k, r)| (format!("{} {}", r.1, r.2), Some(Btn::SetApp(i, k))))
                    .collect();
                self.ui.open_menu_under(Btn::App(i), items);
            }
            Btn::SetApp(i, k) => {
                if let Some(f) = self.g().and_then(|g| g.frames.get_mut(i)) {
                    if f.app != FRAME_APPS[k].0 {
                        f.app = FRAME_APPS[k].0;
                        f.param.clear();
                    }
                    self.changed();
                }
            }
            Btn::Dock(i) => {
                let items = Dock::ALL
                    .iter()
                    .enumerate()
                    .map(|(k, d)| {
                        let what = if *d == Dock::Tabs {
                            "Tab in the middle".to_string()
                        } else {
                            format!("Docked {}", d.label().to_lowercase())
                        };
                        (what, Some(Btn::SetDock(i, k)))
                    })
                    .collect();
                self.ui.open_menu_under(Btn::Dock(i), items);
            }
            Btn::SetDock(i, k) => {
                if let Some(f) = self.g().and_then(|g| g.frames.get_mut(i)) {
                    let dock = Dock::ALL[k];
                    if dock.horizontal() != f.dock.horizontal() || f.dock == Dock::Tabs {
                        f.size = if dock.horizontal() { 28 } else { 10 };
                    }
                    f.dock = dock;
                    self.changed();
                }
            }
            Btn::Smaller(i) | Btn::Bigger(i) => {
                if let Some(f) = self.g().and_then(|g| g.frames.get_mut(i)) {
                    let step = if f.dock.horizontal() { 2 } else { 1 };
                    f.size = if matches!(b, Btn::Bigger(_)) {
                        (f.size + step).min(200)
                    } else {
                        f.size.saturating_sub(step).max(3)
                    };
                    self.changed();
                }
            }
            Btn::Up(i) => {
                if let Some(g) = self.g() {
                    if i > 0 && i < g.frames.len() {
                        g.frames.swap(i, i - 1);
                        self.changed();
                    }
                }
            }
            Btn::Remove(i) => {
                if let Some(g) = self.g() {
                    if i < g.frames.len() {
                        g.frames.remove(i);
                        self.changed();
                    }
                }
            }
            Btn::Add => {
                if let Some(g) = self.g() {
                    g.frames.push(FrameDef::new(group::FrameApp::Editor, Dock::Tabs, 0));
                    self.changed();
                }
            }
            Btn::Save => {
                self.save();
            }
            Btn::Try => {
                if self.save() {
                    if let Some(g) = self.groups.get(self.cur) {
                        return Action::Launch(Launch::Group {
                            id: g.id.clone(),
                            cwd: None,
                            open: Vec::new(),
                        });
                    }
                }
            }
            Btn::Close => {
                if self.dirty && !self.close_armed {
                    self.close_armed = true;
                    self.status = "unsaved changes — Save, or click Close again to discard".into();
                } else {
                    return Action::Close;
                }
            }
            Btn::EditOk => self.commit_edit(),
            Btn::EditCancel => self.editing = None,
        }
        Action::None
    }

    fn render_form(&mut self, buf: &mut Buffer, area: Rect) {
        let Some(g) = self.groups.get(self.cur).cloned() else {
            buf.set_string(
                area.x + 2,
                area.y + 2,
                "No app groups yet — pick ‘+ New group’ above.",
                Style::default().fg(ui::DIM).bg(BG),
            );
            return;
        };
        let x = area.x + 2;
        let vx = x + 12;
        let bottom = area.y + area.height;
        let label = |buf: &mut Buffer, y: u16, t: &str| {
            buf.set_string(x, y, t, Style::default().fg(ui::DIM).bg(BG));
        };
        let mut y = area.y + 1;
        label(buf, y, "Name");
        self.ui.button(buf, vx, y, &g.name, Btn::Edit(Field::Name), Look::Raised);
        y += 1;
        label(buf, y, "Icon");
        self.ui.button(buf, vx, y, &g.emoji, Btn::Edit(Field::Emoji), Look::Raised);
        y += 1;
        label(buf, y, "Menu");
        let w = self
            .ui
            .button(buf, vx, y, &g.section, Btn::Edit(Field::Section), Look::Raised);
        buf.set_string(
            vx + w + 1,
            y,
            "start-menu section",
            Style::default().fg(ui::DIM).bg(BG),
        );
        y += 1;
        label(buf, y, "Opened apps");
        let mark = if g.tabs { "☑" } else { "☐" };
        self.ui.button(
            buf,
            vx,
            y,
            &format!("{mark} open as tabs inside the group"),
            Btn::ToggleTabs,
            Look::on_if(g.tabs),
        );
        y += 2;
        buf.set_string(
            x,
            y,
            "Frames",
            Style::default().fg(ACCENT).bg(BG).add_modifier(Modifier::BOLD),
        );
        buf.set_string(
            x + 7,
            y,
            " docked from the outside in, in this order",
            Style::default().fg(ui::DIM).bg(BG),
        );
        y += 1;
        let app_w = FRAME_APPS.iter().map(|r| r.2.width()).max().unwrap_or(12);
        for (i, f) in g.frames.iter().enumerate() {
            if y + 2 > bottom {
                break;
            }
            buf.set_string(x, y, format!("{:>2}", i + 1), Style::default().fg(ui::DIM).bg(BG));
            let mut cx = x + 3;
            let name = format!("{} {:<app_w$} ▾", f.app.emoji(), f.app.name());
            cx += self.ui.button(buf, cx, y, &name, Btn::App(i), Look::Raised) + 1;
            let dock = format!("{:<6} ▾", f.dock.label());
            cx += self.ui.button(buf, cx, y, &dock, Btn::Dock(i), Look::Raised) + 1;
            if f.dock == Dock::Tabs {
                cx += 11;
            } else {
                cx += self.ui.button(buf, cx, y, "−", Btn::Smaller(i), Look::Raised);
                let unit = if f.dock.horizontal() { "col" } else { "row" };
                buf.set_string(cx, y, format!("{:>3}{unit} ", f.size), Style::default().fg(ui::FG).bg(BG));
                cx += 7;
                cx += self.ui.button(buf, cx, y, "+", Btn::Bigger(i), Look::Raised) + 1;
            }
            cx += self.ui.button(
                buf,
                cx,
                y,
                "↑",
                Btn::Up(i),
                Look::enabled_if(i > 0),
            );
            cx += self.ui.button(buf, cx, y, "✕", Btn::Remove(i), Look::Plain) + 1;
            let hint = f.app.param_hint();
            if !hint.is_empty() && cx + 6 < area.x + area.width {
                let room = (area.x + area.width - cx - 3) as usize;
                let shown = if f.param.is_empty() {
                    format!("‹{hint}›")
                } else {
                    f.param.clone()
                };
                let look = if f.param.is_empty() { Look::Plain } else { Look::Raised };
                self.ui
                    .button(buf, cx, y, &clip(&shown, room), Btn::Edit(Field::Param(i)), look);
            }
            y += 1;
        }
        if y + 1 < bottom {
            self.ui.button(buf, x + 3, y, "+ Add frame", Btn::Add, Look::Raised);
        }
    }
}

/// A small sketch of the arrangement, scaled from a typical 120x40 window.
fn render_preview(buf: &mut Buffer, r: Rect, g: &GroupDef) {
    if r.width < 16 || r.height < 6 {
        return;
    }
    let border = Style::default().fg(ui::DIM).bg(BG);
    buf.set_string(r.x, r.y, format!("╭{}╮", "─".repeat(r.width as usize - 2)), border);
    for y in r.y + 1..r.y + r.height - 1 {
        buf.set_string(r.x, y, "│", border);
        buf.set_string(r.x + r.width - 1, y, "│", border);
    }
    buf.set_string(
        r.x,
        r.y + r.height - 1,
        format!("╰{}╯", "─".repeat(r.width as usize - 2)),
        border,
    );
    buf.set_string(r.x + 2, r.y, " Preview ", Style::default().fg(ui::FG).bg(BG));
    let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, r.height - 2);
    let bar = Rect::new(inner.x, inner.y, inner.width, 1);
    ui::fill_row(buf, bar, ui::BAR_BG);
    buf.set_string(
        bar.x,
        bar.y,
        clip(&format!(" {} {}", g.emoji, g.name), bar.width as usize),
        Style::default().fg(ACCENT).bg(ui::BAR_BG),
    );
    let work = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    let sx = work.width as f32 / 120.0;
    let sy = work.height as f32 / 40.0;
    let docked: Vec<&FrameDef> = g.frames.iter().filter(|f| f.dock != Dock::Tabs).collect();
    let specs: Vec<(Dock, u16)> = docked
        .iter()
        .map(|f| {
            let s = if f.dock.horizontal() { sx } else { sy };
            (f.dock, ((f.size as f32 * s).round() as u16).max(2))
        })
        .collect();
    let (rects, rest) = arrange(work, &specs);
    for (i, ((pane, sep), f)) in rects.iter().zip(&docked).enumerate() {
        let bg = PANE_COLORS[i % PANE_COLORS.len()];
        for y in pane.y..pane.y + pane.height {
            ui::fill_row(buf, Rect::new(pane.x, y, pane.width, 1), bg);
        }
        for y in sep.y..sep.y + sep.height {
            ui::fill_row(buf, Rect::new(sep.x, y, sep.width, 1), BG);
        }
        if pane.width > 0 {
            let t = clip(&format!("{} {}", f.app.emoji(), f.app.name()), pane.width as usize);
            let tx = pane.x + pane.width.saturating_sub(t.width() as u16) / 2;
            buf.set_string(tx, pane.y + pane.height / 2, t, Style::default().fg(Color::White).bg(bg));
        }
    }
    let tabs: Vec<String> = g
        .frames
        .iter()
        .filter(|f| f.dock == Dock::Tabs)
        .map(|f| format!("{} {}", f.app.emoji(), f.app.name()))
        .collect();
    let strip = Rect::new(rest.x, rest.y, rest.width, 1);
    ui::fill_row(buf, strip, ui::BAR_BG);
    let label = if tabs.is_empty() {
        "tabs".to_string()
    } else {
        tabs.join(" │ ")
    };
    buf.set_string(
        strip.x + 1,
        strip.y,
        clip(&label, rest.width.saturating_sub(1) as usize),
        Style::default().fg(ui::FG).bg(ui::BAR_BG),
    );
    if rest.height > 2 {
        let t = if g.tabs { "apps opened inside" } else { "" };
        let tx = rest.x + rest.width.saturating_sub(t.width() as u16) / 2;
        buf.set_string(tx, rest.y + rest.height / 2, clip(t, rest.width as usize), Style::default().fg(ui::DIM).bg(BG));
    }
}

fn clip(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if out.width() + 2 > w {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

impl AppWindow for GroupEditor {
    fn title(&self) -> String {
        let dirty = if self.dirty { " •" } else { "" };
        format!("App Groups{dirty}")
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        self.ui.begin();
        let buf = f.buffer_mut();
        for y in area.y..area.y + area.height {
            ui::fill_row(buf, Rect::new(area.x, y, area.width, 1), BG);
        }
        if area.height < 4 {
            return;
        }
        let cur = self.groups.get(self.cur);
        let pick = match cur {
            Some(g) => format!("{} {} ▾", g.emoji, g.name),
            None => "choose ▾".into(),
        };
        let is_ide = cur.is_some_and(|g| g.id == IDE);
        let mut right = vec![Item::with("⧉ Duplicate", Btn::Duplicate, Look::enabled_if(cur.is_some()))];
        if is_ide {
            right.push(Item::with("↺ Reset", Btn::Reset, Look::Plain));
        }
        right.push(Item::with(
            "🗑 Delete",
            Btn::Delete,
            if cur.is_some() { Look::Danger } else { Look::Disabled },
        ));
        self.ui.toolbar(
            buf,
            Rect::new(area.x, area.y, area.width, 1),
            &[
                Item::Label(" App group".into(), ui::FG),
                Item::with(pick, Btn::PickGroup, Look::Raised),
                Item::with("+ New", Btn::NewGroup, Look::Plain),
            ],
            &right,
        );

        // Form on the left, preview on the right when there is room.
        let body = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(2));
        let wide = area.width >= 110;
        let form_w = if wide { area.width - 40 } else { area.width };
        self.render_form(buf, Rect::new(body.x, body.y, form_w, body.height));
        if let Some(g) = self.groups.get(self.cur) {
            let n = g.frames.len() as u16;
            if wide {
                let r = Rect::new(body.x + form_w, body.y + 1, 38, body.height.saturating_sub(2).min(18));
                render_preview(buf, r, g);
            } else {
                // Under the frames list: 8 form rows, the frames, Add.
                let top = body.y + 9 + n + 1;
                if top + 7 < body.y + body.height {
                    let h = (body.y + body.height - top).min(14);
                    let w = area.width.saturating_sub(4).min(60);
                    render_preview(buf, Rect::new(body.x + 2, top, w, h), g);
                }
            }
        }

        // Bottom: inline editor, or status with Save / Try / Close.
        let bottom = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        if let Some((field, text)) = &self.editing {
            let label = match field {
                Field::Name => "Name".to_string(),
                Field::Emoji => "Icon (emoji)".to_string(),
                Field::Section => "Start-menu section".to_string(),
                Field::Param(i) => self
                    .groups
                    .get(self.cur)
                    .and_then(|g| g.frames.get(*i))
                    .map(|f| format!("{} — {}", f.app.name(), f.app.param_hint()))
                    .unwrap_or_default(),
            };
            let text = text.clone();
            self.ui
                .input_row(buf, bottom, &label, &text, ("OK", Btn::EditOk), Btn::EditCancel);
        } else {
            let status = if self.status.is_empty() {
                Vec::new()
            } else {
                let c = if self.status.starts_with('✓') { Color::Rgb(110, 230, 140) } else { WARN };
                vec![Item::Label(format!(" {}", self.status), c)]
            };
            self.ui.toolbar(
                buf,
                bottom,
                &status,
                &[
                    Item::with(
                        "💾 Save",
                        Btn::Save,
                        if self.dirty { Look::Primary } else { Look::Disabled },
                    ),
                    Item::with("▶ Try it", Btn::Try, Look::enabled_if(self.groups.get(self.cur).is_some())),
                    Item::with("Close", Btn::Close, Look::Raised),
                ],
            );
        }
        self.ui.render_menu(f.buffer_mut(), area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        if let Some((_, text)) = &mut self.editing {
            match k.code {
                KeyCode::Enter => self.commit_edit(),
                KeyCode::Esc => self.editing = None,
                _ => {
                    text_edit(text, k);
                }
            }
            return Action::None;
        }
        if k.code == KeyCode::Esc {
            if self.ui.menu_open() {
                self.ui.close_menu();
                return Action::None;
            }
            return self.press(Btn::Close);
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        match self.ui.route(&m) {
            Some(Some(b)) => self.press(b),
            _ => Action::None,
        }
    }

    fn on_paste(&mut self, text: &str) -> Action {
        if let Some((_, buf)) = &mut self.editing {
            paste_line(buf, text);
        }
        Action::None
    }
}
