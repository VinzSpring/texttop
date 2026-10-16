//! In-desktop terminal emulator: a real shell (or any console program)
//! running on a Windows ConPTY, parsed with vt100 and rendered in a window.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::ui::{Controls, Look};
use super::{Action, AppWindow, Launch};

/// Terminal context menu and exit bar.
#[derive(Clone, Copy, PartialEq)]
enum TCmd {
    Copy,
    Paste,
    Close,
}

pub struct Terminal {
    title: String,
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn MasterPty + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    size: (u16, u16),
    exited: bool,
    ui: Controls<TCmd>,
    error: Option<String>,
    /// Mouse selection over the visible grid, as (anchor, cursor) in
    /// (row, col). Either end may come first — [`Terminal::sel_span`] orders
    /// them.
    sel: Option<((u16, u16), (u16, u16))>,
    /// When the last copy happened, so the confirmation can fade out. Copying
    /// silently replaces the system clipboard, which is worth showing.
    copied: Option<(Instant, usize)>,
}

impl Terminal {
    /// `program` None → the user's shell (PowerShell).
    pub fn new(program: Option<PathBuf>, args: Vec<String>, cwd: Option<PathBuf>) -> Self {
        let (rows, cols) = (24u16, 80u16);
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let default_shell = if cfg!(windows) {
            "powershell.exe".to_string()
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
        };
        let title = program
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                std::path::Path::new(&default_shell)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "shell".into())
            });

        let mut term = Terminal {
            title,
            parser: parser.clone(),
            writer: None,
            master: None,
            child: None,
            size: (cols, rows),
            exited: false,
            ui: Controls::new(),
            error: None,
            sel: None,
            copied: None,
        };

        let pty = native_pty_system();
        let pair = match pty.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(p) => p,
            Err(e) => {
                term.error = Some(format!("cannot open pty: {e}"));
                return term;
            }
        };

        let mut cmd = match &program {
            None => {
                let mut c = CommandBuilder::new(&default_shell);
                if cfg!(windows) {
                    c.args(["-NoLogo"]);
                }
                c
            }
            Some(p) => {
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let mut c = if cfg!(windows) && matches!(ext.as_str(), "bat" | "cmd") {
                    let mut c = CommandBuilder::new("cmd.exe");
                    c.arg("/c");
                    c.arg(p);
                    c
                } else if cfg!(windows) && ext == "ps1" {
                    let mut c = CommandBuilder::new("powershell.exe");
                    c.args(["-ExecutionPolicy", "Bypass", "-File"]);
                    c.arg(p);
                    c
                } else if !cfg!(windows) && ext == "sh" {
                    let mut c = CommandBuilder::new("sh");
                    c.arg(p);
                    c
                } else {
                    CommandBuilder::new(p)
                };
                for a in &args {
                    c.arg(a);
                }
                c
            }
        };
        if let Some(dir) = cwd.filter(|p| p.is_dir()) {
            cmd.cwd(dir);
        } else if let Some(home) = dirs::home_dir() {
            if program.is_none() {
                cmd.cwd(home);
            } else if let Some(dir) = program.as_ref().and_then(|p| p.parent()) {
                cmd.cwd(dir);
            }
        }

        let child = match pair.slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => {
                term.error = Some(format!("cannot start process: {e}"));
                return term;
            }
        };
        drop(pair.slave);

        match pair.master.try_clone_reader() {
            Ok(mut reader) => {
                let parser = parser.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 8192];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => parser.lock().unwrap().process(&buf[..n]),
                        }
                    }
                });
            }
            Err(e) => {
                term.error = Some(format!("cannot read pty: {e}"));
                return term;
            }
        }
        term.writer = pair.master.take_writer().ok();
        term.master = Some(pair.master);
        term.child = Some(child);
        term
    }

    /// How long the "copied" confirmation stays up.
    const COPIED_FOR: Duration = Duration::from_millis(1600);

    fn sel_span(&self) -> Option<((u16, u16), (u16, u16))> {
        span(self.sel?)
    }

    fn selected_text(&self) -> Option<String> {
        let span = self.sel_span()?;
        let parser = self.parser.lock().ok()?;
        Some(grab(parser.screen(), span))
    }

    fn copy_selection(&mut self) {
        match self.selected_text().filter(|t| !t.is_empty()) {
            Some(text) => {
                let n = text.chars().count();
                crate::clipboard::copy(&text);
                self.copied = Some((Instant::now(), n));
            }
            None => self.copied = None,
        }
    }

    /// Send clipboard text to the child, bracketed so a shell with paste
    /// protection treats a multi-line paste as one unit instead of running
    /// each line as it arrives.
    fn paste_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.send(b"\x1b[200~");
        self.send(text.as_bytes());
        self.send(b"\x1b[201~");
    }

    /// Grid cell under the pointer, clamped to the window.
    fn cell_at(m: MouseEvent, area: Rect) -> (u16, u16) {
        (
            m.row.saturating_sub(area.y).min(area.height.saturating_sub(1)),
            m.column.saturating_sub(area.x).min(area.width.saturating_sub(1)),
        )
    }

    fn send(&mut self, bytes: &[u8]) {
        if let Some(w) = &mut self.writer {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    fn vt_color(c: vt100::Color, fg: bool) -> Color {
        match c {
            vt100::Color::Default => {
                if fg {
                    Color::Rgb(220, 222, 232)
                } else {
                    Color::Rgb(8, 10, 18)
                }
            }
            vt100::Color::Idx(i) => Color::Indexed(i),
            vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
        }
    }
}

/// Order a selection top-left to bottom-right, with the end column exclusive.
/// None for a bare click, which has no extent.
fn span((a, b): ((u16, u16), (u16, u16))) -> Option<((u16, u16), (u16, u16))> {
    if a == b {
        return None;
    }
    let (start, (er, ec)) = if a <= b { (a, b) } else { (b, a) };
    // Both ends are cells the pointer touched, so the far one is inside the
    // selection — without this the last character never comes along.
    Some((start, (er, ec + 1)))
}

/// Read selected cells off the vt100 grid. Rows are trimmed on the right, the
/// way every terminal copies: the cells past the end of a line are blanks the
/// emulator painted, not spaces the user typed.
fn grab(screen: &vt100::Screen, ((sr, sc), (er, ec)): ((u16, u16), (u16, u16))) -> String {
    let (rows, cols) = screen.size();
    let mut out: Vec<String> = Vec::new();
    for row in sr..=er.min(rows.saturating_sub(1)) {
        let from = if row == sr { sc } else { 0 };
        let to = if row == er { ec } else { cols };
        let mut line = String::new();
        for col in from..to.min(cols) {
            if let Some(c) = screen.cell(row, col) {
                line.push_str(&c.contents());
            }
        }
        out.push(line.trim_end().to_string());
    }
    out.join("\n")
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
        }
    }
}

