use std::env;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Local;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::scroll::{self, ScrollState};
use super::{Action, AppWindow};
use crate::gfx;

pub struct SysInfo {
    scroll: ScrollState,
    line_count: usize,
    view_h: u16,
    started: std::time::Instant,
}

impl SysInfo {
    pub fn new() -> Self {
        SysInfo {
            scroll: ScrollState::default(),
            line_count: 0,
            view_h: 20,
            started: std::time::Instant::now(),
        }
    }

    fn lines(&self) -> Vec<Line<'static>> {
        let accent = Style::default()
            .fg(Color::Rgb(120, 220, 180))
            .add_modifier(Modifier::BOLD);
        let key = Style::default().fg(Color::Rgb(255, 210, 100));
        let txt = Style::default().fg(Color::Rgb(210, 215, 230));
        let dim = Style::default().fg(Color::Rgb(130, 140, 160));

        let kv = |k: &str, v: String| {
            Line::from(vec![
                Span::styled(format!("   {k:<16}"), key),
                Span::styled(v, txt),
            ])
        };

        let cpus = std::thread::available_parallelism()
            .map(|n| n.get().to_string())
            .unwrap_or_else(|_| "?".into());

        let user = env::var("USERNAME")
            .or_else(|_| env::var("USER"))
            .unwrap_or_else(|_| "?".into());
        let home = dirs::home_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "?".into());
        let cwd = env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into());
        let temp = env::temp_dir().display().to_string();
        let shell = if cfg!(windows) {
            env::var("ComSpec").unwrap_or_else(|_| "powershell".into())
        } else {
            env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
        };

        let mut out = vec![
            Line::default(),
            Line::from(Span::styled("   System", accent)),
            kv("OS", format!("{} ({})", env::consts::OS, env::consts::FAMILY)),
            kv("Arch", env::consts::ARCH.to_string()),
            kv("CPUs", cpus),
            kv("User", user),
            kv("Host", hostname()),
            kv("Time", Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
            kv("Uptime*", format_uptime(self.started.elapsed())),
            Line::default(),
            Line::from(Span::styled("   Paths", accent)),
            kv("Home", home),
            kv("CWD", cwd),
            kv("Temp", temp),
            kv("Shell", shell),
            Line::default(),
            Line::from(Span::styled("   Folders", accent)),
        ];

        for (label, path) in [
            ("Desktop", dirs::desktop_dir()),
            ("Documents", dirs::document_dir()),
            ("Downloads", dirs::download_dir()),
            ("Pictures", dirs::picture_dir()),
            ("Music", dirs::audio_dir()),
            ("Videos", dirs::video_dir()),
        ] {
            let v = path
                .map(|p| {
                    if p.exists() {
                        folder_summary(&p)
                    } else {
                        "(missing)".into()
                    }
                })
                .unwrap_or_else(|| "(n/a)".into());
            out.push(kv(label, v));
        }

        out.push(Line::default());
        out.push(Line::from(Span::styled("   Environment", accent)));
        let interesting = ["PATH", "LANG", "TERM", "COLORTERM", "EDITOR", "CARGO_HOME"];
        for key_name in interesting {
            if let Ok(v) = env::var(key_name) {
                let short = if v.chars().count() > 48 {
                    format!("{}…", v.chars().take(47).collect::<String>())
                } else {
                    v
                };
                out.push(kv(key_name, short));
            }
        }

        out.push(Line::default());
        out.push(Line::from(Span::styled(
            "   * session uptime (since this window opened)",
            dim,
        )));
            out.push(Line::from(Span::styled(
                "   ↑↓ / wheel scroll · F5 refresh · q close",
                dim,
            )));
        out
    }
}

fn hostname() -> String {
    env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| {
            #[cfg(unix)]
            {
                fs_read_hostname().unwrap_or_else(|| "unknown".into())
            }
            #[cfg(not(unix))]
            {
                "unknown".into()
            }
        })
}

#[cfg(unix)]
fn fs_read_hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
}

fn format_uptime(d: Duration) -> String {
    let s = d.as_secs();
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}h {m}m {sec}s")
    } else if m > 0 {
        format!("{m}m {sec}s")
    } else {
        format!("{sec}s")
    }
}

fn folder_summary(p: &PathBuf) -> String {
    match std::fs::read_dir(p) {
        Ok(rd) => {
            let mut files = 0u64;
            let mut dirs = 0u64;
            let mut bytes = 0u64;
            for e in rd.flatten().take(500) {
                if let Ok(md) = e.metadata() {
                    if md.is_dir() {
                        dirs += 1;
                    } else {
                        files += 1;
                        bytes += md.len();
                    }
                }
            }
            format!(
                "{}  ·  {dirs} dirs, {files} files, {}",
                p.display(),
                gfx::fmt_size(bytes)
            )
        }
        Err(_) => p.display().to_string(),
    }
}

impl AppWindow for SysInfo {
    fn title(&self) -> String {
        "System".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        let lines = self.lines();
        self.line_count = lines.len();
        self.view_h = area.height;
        scroll::render_scrollable(f, area, &lines, &mut self.scroll);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        if self.scroll.on_key(k, self.line_count, self.view_h) {
            return Action::None;
        }
        match k.code {
            KeyCode::F(5) => {} // every render pulls fresh data already
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        self.scroll.on_mouse(m, self.line_count, self.view_h);
        Action::None
    }
}
