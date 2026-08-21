use std::fs;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::syntax::{self, HlState, Lang};
use super::{Action, AppWindow};

pub struct Editor {
    path: Option<PathBuf>,
    lines: Vec<String>,
    cx: usize,
    cy: usize,
    scroll: usize,
    modified: bool,
    status: String,
    lang: Lang,
    highlight: bool,
    /// Where a selection was started, as (line, column). The other end is the
    /// cursor, so moving the cursor grows or shrinks the selection.
    anchor: Option<(usize, usize)>,
}

fn bidx(s: &str, cx: usize) -> usize {
    s.char_indices().nth(cx).map(|(i, _)| i).unwrap_or(s.len())
}

impl Editor {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut status =
            String::from("Ctrl+S save · Ctrl+C/X/V copy/cut/paste · Shift+arrows select");
        let lines = match &path {
            Some(p) => match fs::read(p) {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    if bytes.contains(&0) {
                        status = "⚠ binary file — editing not recommended".into();
                    }
                    text.replace("\r\n", "\n").split('\n').map(|s| s.to_string()).collect()
                }
                Err(e) => {
                    status = format!("✗ {e}");
                    vec![String::new()]
                }
            },
            None => vec![String::new()],
        };
        Editor::with_lines(path, lines, status)
    }

    /// Open an in-memory buffer bound to `path`; nothing is read from disk.
    /// Used for files that do not exist yet (see [`super::notes::Notes`]).
    pub fn with_lines(path: Option<PathBuf>, mut lines: Vec<String>, status: String) -> Self {
        // Cursor movement indexes `lines` directly, so it must never be empty.
        if lines.is_empty() {
            lines.push(String::new());
        }
        let lang = Lang::from_path(path.as_deref());
        Editor {
            path,
            lines,
            cx: 0,
            cy: 0,
            scroll: 0,
            modified: false,
            status,
            lang,
            highlight: lang != Lang::Plain,
            anchor: None,
        }
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    fn line_len(&self, y: usize) -> usize {
        self.lines.get(y).map(|l| l.chars().count()).unwrap_or(0)
    }

    fn save(&mut self) {
        match &self.path {
            None => self.status = "untitled — open a file from the file manager to save".into(),
            Some(p) => match fs::write(p, self.lines.join("\n")) {
                Ok(_) => {
                    self.modified = false;
                    self.lang = Lang::from_path(Some(p));
                    self.status = format!("saved {} lines ✓ · {}", self.lines.len(), self.lang.label());
                }
                Err(e) => self.status = format!("✗ save failed: {e}"),
            },
        }
    }

    /// Put the caret under the pointer. `gutter` wide line numbers sit left of
    /// the text, and the view is scrolled by `self.scroll` rows.
    fn move_to(&mut self, m: MouseEvent, area: Rect) {
        let row = m.row.saturating_sub(area.y) as usize + self.scroll;
        self.cy = row.min(self.lines.len().saturating_sub(1));
        let col = m.column.saturating_sub(area.x + 5) as usize;
        self.cx = col.min(self.line_len(self.cy));
    }

    /// The selection as (start, end) with start <= end, or None when the
    /// anchor sits exactly on the cursor.
    fn sel_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.anchor?;
        let b = (self.cy, self.cx);
        if a == b {
            return None;
        }
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    fn selected_text(&self) -> Option<String> {
        let ((sy, sx), (ey, ex)) = self.sel_range()?;
        if sy == ey {
            let line = self.lines.get(sy)?;
            return Some(line.chars().skip(sx).take(ex - sx).collect());
        }
        let mut out: String = self.lines.get(sy)?.chars().skip(sx).collect();
        for line in self.lines.get(sy + 1..ey)? {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.extend(self.lines.get(ey)?.chars().take(ex));
        Some(out)
    }

    /// Remove the selected text and leave the cursor where it started.
    /// Returns false when there was no selection.
    fn delete_selection(&mut self) -> bool {
        let Some(((sy, sx), (ey, ex))) = self.sel_range() else {
            return false;
        };
        let head: String = self.lines[sy].chars().take(sx).collect();
        let tail: String = self.lines[ey].chars().skip(ex).collect();
        self.lines.splice(sy..=ey, [format!("{head}{tail}")]);
        self.cy = sy;
        self.cx = sx;
        self.anchor = None;
        self.modified = true;
        true
    }

    /// Copy the selection, or the whole cursor line when nothing is selected —
    /// the same convenience most editors give Ctrl+C.
    fn copy(&mut self) {
        let (text, what) = match self.selected_text() {
            Some(t) => {
                let n = t.lines().count();
                (t, if n > 1 { format!("{n} lines") } else { "selection".into() })
            }
            None => (
                format!("{}\n", self.lines[self.cy]),
                "line".to_string(),
            ),
        };
        crate::clipboard::copy(&text);
        self.status = format!("copied {what}");
    }

    fn cut(&mut self) {
        if self.sel_range().is_some() {
            self.copy();
            self.delete_selection();
            self.status = "cut selection".into();
        } else {
            crate::clipboard::copy(&format!("{}\n", self.lines[self.cy]));
            if self.lines.len() == 1 {
                self.lines[0].clear();
            } else {
                self.lines.remove(self.cy);
                self.cy = self.cy.min(self.lines.len() - 1);
            }
            self.cx = 0;
            self.modified = true;
            self.status = "cut line".into();
        }
    }

    fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.cy = self.lines.len() - 1;
        self.cx = self.line_len(self.cy);
    }

    /// Start, extend or drop the selection around a cursor move.
    /// Call before moving, with whether Shift was held.
    fn before_move(&mut self, extend: bool) {
        if extend {
            self.anchor.get_or_insert((self.cy, self.cx));
        } else {
            self.anchor = None;
        }
    }

    /// Paint the selection over an already-rendered text area.  Doing it on the
    /// buffer rather than in the spans keeps it independent of syntax
    /// highlighting, which has already split the line its own way.
    fn paint_selection(&self, f: &mut Frame, area: Rect, gutter: u16, hscroll: usize, text_h: usize) {
        let Some(((sy, sx), (ey, ex))) = self.sel_range() else {
            return;
        };
        let bg = Color::Rgb(46, 70, 116);
        for y in sy.max(self.scroll)..=ey.min(self.scroll + text_h.saturating_sub(1)) {
            let row = area.y + (y - self.scroll) as u16;
            let from = if y == sy { sx } else { 0 };
            // A selection running past a line's end shows one extra cell, so
            // the newline it swallowed is visible.
            let to = if y == ey { ex } else { self.line_len(y) + 1 };
            for col in from..to {
                let Some(vx) = col.checked_sub(hscroll) else {
                    continue;
                };
                let x = area.x + gutter + vx as u16;
                if x >= area.x + area.width {
                    break;
                }
                if let Some(c) = f.buffer_mut().cell_mut((x, row)) {
                    c.set_bg(bg);
                }
            }
        }
    }

    fn paint_line(&self, line: &str, state: &mut HlState, hscroll: usize, content_w: usize) -> Vec<Span<'static>> {
        if !self.highlight || self.lang == Lang::Plain {
            let visible: String = line.chars().skip(hscroll).take(content_w).collect();
            return vec![Span::styled(
                visible,
                Style::default().fg(Color::Rgb(220, 222, 232)),
            )];
        }
        let spans = syntax::highlight_line(self.lang, line, state);
        syntax::slice_spans(spans, hscroll, content_w)
    }
}

