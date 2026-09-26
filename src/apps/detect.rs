//! Detect common CLI tools on PATH (and a few install dirs) and let the
//! user pin the ones they want to the desktop.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::pin::{PinTo, PinnedApp};
use super::{Action, AppWindow};

const BG: Color = Color::Rgb(12, 14, 22);
const FG: Color = Color::Rgb(215, 218, 230);
const DIM: Color = Color::Rgb(140, 150, 175);
const ACCENT: Color = Color::Rgb(0, 190, 255);
const SEL: Color = Color::Rgb(255, 220, 90);
const ON: Color = Color::Rgb(110, 230, 140);

/// Static catalog entry: search names tried in order on PATH.
struct Catalog {
    id: &'static str,
    name: &'static str,
    emoji: &'static str,
    /// Binary names to look for (first hit wins), e.g. `["nvim", "neovim"]`.
    bins: &'static [&'static str],
    /// Optional default args template for the pin.
    args: &'static str,
    group: &'static str,
}

const CATALOG: &[Catalog] = &[
    // AI / assistants
    Catalog {
        id: "claude",
        name: "Claude Code",
        emoji: "🤖",
        bins: &["claude"],
        args: "",
        group: "AI",
    },
    Catalog {
        id: "gemini",
        name: "Gemini CLI",
        emoji: "✨",
        bins: &["gemini"],
        args: "",
        group: "AI",
    },
    Catalog {
        id: "codex",
        name: "Codex CLI",
        emoji: "🧠",
        bins: &["codex"],
        args: "",
        group: "AI",
    },
    Catalog {
        id: "aider",
        name: "Aider",
        emoji: "🤝",
        bins: &["aider"],
        args: "",
        group: "AI",
    },
    Catalog {
        id: "ollama",
        name: "Ollama",
        emoji: "🦙",
        bins: &["ollama"],
        args: "",
        group: "AI",
    },
    // editors
    Catalog {
        id: "vim",
        name: "Vim",
        emoji: "📗",
        bins: &["vim", "vim.exe"],
        args: "",
        group: "Editors",
    },
    Catalog {
        id: "nvim",
        name: "Neovim",
        emoji: "🌲",
        bins: &["nvim", "nvim.exe"],
        args: "",
        group: "Editors",
    },
    Catalog {
        id: "hx",
        name: "Helix",
        emoji: "🧬",
        bins: &["hx", "helix"],
        args: "",
        group: "Editors",
    },
    Catalog {
        id: "nano",
        name: "Nano",
        emoji: "📝",
        bins: &["nano"],
        args: "",
        group: "Editors",
    },
    Catalog {
        id: "emacs",
        name: "Emacs",
        emoji: "🐃",
        bins: &["emacs"],
        args: "",
        group: "Editors",
    },
    Catalog {
        id: "code",
        name: "VS Code CLI",
        emoji: "💙",
        bins: &["code", "code.cmd"],
        args: "{path=.}",
        group: "Editors",
    },
    Catalog {
        id: "cursor",
        name: "Cursor CLI",
        emoji: "🖱️",
        bins: &["cursor", "cursor.cmd"],
        args: "{path=.}",
        group: "Editors",
    },
    // languages / tooling
    Catalog {
        id: "python",
        name: "Python",
        emoji: "🐍",
        bins: &["python", "python3", "py"],
        args: "",
        group: "Languages",
    },
    Catalog {
        id: "pip",
        name: "pip",
        emoji: "📦",
        bins: &["pip", "pip3"],
        args: "",
        group: "Languages",
    },
    Catalog {
        id: "node",
        name: "Node.js",
        emoji: "🟢",
        bins: &["node"],
        args: "",
        group: "Languages",
    },
    Catalog {
        id: "npm",
        name: "npm",
        emoji: "📦",
        bins: &["npm", "npm.cmd"],
        args: "",
        group: "Languages",
    },
    Catalog {
        id: "pnpm",
        name: "pnpm",
        emoji: "📦",
        bins: &["pnpm", "pnpm.cmd"],
        args: "",
        group: "Languages",
    },
    Catalog {
        id: "yarn",
        name: "Yarn",
        emoji: "🧶",
        bins: &["yarn", "yarn.cmd"],
        args: "",
        group: "Languages",
    },
    Catalog {
        id: "cargo",
        name: "Cargo",
        emoji: "📦",
        bins: &["cargo"],
        args: "",
        group: "Languages",
    },
    Catalog {
        id: "rustc",
        name: "rustc",
        emoji: "🦀",
        bins: &["rustc"],
        args: "",
        group: "Languages",
    },
    Catalog {
        id: "go",
        name: "Go",
        emoji: "🐹",
        bins: &["go"],
        args: "",
        group: "Languages",
    },
    Catalog {
        id: "ruby",
        name: "Ruby",
        emoji: "💎",
        bins: &["ruby"],
        args: "",
        group: "Languages",
    },
    Catalog {
        id: "php",
        name: "PHP",
        emoji: "🐘",
        bins: &["php"],
        args: "",
        group: "Languages",
    },
    // devops / git
    Catalog {
        id: "git",
        name: "Git",
        emoji: "🌿",
        bins: &["git"],
        args: "",
        group: "DevOps",
    },
    Catalog {
        id: "gh",
        name: "GitHub CLI",
        emoji: "🐙",
        bins: &["gh"],
        args: "",
        group: "DevOps",
    },
    Catalog {
        id: "lazygit",
        name: "Lazygit",
        emoji: "🧭",
        bins: &["lazygit"],
        args: "",
        group: "DevOps",
    },
    Catalog {
        id: "docker",
        name: "Docker",
        emoji: "🐳",
        bins: &["docker"],
        args: "",
        group: "DevOps",
    },
    Catalog {
        id: "kubectl",
        name: "kubectl",
        emoji: "☸️",
        bins: &["kubectl"],
        args: "",
        group: "DevOps",
    },
    Catalog {
        id: "podman",
        name: "Podman",
        emoji: "🦭",
        bins: &["podman"],
        args: "",
        group: "DevOps",
    },
    // unix utilities
    Catalog {
        id: "rg",
        name: "ripgrep",
        emoji: "🔎",
        bins: &["rg"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "fd",
        name: "fd",
        emoji: "📁",
        bins: &["fd", "fdfind"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "bat",
        name: "bat",
        emoji: "🦇",
        bins: &["bat", "batcat"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "fzf",
        name: "fzf",
        emoji: "🔍",
        bins: &["fzf"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "jq",
        name: "jq",
        emoji: "🧩",
        bins: &["jq"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "tmux",
        name: "tmux",
        emoji: "🖥️",
        bins: &["tmux"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "htop",
        name: "htop",
        emoji: "📊",
        bins: &["htop"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "btop",
        name: "btop",
        emoji: "📈",
        bins: &["btop"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "curl",
        name: "curl",
        emoji: "🌐",
        bins: &["curl"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "wget",
        name: "wget",
        emoji: "⬇️",
        bins: &["wget"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "ssh",
        name: "ssh",
        emoji: "🔑",
        bins: &["ssh"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "make",
        name: "make",
        emoji: "🔧",
        bins: &["make", "mingw32-make"],
        args: "",
        group: "Utils",
    },
    Catalog {
        id: "cmake",
        name: "CMake",
        emoji: "🧱",
        bins: &["cmake"],
        args: "",
        group: "Utils",
    },
];

#[derive(Clone, Debug)]
pub struct DetectedApp {
    pub name: String,
    pub emoji: String,
    pub program: PathBuf,
    pub args: String,
    pub group: &'static str,
}

impl DetectedApp {
    pub fn to_pin(&self) -> PinnedApp {
        PinnedApp {
            name: self.name.clone(),
            emoji: self.emoji.clone(),
            program: self.program.clone(),
            args: self.args.clone(),
            cwd: None,
        }
    }
}

/// Resolve `name` on PATH (and PATHEXT on Windows). Returns an absolute path.
pub fn which(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    // Absolute / relative that already exists.
    let direct = PathBuf::from(name);
    if direct.is_file() {
        return Some(canonicalize_soft(&direct));
    }

    let path_dirs = path_dirs();
    let exts = pathext();

    for dir in &path_dirs {
        if let Some(p) = try_in_dir(dir, name, &exts) {
            return Some(p);
        }
    }
    for dir in extra_dirs() {
        if let Some(p) = try_in_dir(&dir, name, &exts) {
            return Some(p);
        }
    }
    None
}

fn canonicalize_soft(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn path_dirs() -> Vec<PathBuf> {
    let Some(raw) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    std::env::split_paths(&raw).filter(|p| !p.as_os_str().is_empty()).collect()
}

fn pathext() -> Vec<String> {
    #[cfg(windows)]
    {
        let raw = std::env::var("PATHEXT").unwrap_or_else(|_| {
            ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC;.CPL".into()
        });
        let mut exts: Vec<String> = raw
            .split(';')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        // Prefer common launcher forms first.
        for preferred in [".exe", ".cmd", ".bat", ".ps1", ".com"] {
            if let Some(i) = exts.iter().position(|e| e == preferred) {
                let e = exts.remove(i);
                exts.insert(0, e);
            } else {
                exts.insert(0, preferred.to_string());
            }
        }
        // Dedup while keeping order.
        let mut seen = HashSet::new();
        exts.retain(|e| seen.insert(e.clone()));
        exts
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn try_in_dir(dir: &Path, name: &str, #[allow(unused_variables)] exts: &[String]) -> Option<PathBuf> {
    // Exact name as given (may already include extension).
    let candidate = dir.join(name);
    if is_runnable(&candidate) {
        return Some(canonicalize_soft(&candidate));
    }
    #[cfg(windows)]
    {
        let lower = name.to_ascii_lowercase();
        let has_ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| !e.is_empty());
        if !has_ext {
            for ext in exts {
                let c = dir.join(format!("{lower}{ext}"));
                if is_runnable(&c) {
                    return Some(canonicalize_soft(&c));
                }
                // Also try original casing.
                let c2 = dir.join(format!("{name}{ext}"));
                if is_runnable(&c2) {
                    return Some(canonicalize_soft(&c2));
                }
            }
        }
    }
    None
}

fn is_runnable(p: &Path) -> bool {
    if !p.is_file() {
        return false;
    }
    crate::apps::pin::is_executable(p)
}

fn extra_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<PathBuf>, p: PathBuf| {
        if p.is_dir() {
            out.push(p);
        }
    };

    if let Some(home) = dirs::home_dir() {
        push(&mut out, home.join(".cargo").join("bin"));
        push(&mut out, home.join("go").join("bin"));
        push(&mut out, home.join(".local").join("bin"));
        push(&mut out, home.join(".npm-global").join("bin"));
        push(&mut out, home.join("AppData").join("Roaming").join("npm"));
        push(
            &mut out,
            home.join("AppData")
                .join("Local")
                .join("Programs")
                .join("Microsoft VS Code")
                .join("bin"),
        );
        push(
            &mut out,
            home.join("AppData")
                .join("Local")
                .join("Programs")
                .join("cursor")
                .join("resources")
                .join("app")
                .join("bin"),
        );
        push(
            &mut out,
            home.join("AppData")
                .join("Local")
                .join("Microsoft")
                .join("WinGet")
                .join("Links"),
        );
    }

    #[cfg(windows)]
    {
        push(&mut out, PathBuf::from(r"C:\Program Files\Git\cmd"));
        push(&mut out, PathBuf::from(r"C:\Program Files\Git\usr\bin"));
        push(&mut out, PathBuf::from(r"C:\Program Files\Git\bin"));
        push(
            &mut out,
            PathBuf::from(r"C:\Program Files\Microsoft VS Code\bin"),
        );
        push(
            &mut out,
            PathBuf::from(r"C:\Program Files\nodejs"),
        );
        push(
            &mut out,
            PathBuf::from(r"C:\Program Files\Docker\Docker\resources\bin"),
        );
    }

    #[cfg(target_os = "macos")]
    {
        push(&mut out, PathBuf::from("/opt/homebrew/bin"));
        push(&mut out, PathBuf::from("/usr/local/bin"));
        push(
            &mut out,
            PathBuf::from(
                "/Applications/Visual Studio Code.app/Contents/Resources/app/bin",
            ),
        );
        push(
            &mut out,
            PathBuf::from("/Applications/Cursor.app/Contents/Resources/app/bin"),
        );
    }

    #[cfg(target_os = "linux")]
    {
        push(&mut out, PathBuf::from("/usr/local/bin"));
        push(&mut out, PathBuf::from("/snap/bin"));
    }

    out
}

/// Scan the catalog; skip tools already present in `already` (by bin stem).
pub fn scan_known_apps(already: &HashSet<String>) -> Vec<DetectedApp> {
    let mut found = Vec::new();
    let mut seen_ids = HashSet::new();
    for entry in CATALOG {
        if seen_ids.contains(entry.id) {
            continue;
        }
        if already.contains(entry.id) || already.contains(&entry.name.to_ascii_lowercase()) {
            continue;
        }
        let Some(program) = entry.bins.iter().find_map(|b| which(b)) else {
            continue;
        };
        // Also skip if this exact program path is already pinned.
        let stem = program
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if already.contains(&stem) {
            continue;
        }
        seen_ids.insert(entry.id);
        found.push(DetectedApp {
            name: entry.name.to_string(),
            emoji: entry.emoji.to_string(),
            program,
            args: entry.args.to_string(),
            group: entry.group,
        });
    }
    found
}

// ── chooser UI ───────────────────────────────────────────────────────────────

pub struct DetectedAppsChooser {
    apps: Vec<DetectedApp>,
    /// Selected for pinning.
    checked: Vec<bool>,
    sel: usize,
    scroll: usize,
    view_h: usize,
}

impl DetectedAppsChooser {
    pub fn new(already_pinned: HashSet<String>) -> Self {
        let apps = scan_known_apps(&already_pinned);
        let n = apps.len();
        DetectedAppsChooser {
            checked: vec![true; n], // default: pin everything found
            apps,
            sel: 0,
            scroll: 0,
            view_h: 12,
        }
    }

    fn move_sel(&mut self, delta: isize) {
        if self.apps.is_empty() {
            return;
        }
        let n = self.apps.len() as isize;
        self.sel = (self.sel as isize + delta).rem_euclid(n) as usize;
        self.ensure_visible();
    }

    fn ensure_visible(&mut self) {
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + self.view_h {
            self.scroll = self.sel + 1 - self.view_h;
        }
    }

    fn toggle(&mut self) {
        if let Some(c) = self.checked.get_mut(self.sel) {
            *c = !*c;
        }
    }

    fn toggle_all(&mut self) {
        let all_on = self.checked.iter().all(|&c| c);
        for c in &mut self.checked {
            *c = !all_on;
        }
    }

    fn checked_count(&self) -> usize {
        self.checked.iter().filter(|&&c| c).count()
    }

    fn apply(&self) -> Action {
        let pins: Vec<PinnedApp> = self
            .apps
            .iter()
            .zip(self.checked.iter())
            .filter(|(_, c)| **c)
            .map(|(a, _)| a.to_pin())
            .collect();
        if pins.is_empty() {
            Action::Close
        } else {
            Action::AddDetectedPins {
                pins,
                to: PinTo::Desktop,
            }
        }
    }
}

impl AppWindow for DetectedAppsChooser {
    fn title(&self) -> String {
        format!(
            "Detected apps · {} found · {} selected",
            self.apps.len(),
            self.checked_count()
        )
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        {
            let buf = f.buffer_mut();
            for y in area.y..area.y + area.height {
                for x in area.x..area.x + area.width {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_symbol(" ");
                        cell.set_fg(BG);
                        cell.set_bg(BG);
                    }
                }
            }
        }

        let hint = Line::from(vec![
            Span::styled(" ↑↓ ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("move  ", Style::default().fg(DIM)),
            Span::styled("Space ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("toggle  ", Style::default().fg(DIM)),
            Span::styled("a ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("all  ", Style::default().fg(DIM)),
            Span::styled("Enter ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("add to desktop", Style::default().fg(DIM)),
        ]);
        f.render_widget(Paragraph::new(hint), Rect::new(area.x, area.y, area.width, 1));

        if self.apps.is_empty() {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "  no known CLI tools found on PATH",
                    Style::default().fg(DIM),
                )),
                Rect::new(area.x, area.y + 2, area.width, 1),
            );
            return;
        }

        let list_top = area.y + 2;
        let list_h = area.height.saturating_sub(3).max(1) as usize;
        self.view_h = list_h;
        self.ensure_visible();

        let mut last_group = "";
        let mut row = 0u16;
        for (i, app) in self.apps.iter().enumerate() {
            if i < self.scroll {
                continue;
            }
            if row as usize >= list_h {
                break;
            }
            let y = list_top + row;
            if app.group != last_group {
                if row as usize + 1 >= list_h && i > self.scroll {
                    break;
                }
                f.render_widget(
                    Paragraph::new(Span::styled(
                        format!("  {}", app.group),
                        Style::default()
                            .fg(ACCENT)
                            .bg(BG)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Rect::new(area.x, y, area.width, 1),
                );
                last_group = app.group;
                row += 1;
                if row as usize >= list_h {
                    break;
                }
            }
            let y = list_top + row;
            let on = self.checked.get(i).copied().unwrap_or(false);
            let selected = i == self.sel;
            let mark = if on { "[x]" } else { "[ ]" };
            let path = app.program.to_string_lossy();
            let path_short = if path.chars().count() > 36 {
                format!(
                    "…{}",
                    path.chars()
                        .skip(path.chars().count().saturating_sub(35))
                        .collect::<String>()
                )
            } else {
                path.to_string()
            };
            let line = format!(
                "  {mark} {} {:<14} {}",
                app.emoji, app.name, path_short
            );
            let style = if selected {
                Style::default()
                    .fg(SEL)
                    .bg(Color::Rgb(28, 34, 52))
                    .add_modifier(Modifier::BOLD)
            } else if on {
                Style::default().fg(ON).bg(BG)
            } else {
                Style::default().fg(FG).bg(BG)
            };
            // pad to full width for selection bar
            let pad = area.width.saturating_sub(line.width() as u16) as usize;
            f.render_widget(
                Paragraph::new(Span::styled(format!("{line}{:pad$}", "", pad = pad), style)),
                Rect::new(area.x, y, area.width, 1),
            );
            row += 1;
        }

        let foot = Line::from(vec![
            Span::styled("  ", Style::default().bg(BG)),
            Span::styled(
                format!("{} selected · Esc cancel", self.checked_count()),
                Style::default().fg(DIM).bg(BG),
            ),
        ]);
        f.render_widget(
            Paragraph::new(foot),
            Rect::new(area.x, area.y + area.height.saturating_sub(1), area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => Action::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_sel(-1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_sel(1);
                Action::None
            }
            KeyCode::PageUp => {
                self.move_sel(-(self.view_h as isize));
                Action::None
            }
            KeyCode::PageDown => {
                self.move_sel(self.view_h as isize);
                Action::None
            }
            KeyCode::Home => {
                self.sel = 0;
                self.ensure_visible();
                Action::None
            }
            KeyCode::End => {
                self.sel = self.apps.len().saturating_sub(1);
                self.ensure_visible();
                Action::None
            }
            KeyCode::Char(' ') => {
                self.toggle();
                Action::None
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.toggle_all();
                Action::None
            }
            KeyCode::Enter => self.apply(),
            _ => Action::None,
        }
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        match m.kind {
            MouseEventKind::ScrollUp => {
                self.move_sel(-1);
                Action::None
            }
            MouseEventKind::ScrollDown => {
                self.move_sel(1);
                Action::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let list_top = area.y + 2;
                if m.row < list_top || m.row >= area.y + area.height.saturating_sub(1) {
                    return Action::None;
                }
                // Approximate hit: walk visible rows like render.
                let mut last_group = "";
                let mut row = 0u16;
                let target = m.row.saturating_sub(list_top);
                for (i, app) in self.apps.iter().enumerate().skip(self.scroll) {
                    if app.group != last_group {
                        if row == target {
                            return Action::None; // clicked group header
                        }
                        last_group = app.group;
                        row += 1;
                    }
                    if row == target {
                        self.sel = i;
                        self.toggle();
                        return Action::None;
                    }
                    row += 1;
                    if row as usize >= self.view_h {
                        break;
                    }
                }
                Action::None
            }
            _ => Action::None,
        }
    }
}
