//! Contribute / Customize: fetch texttop's own source code into an
//! app-internal folder with git and open it in the IDE app group.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::group::IDE;
use super::ui::{self, Controls, Look};
use super::{gitutil, state_home, Action, AppWindow, Launch};

pub const REPO: &str = "https://github.com/VinzSpring/texttop.git";
const REPO_PAGE: &str = "https://github.com/VinzSpring/texttop";

const BG: Color = Color::Rgb(12, 14, 22);
const ACCENT: Color = Color::Rgb(0, 190, 255);
const OK: Color = Color::Rgb(110, 230, 140);
const BAD: Color = Color::Rgb(255, 120, 110);

/// Where the checkout lives: inside texttop's own data, not the user's files.
pub fn source_dir() -> PathBuf {
    state_home().join(".texttop_source").join("texttop")
}

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Get,
    Open,
    Update,
    Files,
    Close,
}

enum Msg {
    Line(String),
    Done(Result<(), String>),
}

#[derive(Clone, Copy, PartialEq)]
enum Job {
    Clone,
    Pull,
}

pub struct Contribute {
    log: Vec<String>,
    job: Option<(Job, Receiver<Msg>)>,
    /// Branch and last commit of the checkout, when there is one.
    info: Option<String>,
    status: (String, Color),
    ticks: usize,
    pending: Option<Action>,
    ui: Controls<Btn>,
}

impl Contribute {
    pub fn new() -> Self {
        let mut c = Contribute {
            log: Vec::new(),
            job: None,
            info: None,
            status: (String::new(), ui::FG),
            ticks: 0,
            pending: None,
            ui: Controls::new(),
        };
        c.refresh();
        c
    }

    fn cloned() -> bool {
        source_dir().join(".git").exists()
    }

    fn refresh(&mut self) {
        let dir = source_dir();
        self.info = Self::cloned().then(|| {
            let run = |args: &[&str]| {
                gitutil::run_git(&dir, args)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default()
            };
            let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"]);
            let last = run(&["log", "-1", "--format=%h %s (%cr)"]);
            format!("branch {branch} · {last}")
        });
    }

    fn ide_launch() -> Launch {
        let dir = source_dir();
        let readme = dir.join("README.md");
        Launch::Group {
            id: IDE.into(),
            cwd: Some(dir),
            open: if readme.exists() { vec![readme] } else { Vec::new() },
        }
    }

    fn start(&mut self, job: Job) {
        if self.job.is_some() {
            return;
        }
        let dir = source_dir();
        let (args, cwd): (Vec<String>, Option<PathBuf>) = match job {
            Job::Clone => {
                if let Some(parent) = dir.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        self.status = (format!("✗ cannot create {}: {e}", parent.display()), BAD);
                        return;
                    }
                }
                (
                    vec![
                        "clone".into(),
                        "--progress".into(),
                        REPO.into(),
                        dir.to_string_lossy().to_string(),
                    ],
                    None,
                )
            }
            Job::Pull => (
                vec!["pull".into(), "--ff-only".into(), "--progress".into()],
                Some(dir),
            ),
        };
        self.log.clear();
        self.log.push(format!("$ git {}", args.join(" ")));
        self.status = (
            match job {
                Job::Clone => "Downloading the source code…".into(),
                Job::Pull => "Fetching the latest changes…".into(),
            },
            ACCENT,
        );
        self.job = Some((job, run_git(args, cwd)));
    }

    fn push_line(&mut self, line: String) {
        let line = line.trim_end().to_string();
        if line.is_empty() {
            return;
        }
        // git redraws progress ("Receiving objects:  42% …") with \r: keep one
        // line per phase instead of hundreds.
        let phase = |s: &str| s.split_once(':').map(|(p, _)| p.to_string());
        if let (Some(last), Some(p)) = (self.log.last(), phase(&line)) {
            if phase(last).as_deref() == Some(p.as_str()) && line.contains('%') {
                *self.log.last_mut().unwrap() = line;
                return;
            }
        }
        self.log.push(line);
        if self.log.len() > 200 {
            self.log.remove(1);
        }
    }

    fn press(&mut self, b: Btn) -> Action {
        match b {
            Btn::Get => self.start(Job::Clone),
            Btn::Update => self.start(Job::Pull),
            Btn::Open => return Action::LaunchAndClose(Self::ide_launch()),
            Btn::Files => return Action::Launch(Launch::Files(source_dir())),
            Btn::Close => return Action::Close,
        }
        Action::None
    }
}

impl Default for Contribute {
    fn default() -> Self {
        Self::new()
    }
}

/// Run git in the background, streaming its output lines.
fn run_git(args: Vec<String>, cwd: Option<PathBuf>) -> Receiver<Msg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut cmd = Command::new("git");
        // git asks for credentials on /dev/tty, which would draw over the
        // desktop and wait forever: fail instead (credential helpers such as
        // Git Credential Manager still work).
        cmd.args(&args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(d) = &cwd {
            cmd.current_dir(d);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::NotFound {
                    "git is not installed — get it from https://git-scm.com and try again".into()
                } else {
                    format!("could not start git: {e}")
                };
                let _ = tx.send(Msg::Done(Err(msg)));
                return;
            }
        };
        let readers: Vec<_> = [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .map(|r| {
            let tx = tx.clone();
            thread::spawn(move || stream_lines(r, tx))
        })
        .collect();
        for r in readers {
            let _ = r.join();
        }
        let result = match child.wait() {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(format!("git exited with {s}")),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(Msg::Done(result));
    });
    rx
}

