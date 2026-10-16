//! Clickable controls shared by the built-in apps: toolbar buttons, large
//! buttons, dropdown menus and input rows with OK / Cancel.
//!
//! Every app action is reachable through one of these. Keys still work as
//! accelerators, but nothing on screen asks the user to memorise them.
//!
//! The pattern is immediate mode: an app owns one `Controls<T>` (with `T` its
//! own action enum), calls `begin` at the top of `render`, draws controls,
//! and finishes with `render_menu` so an open dropdown sits over everything.
//! `on_mouse` feeds the event to `click`, which returns the action clicked.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

pub const BAR_BG: Color = Color::Rgb(28, 36, 62);
pub const FG: Color = Color::Rgb(210, 218, 240);
pub const DIM: Color = Color::Rgb(90, 100, 130);
pub const HOVER_BG: Color = Color::Rgb(0, 90, 140);
pub const ON_BG: Color = Color::Rgb(0, 110, 170);
pub const BTN_BG: Color = Color::Rgb(44, 54, 88);
pub const PRIMARY_BG: Color = Color::Rgb(30, 120, 90);
pub const DANGER_BG: Color = Color::Rgb(150, 50, 60);
pub const MENU_BG: Color = Color::Rgb(22, 26, 44);
pub const MENU_BORDER: Color = Color::Rgb(160, 170, 200);

/// How a control is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    /// Flat on the bar; highlights on hover.
    Plain,
    /// Raised chip, for buttons that stand alone.
    Raised,
    /// Toggled on / the current choice.
    On,
    /// Not available right now: drawn dim and never clickable.
    Disabled,
    /// The main thing to do here.
    Primary,
    /// Destructive.
    Danger,
}

impl Look {
    pub fn on_if(cond: bool) -> Look {
        if cond {
            Look::On
        } else {
            Look::Plain
        }
    }

    pub fn enabled_if(cond: bool) -> Look {
        if cond {
            Look::Plain
        } else {
            Look::Disabled
        }
    }
}

/// One entry of a toolbar.
pub enum Item<T> {
    Button(String, T, Look),
    /// Thin divider between groups.
    Sep,
    /// Non-clickable text (a title, a counter).
    Label(String, Color),
}

impl<T> Item<T> {
    pub fn btn(label: impl Into<String>, id: T) -> Self {
        Item::Button(label.into(), id, Look::Plain)
    }

    pub fn with(label: impl Into<String>, id: T, look: Look) -> Self {
        Item::Button(label.into(), id, look)
    }

    fn width(&self) -> u16 {
        match self {
            Item::Button(l, ..) => l.width() as u16 + 2,
            Item::Sep => 1,
            Item::Label(l, _) => l.width() as u16 + 1,
        }
    }
}

/// A dropdown list, open below its anchor.
struct OpenMenu<T> {
    rect: Rect,
    items: Vec<(String, Option<T>)>,
}

/// Hit boxes and pointer state for one window's controls.
pub struct Controls<T> {
    hits: Vec<(Rect, T)>,
    pointer: Option<(u16, u16)>,
    menu: Option<OpenMenu<T>>,
}

impl<T: Copy> Default for Controls<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy> Controls<T> {
    pub fn new() -> Self {
        Controls {
            hits: Vec::new(),
            pointer: None,
            menu: None,
        }
    }

    /// Forget last frame's hit boxes. Call at the top of `render`.
    pub fn begin(&mut self) {
        self.hits.clear();
    }

    /// Register a clickable region drawn by the app itself.
    pub fn add(&mut self, r: Rect, id: T) {
        if r.width > 0 && r.height > 0 {
            self.hits.push((r, id));
        }
    }

    pub fn hot(&self, r: Rect) -> bool {
        self.pointer
            .is_some_and(|(x, y)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
    }

    /// The control under a cell, topmost (last drawn) first.
    pub fn at(&self, col: u16, row: u16) -> Option<T> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height)
            .map(|(_, id)| *id)
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    pub fn close_menu(&mut self) {
        self.menu = None;
    }

