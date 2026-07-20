//! Shared vertical overflow scrolling for apps whose content can exceed the window.

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Track and apply vertical scroll for a list of lines.
#[derive(Clone, Copy, Default)]
pub struct ScrollState {
    pub offset: u16,
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

    /// Handle mouse wheel. Returns true if consumed.
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
            _ => false,
        }
    }
}

/// Render `lines` into `area` with vertical scroll + a thin scrollbar when needed.
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
    if overflow {
        draw_scrollbar(
            f,
            Rect::new(area.x + text_w, area.y, 1, view_h),
            lines.len(),
            view_h,
            scroll.offset,
        );
    }
}

fn draw_scrollbar(f: &mut Frame, area: Rect, content_len: usize, view_h: u16, offset: u16) {
    if area.height == 0 {
        return;
    }
    let buf = f.buffer_mut();
    let track = Style::default().fg(Color::Rgb(40, 48, 70));
    let thumb = Style::default().fg(Color::Rgb(0, 170, 220));
    for y in 0..area.height {
        if let Some(c) = buf.cell_mut((area.x, area.y + y)) {
            c.set_symbol("│");
            c.set_style(track);
        }
    }
    let max = ScrollState::max(content_len, view_h);
    let thumb_h = ((view_h as usize * view_h as usize) / content_len.max(1))
        .clamp(1, view_h as usize) as u16;
    let travel = view_h.saturating_sub(thumb_h);
    let thumb_y = if max == 0 {
        0
    } else {
        (offset as u32 * travel as u32 / max as u32) as u16
    };
    for dy in 0..thumb_h {
        if let Some(c) = buf.cell_mut((area.x, area.y + thumb_y + dy)) {
            c.set_symbol("█");
            c.set_style(thumb);
        }
    }
}