impl AppWindow for Terminal {
    fn title(&self) -> String {
        if self.exited {
            format!("{} · exited", self.title)
        } else {
            self.title.clone()
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        if let Some(err) = &self.error {
            f.render_widget(
                Paragraph::new(vec![
                    Line::default(),
                    Line::from(Span::styled(
                        format!("  ✗ {err}"),
                        Style::default().fg(Color::Rgb(255, 110, 110)),
                    )),
                ]),
                area,
            );
            return;
        }

        // keep pty size in sync with the window
        if (area.width, area.height) != self.size && area.width > 4 && area.height > 2 {
            self.size = (area.width, area.height);
            if let Some(m) = &self.master {
                let _ = m.resize(PtySize {
                    rows: area.height,
                    cols: area.width,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
            self.parser
                .lock()
                .unwrap()
                .set_size(area.height, area.width);
        }

        let sel = self.sel_span();
        let parser = self.parser.lock().unwrap();
        let screen = parser.screen();
        let buf = f.buffer_mut();
        for row in 0..area.height.min(screen.size().0) {
            for col in 0..area.width.min(screen.size().1) {
                let (x, y) = (area.x + col, area.y + row);
                let Some(target) = buf.cell_mut((x, y)) else {
                    continue;
                };
                match screen.cell(row, col) {
                    Some(c) => {
                        let contents = c.contents();
                        if contents.is_empty() {
                            target.set_symbol(" ");
                        } else {
                            target.set_symbol(&contents);
                        }
                        let mut fg = Self::vt_color(c.fgcolor(), true);
                        let mut bg = Self::vt_color(c.bgcolor(), false);
                        if c.inverse() {
                            std::mem::swap(&mut fg, &mut bg);
                        }
                        target.set_fg(fg);
                        target.set_bg(bg);
                        let mut m = Modifier::empty();
                        if c.bold() {
                            m |= Modifier::BOLD;
                        }
                        if c.italic() {
                            m |= Modifier::ITALIC;
                        }
                        if c.underline() {
                            m |= Modifier::UNDERLINED;
                        }
                        target.modifier = m;
                    }
                    None => {
                        target.set_symbol(" ");
                        target.set_bg(Color::Rgb(8, 10, 18));
                    }
                }
            }
        }
        // selection, over the grid the shell drew
        if let Some(((sr, sc), (er, ec))) = sel {
            let bg = Color::Rgb(46, 70, 116);
            for row in sr..=er.min(area.height.saturating_sub(1)) {
                let from = if row == sr { sc } else { 0 };
                let to = if row == er { ec } else { area.width };
                for col in from..to.min(area.width) {
                    if let Some(c) = buf.cell_mut((area.x + col, area.y + row)) {
                        c.set_bg(bg);
                    }
                }
            }
        }

        // cursor
        if focused && !self.exited && !screen.hide_cursor() {
            let (crow, ccol) = screen.cursor_position();
            if crow < area.height && ccol < area.width {
                if let Some(c) = buf.cell_mut((area.x + ccol, area.y + crow)) {
                    c.modifier |= Modifier::REVERSED;
                }
            }
        }
        if let Some((at, n)) = self.copied {
            if at.elapsed() < Self::COPIED_FOR {
                let msg = format!(" ✓ copied {n} chars ");
                let x = area.x + area.width.saturating_sub(msg.len() as u16) / 2;
                buf.set_string(
                    x,
                    area.y,
                    &msg,
                    Style::default()
                        .fg(Color::Rgb(140, 230, 170))
                        .bg(Color::Rgb(20, 44, 30)),
                );
            }
        }
        self.ui.begin();
        if self.exited {
            let msg = " process exited ";
            let w = msg.len() as u16 + 9;
            let x = area.x + area.width.saturating_sub(w) / 2;
            let y = area.y + area.height.saturating_sub(1);
            buf.set_string(
                x,
                y,
                msg,
                Style::default()
                    .fg(Color::Rgb(255, 200, 90))
                    .bg(Color::Rgb(40, 40, 20)),
            );
            self.ui
                .button(buf, x + msg.len() as u16, y, "Close", TCmd::Close, Look::Primary);
        }
        self.ui.render_menu(buf, area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let sup = k.modifiers.contains(KeyModifiers::SUPER);

        // Copy and paste have to be shifted here: plain Ctrl+C is SIGINT and
        // Ctrl+V is the shell's literal-next, and a terminal that cannot send
        // those is not a terminal.
        match k.code {
            KeyCode::Char(c) if (ctrl && shift || sup) && c.eq_ignore_ascii_case(&'c') => {
                self.copy_selection();
                return Action::None;
            }
            KeyCode::Char(c) if (ctrl && shift || sup) && c.eq_ignore_ascii_case(&'v') => {
                let text = crate::clipboard::paste();
                self.paste_text(&text);
                self.sel = None;
                return Action::None;
            }
            _ => {}
        }
        // Anything else is input for the child, and scrolls the selection out
        // from under the user.
        self.sel = None;

        let bytes: Vec<u8> = match k.code {
            KeyCode::Char(c) if ctrl => {
                let u = (c.to_ascii_uppercase() as u8).wrapping_sub(b'@');
                if u < 32 {
                    vec![u]
                } else {
                    return Action::None;
                }
            }
            KeyCode::Char(c) => c.to_string().into_bytes(),
            KeyCode::Enter => b"\r".to_vec(),
            KeyCode::Backspace => vec![0x7f],
            KeyCode::Tab => b"\t".to_vec(),
            KeyCode::Esc => vec![0x1b],
            KeyCode::Up => b"\x1b[A".to_vec(),
            KeyCode::Down => b"\x1b[B".to_vec(),
            KeyCode::Right => b"\x1b[C".to_vec(),
            KeyCode::Left => b"\x1b[D".to_vec(),
            KeyCode::Home => b"\x1b[H".to_vec(),
            KeyCode::End => b"\x1b[F".to_vec(),
            KeyCode::PageUp => b"\x1b[5~".to_vec(),
            KeyCode::PageDown => b"\x1b[6~".to_vec(),
            KeyCode::Delete => b"\x1b[3~".to_vec(),
            KeyCode::Insert => b"\x1b[2~".to_vec(),
            _ => return Action::None,
        };
        self.send(&bytes);
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        // Forward the bracketing too, so a shell with paste protection on still
        // treats a multi-line paste as one unit rather than running each line.
        self.send(b"\x1b[200~");
        self.send(text.as_bytes());
        self.send(b"\x1b[201~");
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        if let Some(hit) = self.ui.route(&m) {
            match hit {
                Some(TCmd::Copy) => self.copy_selection(),
                Some(TCmd::Paste) => {
                    let text = crate::clipboard::paste();
                    if !text.is_empty() {
                        return self.on_paste(&text);
                    }
                }
                Some(TCmd::Close) => return Action::Close,
                None => {}
            }
            return Action::None;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                let has_sel = self.sel.is_some_and(|(a, b)| a != b);
                self.ui.open_menu(
                    m.column,
                    m.row,
                    vec![
                        ("Copy".into(), has_sel.then_some(TCmd::Copy)),
                        ("Paste".into(), Some(TCmd::Paste)),
                    ],
                );
            }
            MouseEventKind::ScrollUp => {
                self.sel = None;
                self.send(b"\x1b[A");
            }
            MouseEventKind::ScrollDown => {
                self.sel = None;
                self.send(b"\x1b[B");
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let at = Self::cell_at(m, area);
                self.sel = Some((at, at));
                self.copied = None;
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((_, cur)) = &mut self.sel {
                    *cur = Self::cell_at(m, area);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Copy on release, the way iTerm2 does — needing a chord after
                // every drag is what makes terminal copy tedious. A bare click
                // selects nothing, so it leaves the clipboard alone.
                self.copy_selection();
            }
            _ => {}
        }
        Action::None
    }

    fn on_tick(&mut self) {
        if !self.exited {
            if let Some(child) = &mut self.child {
                if let Ok(Some(_)) = child.try_wait() {
                    self.exited = true;
                }
            }
        }
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    fn on_drop(&mut self, paths: &[std::path::PathBuf], _cut: bool) -> Action {
        let Some(p) = paths.first() else {
            return Action::None;
        };
        if p.is_dir() {
            return Action::Launch(Launch::Terminal {
                program: None,
                args: Vec::new(),
                cwd: Some(p.clone()),
            });
        }
        // type the path into the shell
        let s = format!("{} ", p.display());
        self.send(s.as_bytes());
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::{grab, span};

    fn screen(lines: &[&str]) -> vt100::Parser {
        let mut p = vt100::Parser::new(6, 20, 0);
        p.process(lines.join("\r\n").as_bytes());
        p
    }

    #[test]
    fn grabs_a_run_within_one_row() {
        let p = screen(&["hello world"]);
        // columns 6..=10 — "world"
        let s = span(((0, 6), (0, 10))).unwrap();
        assert_eq!(grab(p.screen(), s), "world");
    }

    #[test]
    fn the_cell_under_the_pointer_is_included() {
        let p = screen(&["abcdef"]);
        let s = span(((0, 0), (0, 2))).unwrap();
        assert_eq!(grab(p.screen(), s), "abc");
    }

    #[test]
    fn dragging_backwards_selects_the_same_run() {
        let p = screen(&["hello world"]);
        let forward = grab(p.screen(), span(((0, 6), (0, 10))).unwrap());
        let backward = grab(p.screen(), span(((0, 10), (0, 6))).unwrap());
        assert_eq!(forward, backward);
    }

    #[test]
    fn multi_row_runs_to_the_end_of_each_line_and_trims_the_blanks() {
        let p = screen(&["first line", "second", "third"]);
        let s = span(((0, 6), (2, 4))).unwrap();
        // tail of row 0, all of row 1, head of row 2 — with the blank cells
        // past "second" dropped rather than copied as spaces.
        assert_eq!(grab(p.screen(), s), "line\nsecond\nthird");
    }

    #[test]
    fn a_bare_click_selects_nothing() {
        assert!(span(((2, 5), (2, 5))).is_none());
    }

    #[test]
    fn a_selection_past_the_end_of_a_line_yields_an_empty_row() {
        let p = screen(&["ab", "cd"]);
        let s = span(((0, 5), (0, 9))).unwrap();
        assert_eq!(grab(p.screen(), s), "");
    }
}