/// Split output on both \n and \r (progress updates) into lines.
fn stream_lines(mut r: Box<dyn Read + Send>, tx: Sender<Msg>) {
    let mut buf = [0u8; 4096];
    let mut cur = Vec::new();
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                if !cur.is_empty() {
                    let _ = tx.send(Msg::Line(String::from_utf8_lossy(&cur).to_string()));
                    cur.clear();
                }
            } else {
                cur.push(b);
            }
        }
    }
    if !cur.is_empty() {
        let _ = tx.send(Msg::Line(String::from_utf8_lossy(&cur).to_string()));
    }
}

impl AppWindow for Contribute {
    fn title(&self) -> String {
        "Contribute / Customize".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        self.ui.begin();
        let buf = f.buffer_mut();
        for y in area.y..area.y + area.height {
            ui::fill_row(buf, Rect::new(area.x, y, area.width, 1), BG);
        }
        let x = area.x + 2;
        let w = area.width.saturating_sub(4) as usize;
        let mut y = area.y + 1;
        let bottom = area.y + area.height;
        let line = |buf: &mut ratatui::buffer::Buffer, y: &mut u16, text: &str, st: Style| {
            if *y < bottom {
                buf.set_string(x, *y, clip(text, w), st.bg(BG));
            }
            *y += 1;
        };
        let plain = Style::default().fg(ui::FG);
        let dim = Style::default().fg(ui::DIM);
        line(
            buf,
            &mut y,
            "🛠  Make texttop your own",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        );
        y += 1;
        for text in [
            "texttop is open source. Download its code to read how it works,",
            "change anything you like, and send improvements back.",
        ] {
            line(buf, &mut y, text, plain);
        }
        y += 1;
        line(buf, &mut y, &format!("Source   {REPO_PAGE}"), dim);
        line(buf, &mut y, &format!("Folder   {}", source_dir().display()), dim);
        let busy = self.job.is_some();
        let (state, color) = match (&self.info, busy) {
            (_, true) => (
                format!(
                    "{} {}",
                    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][self.ticks % 10],
                    self.status.0
                ),
                ACCENT,
            ),
            (Some(info), false) => (format!("● Downloaded · {info}"), OK),
            (None, false) => ("○ Not downloaded yet".into(), ui::FG),
        };
        line(buf, &mut y, &format!("Status   {state}"), Style::default().fg(color));
        y += 1;

        // Big buttons.
        if y + 3 <= bottom {
            let row = Rect::new(x, y, area.width.saturating_sub(4), 3);
            let dis = |l: Look| if busy { Look::Disabled } else { l };
            if self.info.is_some() {
                self.ui.button_row(
                    buf,
                    row,
                    &[
                        ("🛠 Open in IDE", Btn::Open, dis(Look::Primary)),
                        ("⟳ Get latest", Btn::Update, dis(Look::Raised)),
                        ("🗂 Show files", Btn::Files, Look::Raised),
                    ],
                );
            } else {
                self.ui.button_row(
                    buf,
                    row,
                    &[
                        ("⬇ Get the source code", Btn::Get, dis(Look::Primary)),
                        ("Close", Btn::Close, Look::Raised),
                    ],
                );
            }
            y += 4;
        }
        let mut lines: Vec<(String, Style)> = Vec::new();
        if !busy && self.status.0.is_empty() {
            lines.push((
                "The IDE opens with the code on the left and a terminal below.".into(),
                dim,
            ));
            lines.push(("Build and run your copy there with:  cargo run --release".into(), dim));
        } else if !busy {
            lines.push((self.status.0.clone(), Style::default().fg(self.status.1)));
        }
        lines.push((String::new(), dim));
        // git output, newest at the bottom.
        let room = (bottom.saturating_sub(y) as usize).saturating_sub(lines.len());
        let start = self.log.len().saturating_sub(room);
        lines.extend(self.log[start..].iter().map(|l| (l.clone(), dim)));
        for (text, st) in lines {
            line(buf, &mut y, &text, st);
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc => Action::Close,
            KeyCode::Enter if self.job.is_none() => {
                if Self::cloned() {
                    self.press(Btn::Open)
                } else {
                    self.press(Btn::Get)
                }
            }
            _ => Action::None,
        }
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        match self.ui.route(&m) {
            Some(Some(b)) => self.press(b),
            _ => Action::None,
        }
    }

    fn on_tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        let mut done = None;
        if let Some((job, rx)) = &self.job {
            let job = *job;
            let msgs: Vec<Msg> = rx.try_iter().collect();
            for m in msgs {
                match m {
                    Msg::Line(l) => self.push_line(l),
                    Msg::Done(r) => done = Some((job, r)),
                }
            }
        }
        let Some((job, result)) = done else {
            return;
        };
        self.job = None;
        self.refresh();
        match result {
            Ok(()) => {
                self.status = (
                    match job {
                        Job::Clone => "✓ Source code downloaded — opening the IDE".into(),
                        Job::Pull => "✓ Up to date".into(),
                    },
                    OK,
                );
                if job == Job::Clone {
                    self.pending = Some(Action::LaunchAndClose(Self::ide_launch()));
                }
            }
            Err(e) => {
                // A failed clone leaves a half-written folder behind.
                if job == Job::Clone && !Self::cloned() {
                    let _ = std::fs::remove_dir_all(source_dir());
                }
                let needs_login = self.log.iter().any(|l| {
                    let l = l.to_ascii_lowercase();
                    l.contains("could not read username")
                        || l.contains("authentication failed")
                        || l.contains("terminal prompts disabled")
                        || l.contains("repository not found")
                });
                let msg = if needs_login {
                    "✗ GitHub wants a login for this repository — sign in to git                      (e.g. Git Credential Manager) and try again"
                        .to_string()
                } else {
                    format!("✗ {e}")
                };
                self.status = (msg, BAD);
            }
        }
    }

    fn take_action(&mut self) -> Action {
        self.pending.take().unwrap_or(Action::None)
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