    /// Feed a mouse event. Returns the control a left click landed on.
    ///
    /// While a dropdown is open it owns the pointer: a click on an entry
    /// picks it, a click anywhere else closes the menu. Either way the event
    /// is consumed (see `route`).
    pub fn click(&mut self, m: &MouseEvent) -> Option<T> {
        self.pointer = Some((m.column, m.row));
        if !matches!(m.kind, MouseEventKind::Down(_)) {
            return None;
        }
        if let Some(menu) = self.menu.take() {
            let r = menu.rect;
            let inside = m.column > r.x
                && m.column + 1 < r.x + r.width
                && m.row > r.y
                && m.row + 1 < r.y + r.height;
            if inside {
                let i = (m.row - r.y - 1) as usize;
                return menu.items.get(i).and_then(|(_, id)| *id);
            }
            return None;
        }
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return None;
        }
        self.at(m.column, m.row)
    }

    /// Convenience for the common `on_mouse` prologue: returns
    /// `Some(Some(id))` for a clicked control, `Some(None)` for an event the
    /// controls swallowed, `None` when the app should handle it itself.
    pub fn route(&mut self, m: &MouseEvent) -> Option<Option<T>> {
        let was_open = self.menu_open();
        let on_control = matches!(m.kind, MouseEventKind::Down(_))
            && self.at(m.column, m.row).is_some();
        let picked = self.click(m);
        if picked.is_some() {
            return Some(picked);
        }
        if matches!(m.kind, MouseEventKind::Down(_)) && (was_open || on_control) {
            return Some(None);
        }
        None
    }

    /// Draw one button at (x, y) and register it. Returns its width.
    pub fn button(&mut self, buf: &mut Buffer, x: u16, y: u16, label: &str, id: T, look: Look) -> u16 {
        let text = format!(" {label} ");
        let w = text.width() as u16;
        let r = Rect::new(x, y, w, 1);
        buf.set_string(x, y, &text, self.style(r, look, BAR_BG));
        if look != Look::Disabled {
            self.add(r, id);
        }
        w
    }

    fn style(&self, r: Rect, look: Look, base: Color) -> Style {
        let hot = look != Look::Disabled && self.hot(r);
        let (fg, bg) = match look {
            Look::Disabled => (DIM, base),
            Look::On => (Color::White, ON_BG),
            Look::Primary => (Color::White, PRIMARY_BG),
            Look::Danger => (Color::White, DANGER_BG),
            Look::Raised => (FG, BTN_BG),
            Look::Plain => (FG, base),
        };
        let mut s = Style::default().fg(if hot { Color::White } else { fg });
        s = s.bg(if hot { HOVER_BG } else { bg });
        if matches!(look, Look::On | Look::Primary | Look::Danger) || hot {
            s = s.add_modifier(Modifier::BOLD);
        }
        s
    }

    /// A full-width toolbar: `left` packed from the left edge, `right` from
    /// the right edge. Items that do not fit are dropped from the end of each
    /// group (right first), so order groups by importance. Returns the free
    /// span between the two groups.
    pub fn toolbar(&mut self, buf: &mut Buffer, row: Rect, left: &[Item<T>], right: &[Item<T>]) -> Rect {
        fill_row(buf, row, BAR_BG);
        let row = Rect::new(row.x, row.y, row.width, 1);
        let end = row.x + row.width;
        let mut x = row.x;
        for it in left {
            if x + it.width() > end {
                break;
            }
            x += self.item(buf, x, row.y, it);
        }
        let room = end.saturating_sub(x);
        let mut used = 0u16;
        let mut n = 0;
        for it in right {
            if used + it.width() + 1 > room {
                break;
            }
            used += it.width();
            n += 1;
        }
        let mut rx = end - used;
        for it in &right[..n] {
            rx += self.item(buf, rx, row.y, it);
        }
        Rect::new(x, row.y, (end - used).saturating_sub(x), 1)
    }

    fn item(&mut self, buf: &mut Buffer, x: u16, y: u16, it: &Item<T>) -> u16 {
        match it {
            Item::Button(l, id, look) => self.button(buf, x, y, l, *id, *look),
            Item::Sep => {
                buf.set_string(x, y, "│", Style::default().fg(DIM).bg(BAR_BG));
                1
            }
            Item::Label(l, c) => {
                buf.set_string(x, y, format!("{l} "), Style::default().fg(*c).bg(BAR_BG));
                l.width() as u16 + 1
            }
        }
    }

    /// A large button filling `r`, its label centered. Rows outside the
    /// label are padding, so a 3-row button reads as a solid key.
    pub fn big_button(&mut self, buf: &mut Buffer, r: Rect, label: &str, id: T, look: Look) {
        if r.width == 0 || r.height == 0 {
            return;
        }
        let look = if look == Look::Plain { Look::Raised } else { look };
        let style = self.style(r, look, BTN_BG);
        fill_row_style(buf, r, style);
        let lw = label.width() as u16;
        let lx = r.x + r.width.saturating_sub(lw) / 2;
        let text: String = if lw > r.width {
            label.chars().take(r.width as usize).collect()
        } else {
            label.to_string()
        };
        buf.set_string(lx.max(r.x), r.y + r.height / 2, text, style);
        if look != Look::Disabled {
            self.add(r, id);
        }
    }

    /// A row of equal big buttons spread across `r`, with one-cell gaps.
    pub fn button_row(&mut self, buf: &mut Buffer, r: Rect, items: &[(&str, T, Look)]) {
        if items.is_empty() || r.width == 0 {
            return;
        }
        let n = items.len() as u16;
        let gap = 1u16;
        let w = (r.width.saturating_sub(gap * (n - 1))) / n;
        if w == 0 {
            return;
        }
        for (i, (label, id, look)) in items.iter().enumerate() {
            let x = r.x + i as u16 * (w + gap);
            self.big_button(buf, Rect::new(x, r.y, w, r.height), label, *id, *look);
        }
    }

    /// Open a dropdown under (x, y). `None` ids are section headings that
    /// cannot be picked; an empty label draws a divider.
    pub fn open_menu(&mut self, x: u16, y: u16, items: Vec<(String, Option<T>)>) {
        let w = items.iter().map(|(l, _)| l.width() as u16).max().unwrap_or(8) + 4;
        let h = items.len() as u16 + 2;
        self.menu = Some(OpenMenu {
            rect: Rect::new(x, y, w.max(12), h),
            items,
        });
    }

    /// Open a dropdown right under the control `id`, if it was drawn.
    pub fn open_menu_under(&mut self, id: T, items: Vec<(String, Option<T>)>)
    where
        T: PartialEq,
    {
        let at = self
            .hits
            .iter()
            .find(|(_, h)| *h == id)
            .map(|(r, _)| (r.x, r.y + r.height));
        if let Some((x, y)) = at {
            self.open_menu(x, y, items);
        }
    }

    /// Draw the open dropdown, kept inside `area`. Call last in `render`.
    pub fn render_menu(&mut self, buf: &mut Buffer, area: Rect) {
        let pointer = self.pointer;
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let mut r = menu.rect;
        r.width = r.width.min(area.width);
        r.height = r.height.min(area.height);
        if r.x + r.width > area.x + area.width {
            r.x = (area.x + area.width).saturating_sub(r.width);
        }
        if r.y + r.height > area.y + area.height {
            r.y = (area.y + area.height).saturating_sub(r.height);
        }
        r.x = r.x.max(area.x);
        r.y = r.y.max(area.y);
        menu.rect = r;
        let bg = Style::default().fg(MENU_BORDER).bg(MENU_BG);
        fill_row_style(buf, r, bg);
        let inner_w = r.width.saturating_sub(2) as usize;
        let bottom = r.y + r.height - 1;
        let right = r.x + r.width - 1;
        buf.set_string(r.x, r.y, format!("╭{}╮", "─".repeat(inner_w)), bg);
        buf.set_string(r.x, bottom, format!("╰{}╯", "─".repeat(inner_w)), bg);
        for y in r.y + 1..bottom {
            buf.set_string(r.x, y, "│", bg);
            buf.set_string(right, y, "│", bg);
        }
        for (i, (label, id)) in menu.items.iter().enumerate() {
            let y = r.y + 1 + i as u16;
            if y >= bottom {
                break;
            }
            let row = Rect::new(r.x + 1, y, inner_w as u16, 1);
            if label.is_empty() {
                buf.set_string(row.x, y, "─".repeat(inner_w), Style::default().fg(DIM).bg(MENU_BG));
                continue;
            }
            let hot = id.is_some()
                && pointer.is_some_and(|(px, py)| py == y && px >= row.x && px < row.x + row.width);
            let style = match (id.is_some(), hot) {
                (_, true) => Style::default()
                    .fg(Color::White)
                    .bg(ON_BG)
                    .add_modifier(Modifier::BOLD),
                (true, false) => Style::default().fg(FG).bg(MENU_BG),
                (false, _) => Style::default().fg(DIM).bg(MENU_BG),
            };
            fill_row_style(buf, row, style);
            let text: String = format!(" {label}").chars().take(inner_w).collect();
            buf.set_string(row.x, y, text, style);
        }
    }

    /// A text field on one row with OK / Cancel buttons at the right end.
    pub fn input_row(
        &mut self,
        buf: &mut Buffer,
        row: Rect,
        label: &str,
        input: &str,
        ok: (&str, T),
        cancel: T,
    ) {
        let bg = Color::Rgb(0, 70, 110);
        fill_row(buf, Rect::new(row.x, row.y, row.width, 1), bg);
        let cw = format!(" {} ", "Cancel").width() as u16;
        let ow = format!(" {} ", ok.0).width() as u16;
        let end = row.x + row.width;
        let cx = end.saturating_sub(cw);
        let ox = cx.saturating_sub(ow + 1);
        let head = format!(" {label}: ");
        buf.set_string(row.x, row.y, &head, Style::default().fg(Color::Rgb(170, 200, 230)).bg(bg));
        let fx = row.x + head.width() as u16;
        let field_w = ox.saturating_sub(fx + 1) as usize;
        let text = format!("{input}▏");
        // keep the caret visible: the label stays, the text shows its tail
        let shown: String = if text.width() > field_w {
            let skip = text.chars().count().saturating_sub(field_w.saturating_sub(1));
            format!("…{}", text.chars().skip(skip).collect::<String>())
        } else {
            text
        };
        buf.set_string(fx, row.y, shown, Style::default().fg(Color::White).bg(bg));
        if ox > row.x + 8 {
            self.button(buf, ox, row.y, ok.0, ok.1, Look::Primary);
            self.button(buf, cx, row.y, "Cancel", cancel, Look::Raised);
        }
    }
}

