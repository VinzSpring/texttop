//! In-desktop terminal emulator: a real shell (or any console program)
//! running on a Windows ConPTY, parsed with vt100 and rendered in a window.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};

pub struct Terminal {
    title: String,
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Option<Box<dyn Write + Send>>,
    master: Option<Box<dyn MasterPty + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    size: (u16, u16),
    exited: bool,
    error: Option<String>,
}

impl Terminal {
    /// `program` None → the user's shell (PowerShell).
    pub fn new(program: Option<PathBuf>, args: Vec<String>) -> Self {
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
            error: None,
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
        if let Some(home) = dirs::home_dir() {
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
        // cursor
        if focused && !self.exited && !screen.hide_cursor() {
            let (crow, ccol) = screen.cursor_position();
            if crow < area.height && ccol < area.width {
                if let Some(c) = buf.cell_mut((area.x + ccol, area.y + crow)) {
                    c.modifier |= Modifier::REVERSED;
                }
            }
        }
        if self.exited {
            let msg = " process exited — Ctrl+W closes ";
            let x = area.x + area.width.saturating_sub(msg.len() as u16) / 2;
            let y = area.y + area.height.saturating_sub(1);
            buf.set_string(
                x,
                y,
                msg,
                Style::default()
                    .fg(Color::Rgb(255, 200, 90))
                    .bg(Color::Rgb(40, 40, 20)),
            );
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
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

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        match m.kind {
            MouseEventKind::ScrollUp => self.send(b"\x1b[A"),
            MouseEventKind::ScrollDown => self.send(b"\x1b[B"),
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
}
