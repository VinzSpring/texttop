//! Shared vertical overflow scrolling for apps whose content can exceed the window.

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Track and apply vertical scroll for a list of lines.
#[derive(Clone, Copy, Default)]
pub struct ScrollState {
    pub offset: u16,
    /// Where the scrollbar was last drawn, so it can be clicked and dragged.
    bar: Option<Rect>,
    /// Dragging the thumb: keeps tracking even when the pointer leaves the bar.
    dragging: bool,
}

impl ScrollState {
    pub fn max(content_len: usize, view_h: u16) -> u16 {
        content_len
            .saturating_sub(view_h.max(1) as usize)
            .min(u16::MAX as usize) as u16
    }

    pub fn clamp(&mut self, content_len: usize, view_h: u16) {
        self.offset = self.offset.min(Self::max(content_len, view_h));
    }

    pub fn by(&mut self, delta: i32, content_len: usize, view_h: u16) {
        let max = Self::max(content_len, view_h) as i32;
        let next = (self.offset as i32 + delta).clamp(0, max);
        self.offset = next as u16;
    }

    /// Handle common scroll keys. Returns true if the key was consumed.
    pub fn on_key(&mut self, k: KeyEvent, content_len: usize, view_h: u16) -> bool {
        match k.code {
            KeyCode::Up => self.by(-1, content_len, view_h),
            KeyCode::Down => self.by(1, content_len, view_h),
            KeyCode::PageUp => self.by(-(view_h as i32).max(1), content_len, view_h),
            KeyCode::PageDown => self.by((view_h as i32).max(1), content_len, view_h),
            KeyCode::Home => self.offset = 0,
            KeyCode::End => self.offset = Self::max(content_len, view_h),
            _ => return false,
        }
        true
    }

    /// Handle the mouse wheel and the scrollbar: its arrows step, a click on
    /// the track jumps there, and the thumb drags. Returns true if consumed.
    pub fn on_mouse(&mut self, m: MouseEvent, content_len: usize, view_h: u16) -> bool {
        match m.kind {
            MouseEventKind::ScrollUp => {
                self.by(-3, content_len, view_h);
                true
            }
            MouseEventKind::ScrollDown => {
                self.by(3, content_len, view_h);
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(bar) = self.bar.filter(|b| {
                    m.column == b.x && m.row >= b.y && m.row < b.y + b.height
                }) else {
                    return false;
                };
                let arrows = bar.height >= 4;
                if arrows && m.row == bar.y {
                    self.by(-1, content_len, view_h);
                } else if arrows && m.row == bar.y + bar.height - 1 {
                    self.by(1, content_len, view_h);
                } else {
                    self.dragging = true;
                    self.jump(m.row, content_len, view_h);
                }
                true
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.jump(m.row, content_len, view_h);
                true
            }
            MouseEventKind::Up(_) if self.dragging => {
                self.dragging = false;
                true
            }
            _ => false,
        }
    }

    /// Scroll so the thumb centers on `row` of the track.
    fn jump(&mut self, row: u16, content_len: usize, view_h: u16) {
        let Some(bar) = self.bar else { return };
        let (top, h) = track(bar);
        let max = Self::max(content_len, view_h) as u32;
        let rel = row.saturating_sub(top).min(h.saturating_sub(1)) as u32;
        self.offset = if h <= 1 {
            0
        } else {
            ((rel * max + (h as u32 - 1) / 2) / (h as u32 - 1)) as u16
        };
    }
}

/// The part of the bar between the arrows: (first row, height).
fn track(bar: Rect) -> (u16, u16) {
    if bar.height >= 4 {
        (bar.y + 1, bar.height - 2)
    } else {
        (bar.y, bar.height)
    }
}

/// Render `lines` into `area` with vertical scroll + a scrollbar when needed.
pub fn render_scrollable(f: &mut Frame, area: Rect, lines: &[Line], scroll: &mut ScrollState) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let view_h = area.height;
    scroll.clamp(lines.len(), view_h);
    let overflow = lines.len() > view_h as usize;
    let text_w = if overflow {
        area.width.saturating_sub(1).max(1)
    } else {
        area.width
    };
    let text_area = Rect::new(area.x, area.y, text_w, view_h);
    f.render_widget(
        Paragraph::new(lines.to_vec()).scroll((scroll.offset, 0)),
        text_area,
    );
    scroll.bar = None;
    if overflow {
        let bar = Rect::new(area.x + text_w, area.y, 1, view_h);
        scroll.bar = Some(bar);
        draw_scrollbar(f, bar, lines.len(), view_h, scroll.offset);
    }
}

fn draw_scrollbar(f: &mut Frame, area: Rect, content_len: usize, view_h: u16, offset: u16) {
    if area.height == 0 {
        return;
    }
    let buf = f.buffer_mut();
    let track_s = Style::default().fg(Color::Rgb(40, 48, 70));
    let thumb = Style::default().fg(Color::Rgb(0, 170, 220));
    let arrow = Style::default()
        .fg(Color::Rgb(200, 210, 235))
        .bg(Color::Rgb(44, 54, 88));
    let (top, h) = track(area);
    if h != area.height {
        if let Some(c) = buf.cell_mut((area.x, area.y)) {
            c.set_symbol("▴");
            c.set_style(arrow);
        }
        if let Some(c) = buf.cell_mut((area.x, area.y + area.height - 1)) {
            c.set_symbol("▾");
            c.set_style(arrow);
        }
    }
    for y in 0..h {
        if let Some(c) = buf.cell_mut((area.x, top + y)) {
            c.set_symbol("│");
            c.set_style(track_s);
        }
    }
    let max = ScrollState::max(content_len, view_h);
    let thumb_h = ((h as usize * view_h as usize) / content_len.max(1)).clamp(1, h as usize) as u16;
    let travel = h.saturating_sub(thumb_h);
    let thumb_y = if max == 0 {
        0
    } else {
        (offset as u32 * travel as u32 / max as u32) as u16
    };
    for dy in 0..thumb_h {
        if let Some(c) = buf.cell_mut((area.x, top + thumb_y + dy)) {
            c.set_symbol("█");
            c.set_style(thumb);
        }
    }
}