impl AppWindow for Editor {
    fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".into());
        if self.modified {
            format!("{name} •")
        } else {
            name
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        if area.height < 2 {
            return;
        }
        let text_h = area.height as usize - 1;
        if self.cy < self.scroll {
            self.scroll = self.cy;
        }
        if self.cy >= self.scroll + text_h {
            self.scroll = self.cy + 1 - text_h;
        }
        let gutter = 5u16;
        let content_w = area.width.saturating_sub(gutter) as usize;
        let hscroll = self.cx.saturating_sub(content_w.saturating_sub(1));

        let mut state = if self.highlight {
            syntax::state_before(self.lang, &self.lines, self.scroll)
        } else {
            HlState::default()
        };

        let mut out: Vec<Line> = Vec::new();
        for (i, line) in self.lines.iter().enumerate().skip(self.scroll).take(text_h) {
            let num = Span::styled(
                format!("{:>4} ", i + 1),
                Style::default().fg(Color::Rgb(90, 95, 120)),
            );
            let mut body = self.paint_line(line, &mut state, hscroll, content_w);
            let mut spans = vec![num];
            spans.append(&mut body);
            out.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(out), Rect::new(area.x, area.y, area.width, text_h as u16));
        self.paint_selection(f, area, gutter, hscroll, text_h);

        // cursor
        if focused {
            let cur_x = area.x + gutter + (self.cx - hscroll.min(self.cx)) as u16;
            let cur_y = area.y + (self.cy - self.scroll) as u16;
            if cur_x < area.x + area.width && cur_y < area.y + text_h as u16 {
                if let Some(c) = f.buffer_mut().cell_mut((cur_x, cur_y)) {
                    c.modifier |= Modifier::REVERSED;
                }
            }
        }

        let hl = if self.highlight {
            self.lang.label()
        } else {
            "plain"
        };
        let status = format!(
            " {}  ·  Ln {}, Col {}  ·  {}  ·  {} ",
            if self.modified { "modified •" } else { "saved ✓" },
            self.cy + 1,
            self.cx + 1,
            hl,
            self.status
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                status,
                Style::default().fg(Color::Rgb(140, 150, 175)).bg(Color::Rgb(18, 22, 36)),
            ))
            .style(Style::default().bg(Color::Rgb(18, 22, 36))),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        // Terminals that forward the Command/Windows key report it as SUPER;
        // treat it as Ctrl so the platform-native chords work where they
        // survive the terminal emulator.
        let ctrl = k
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        // Shift with a movement key extends the selection instead of dropping it.
        let ext = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('s') if ctrl => self.save(),
            KeyCode::Char('c') if ctrl => self.copy(),
            KeyCode::Char('x') if ctrl => self.cut(),
            KeyCode::Char('v') if ctrl => {
                let text = crate::clipboard::paste();
                if text.is_empty() {
                    self.status = "clipboard empty".into();
                } else {
                    self.on_paste(&text);
                    self.status = "pasted".into();
                }
            }
            KeyCode::Char('a') if ctrl => self.select_all(),
            KeyCode::Char('h') if ctrl => {
                self.highlight = !self.highlight;
                self.status = if self.highlight {
                    format!("highlight on ({})", self.lang.label())
                } else {
                    "highlight off".into()
                };
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.delete_selection();
                let i = bidx(&self.lines[self.cy], self.cx);
                self.lines[self.cy].insert(i, c);
                self.cx += 1;
                self.modified = true;
            }
            KeyCode::Tab => {
                self.delete_selection();
                let i = bidx(&self.lines[self.cy], self.cx);
                self.lines[self.cy].insert_str(i, "    ");
                self.cx += 4;
                self.modified = true;
            }
            KeyCode::Enter => {
                self.delete_selection();
                let i = bidx(&self.lines[self.cy], self.cx);
                let rest = self.lines[self.cy].split_off(i);
                self.lines.insert(self.cy + 1, rest);
                self.cy += 1;
                self.cx = 0;
                self.modified = true;
            }
            KeyCode::Backspace => {
                if self.delete_selection() {
                } else if self.cx > 0 {
                    let i = bidx(&self.lines[self.cy], self.cx - 1);
                    self.lines[self.cy].remove(i);
                    self.cx -= 1;
                    self.modified = true;
                } else if self.cy > 0 {
                    let cur = self.lines.remove(self.cy);
                    self.cy -= 1;
                    self.cx = self.line_len(self.cy);
                    self.lines[self.cy].push_str(&cur);
                    self.modified = true;
                }
            }
            KeyCode::Delete => {
                if self.delete_selection() {
                } else if self.cx < self.line_len(self.cy) {
                    let i = bidx(&self.lines[self.cy], self.cx);
                    self.lines[self.cy].remove(i);
                    self.modified = true;
                } else if self.cy + 1 < self.lines.len() {
                    let next = self.lines.remove(self.cy + 1);
                    self.lines[self.cy].push_str(&next);
                    self.modified = true;
                }
            }
            KeyCode::Left => {
                self.before_move(ext);
                if self.cx > 0 {
                    self.cx -= 1;
                } else if self.cy > 0 {
                    self.cy -= 1;
                    self.cx = self.line_len(self.cy);
                }
            }
            KeyCode::Right => {
                self.before_move(ext);
                if self.cx < self.line_len(self.cy) {
                    self.cx += 1;
                } else if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.cx = 0;
                }
            }
            KeyCode::Up => {
                self.before_move(ext);
                self.cy = self.cy.saturating_sub(1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::Down => {
                self.before_move(ext);
                self.cy = (self.cy + 1).min(self.lines.len() - 1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::PageUp => {
                self.before_move(ext);
                self.cy = self.cy.saturating_sub(20);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::PageDown => {
                self.before_move(ext);
                self.cy = (self.cy + 20).min(self.lines.len() - 1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            KeyCode::Home => {
                self.before_move(ext);
                self.cx = 0;
            }
            KeyCode::End => {
                self.before_move(ext);
                self.cx = self.line_len(self.cy);
            }
            _ => {}
        }
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        if text.is_empty() {
            return Action::None;
        }
        self.delete_selection();
        // Split the current line at the cursor and stitch the paste in between.
        let at = bidx(&self.lines[self.cy], self.cx);
        let tail = self.lines[self.cy].split_off(at);
        let norm = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut parts = norm.split('\n');
        let first = parts.next().unwrap_or("");
        self.lines[self.cy].push_str(first);
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            self.cx += first.chars().count();
        } else {
            for (i, part) in rest.iter().enumerate() {
                self.lines.insert(self.cy + 1 + i, (*part).to_string());
            }
            self.cy += rest.len();
            self.cx = self.lines[self.cy].chars().count();
        }
        self.lines[self.cy].push_str(&tail);
        self.modified = true;
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        match m.kind {
            MouseEventKind::ScrollUp => {
                self.anchor = None;
                self.cy = self.cy.saturating_sub(3);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            MouseEventKind::ScrollDown => {
                self.anchor = None;
                self.cy = (self.cy + 3).min(self.lines.len() - 1);
                self.cx = self.cx.min(self.line_len(self.cy));
            }
            MouseEventKind::Down(_) => {
                self.anchor = None;
                self.move_to(m, area);
                // Anchor here so an immediate drag selects from the click.
                self.anchor = Some((self.cy, self.cx));
            }
            MouseEventKind::Drag(_) => self.move_to(m, area),
            MouseEventKind::Up(_) => {
                // A click that never moved is a caret placement, not an empty
                // selection — clear the anchor so Ctrl+C still copies the line.
                if self.anchor == Some((self.cy, self.cx)) {
                    self.anchor = None;
                }
            }
            _ => {}
        }
        Action::None
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    fn on_drop(&mut self, paths: &[PathBuf], _cut: bool) -> Action {
        let Some(p) = paths.first() else {
            return Action::None;
        };
        if p.is_dir() {
            return Action::Launch(super::Launch::Files(p.clone()));
        }
        if self.modified {
            return Action::Launch(super::Launch::Editor(Some(p.clone())));
        }
        *self = Editor::new(Some(p.clone()));
        Action::None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn ed(lines: &[&str]) -> Editor {
        Editor::with_lines(
            None,
            lines.iter().map(|s| s.to_string()).collect(),
            String::new(),
        )
    }

    /// Select from (ay, ax) to (by, bx).
    fn select(e: &mut Editor, a: (usize, usize), b: (usize, usize)) {
        e.anchor = Some(a);
        e.cy = b.0;
        e.cx = b.1;
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn selection_reads_across_lines() {
        let mut e = ed(&["hello world", "second", "third line"]);
        select(&mut e, (0, 6), (0, 11));
        assert_eq!(e.selected_text().unwrap(), "world");

        select(&mut e, (0, 6), (2, 5));
        assert_eq!(e.selected_text().unwrap(), "world\nsecond\nthird");
    }

    #[test]
    fn a_backwards_selection_reads_the_same() {
        let mut e = ed(&["hello world", "second"]);
        select(&mut e, (1, 3), (0, 6));
        assert_eq!(e.selected_text().unwrap(), "world\nsec");
    }

    #[test]
    fn an_empty_selection_is_no_selection() {
        let mut e = ed(&["hello"]);
        select(&mut e, (0, 2), (0, 2));
        assert!(e.sel_range().is_none());
        assert!(e.selected_text().is_none());
    }

    #[test]
    fn deleting_a_selection_joins_the_ends() {
        let mut e = ed(&["hello world", "second", "third line"]);
        select(&mut e, (0, 6), (2, 6));
        assert!(e.delete_selection());
        assert_eq!(e.lines, vec!["hello line"]);
        assert_eq!((e.cy, e.cx), (0, 6));
        assert!(e.anchor.is_none());
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut e = ed(&["hello world"]);
        select(&mut e, (0, 6), (0, 11));
        e.on_key(key(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(e.lines, vec!["hello t"]);
    }

    #[test]
    fn pasting_over_a_selection_replaces_it() {
        let mut e = ed(&["hello world", "tail"]);
        select(&mut e, (0, 6), (1, 2));
        e.on_paste("there\nfriend");
        assert_eq!(e.lines, vec!["hello there", "friendil"]);
        assert_eq!((e.cy, e.cx), (1, 6));
    }

    #[test]
    fn pasting_a_single_line_stays_on_the_line() {
        let mut e = ed(&["ab"]);
        e.cx = 1;
        e.on_paste("XY");
        assert_eq!(e.lines, vec!["aXYb"]);
        assert_eq!((e.cy, e.cx), (0, 3));
    }

    #[test]
    fn shift_arrows_extend_and_plain_arrows_drop_the_selection() {
        let mut e = ed(&["hello"]);
        e.on_key(key(KeyCode::Right, KeyModifiers::SHIFT));
        e.on_key(key(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(e.selected_text().unwrap(), "he");
        e.on_key(key(KeyCode::Right, KeyModifiers::NONE));
        assert!(e.sel_range().is_none());
    }

    #[test]
    fn select_all_spans_the_buffer() {
        let mut e = ed(&["one", "two", "three"]);
        e.on_key(key(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(e.selected_text().unwrap(), "one\ntwo\nthree");
    }

    #[test]
    fn super_is_accepted_wherever_ctrl_is() {
        let mut e = ed(&["one", "two"]);
        e.on_key(key(KeyCode::Char('a'), KeyModifiers::SUPER));
        assert_eq!(e.selected_text().unwrap(), "one\ntwo");
    }

    #[test]
    fn backspace_with_a_selection_deletes_the_selection() {
        let mut e = ed(&["hello world"]);
        select(&mut e, (0, 5), (0, 11));
        e.on_key(key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(e.lines, vec!["hello"]);
    }
}