/// Paint `r` with a background, clearing text.
pub fn fill_row(buf: &mut Buffer, r: Rect, bg: Color) {
    fill_row_style(buf, r, Style::default().bg(bg));
}

fn fill_row_style(buf: &mut Buffer, r: Rect, style: Style) {
    let r = r.intersection(buf.area);
    for y in r.y..r.y + r.height {
        for x in r.x..r.x + r.width {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.reset();
                c.set_symbol(" ");
                c.set_style(style);
            }
        }
    }
}

/// A status line: plain information, never a list of keys.
pub fn status_bar(buf: &mut Buffer, row: Rect, text: &str) {
    let bg = Color::Rgb(20, 24, 40);
    fill_row(buf, Rect::new(row.x, row.y, row.width, 1), bg);
    let t: String = format!(" {text}").chars().take(row.width as usize).collect();
    buf.set_string(row.x, row.y, t, Style::default().fg(Color::Rgb(150, 158, 185)).bg(bg));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn down(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn toolbar_buttons_are_clickable_and_disabled_ones_are_not() {
        let mut c: Controls<u8> = Controls::new();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        c.begin();
        c.toolbar(
            &mut buf,
            Rect::new(0, 0, 40, 1),
            &[Item::btn("Open", 1), Item::with("Save", 2, Look::Disabled)],
            &[Item::btn("Help", 3)],
        );
        assert_eq!(c.click(&down(1, 0)), Some(1));
        assert_eq!(c.click(&down(8, 0)), None);
        assert_eq!(c.click(&down(38, 0)), Some(3));
    }

    #[test]
    fn a_menu_owns_the_next_click() {
        let mut c: Controls<u8> = Controls::new();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
        c.begin();
        c.button(&mut buf, 0, 0, "File", 1, Look::Plain);
        c.open_menu_under(1, vec![("New".into(), Some(10)), ("Open".into(), Some(11))]);
        let area = buf.area;
        c.render_menu(&mut buf, area);
        // second entry: border row is 1, entries start at 2
        assert_eq!(c.route(&down(3, 3)), Some(Some(11)));
        assert!(!c.menu_open());
        c.open_menu_under(1, vec![("New".into(), Some(10))]);
        let area = buf.area;
        c.render_menu(&mut buf, area);
        assert_eq!(c.route(&down(30, 8)), Some(None), "click outside just closes");
        assert!(!c.menu_open());
    }
}
