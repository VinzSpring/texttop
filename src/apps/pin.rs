//! Pinned application links: launch an external program inside the
//! built-in terminal with saved arguments. The argument template may
//! contain `{field}` / `{field=default}` placeholders which are asked
//! from the user each time the pin is started.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::ui::{self, Controls, Look};
use super::{Action, AppWindow, Launch};

/// Buttons of the pin dialogs.
#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Ok,
    Cancel,
    Target(PinTo),
}

/// Where a pin lives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PinTo {
    Desktop,
    Taskbar,
}

/// Identifies an existing pin being edited (so Save replaces it).
#[derive(Clone, Copy, Debug)]
pub enum PinEdit {
    /// Desktop icon index.
    Desktop(usize),
    /// Taskbar pin index.
    Taskbar(usize),
}

#[derive(Clone, Debug)]
pub struct PinnedApp {
    pub name: String,
    pub emoji: String,
    pub program: PathBuf,
    /// Argument template, split on whitespace (double/single quotes group).
    /// `{field}` / `{field=default}` tokens are queried at launch.
    pub args: String,
    pub cwd: Option<PathBuf>,
}

impl PinnedApp {
    pub fn from_path(p: PathBuf) -> Self {
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| p.to_string_lossy().to_string());
        PinnedApp {
            name,
            emoji: "🚀".into(),
            program: p,
            args: String::new(),
            cwd: None,
        }
    }

    /// Placeholder fields (name, default) found in the args template.
    pub fn fields(&self) -> Vec<(String, String)> {
        template_fields(&self.args)
    }

    /// Build the terminal launch with placeholder values filled in.
    pub fn launch_with(&self, values: &[(String, String)]) -> Launch {
        Launch::Terminal {
            program: Some(self.program.clone()),
            args: split_args(&fill_template(&self.args, values)),
            cwd: self.cwd.clone(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "emoji": self.emoji,
            "program": self.program.to_string_lossy(),
            "args": self.args,
            "cwd": self.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
        })
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        let program = PathBuf::from(v.get("program")?.as_str()?);
        Some(PinnedApp {
            name: v
                .get("name")
                .and_then(|s| s.as_str())
                .unwrap_or("app")
                .to_string(),
            emoji: v
                .get("emoji")
                .and_then(|s| s.as_str())
                .unwrap_or("🚀")
                .to_string(),
            program,
            args: v
                .get("args")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            cwd: v.get("cwd").and_then(|s| s.as_str()).map(PathBuf::from),
        })
    }
}

/// Whether a path looks like something we can pin (run in a terminal).
pub fn is_executable(p: &Path) -> bool {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(
        ext.as_str(),
        "exe" | "com" | "bat" | "cmd" | "ps1" | "sh" | "run" | "appimage" | "command"
    ) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if p.metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// Split a command line on whitespace; double/single quotes group words.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut had_quote = false;
    for c in s.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    had_quote = true;
                }
                c if c.is_whitespace() => {
                    if !cur.is_empty() || had_quote {
                        out.push(std::mem::take(&mut cur));
                    }
                    had_quote = false;
                }
                c => cur.push(c),
            },
        }
    }
    if !cur.is_empty() || had_quote {
        out.push(cur);
    }
    out
}

/// Extract `{name}` / `{name=default}` placeholders, in order, deduplicated.
pub fn template_fields(args: &str) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut rest = args;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        let inner = &after[..end];
        let (name, default) = match inner.find('=') {
            Some(i) => (inner[..i].trim(), inner[i + 1..].trim()),
            None => (inner.trim(), ""),
        };
        if !name.is_empty() && !fields.iter().any(|(n, _)| n == name) {
            fields.push((name.to_string(), default.to_string()));
        }
        rest = &after[end + 1..];
    }
    fields
}

/// Replace each placeholder with the collected value for its field name.
pub fn fill_template(args: &str, values: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut rest = args;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push('{');
            rest = after;
            continue;
        };
        let inner = &after[..end];
        let name = inner.split('=').next().unwrap_or("").trim();
        match values.iter().find(|(n, _)| n == name) {
            Some((_, v)) => out.push_str(v),
            // unknown field: keep the default if present
            None => out.push_str(inner.split_once('=').map(|(_, d)| d.trim()).unwrap_or("")),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

const ACCENT: Color = Color::Rgb(0, 190, 255);
const DIM: Color = Color::Rgb(140, 150, 175);
const TXT: Color = Color::Rgb(215, 218, 230);

fn sel_style() -> Style {
    Style::default()
        .fg(Color::White)
        .bg(Color::Rgb(0, 96, 150))
        .add_modifier(Modifier::BOLD)
}

/// An input box that does not have focus.
fn field_style() -> Style {
    Style::default().fg(TXT).bg(Color::Rgb(12, 16, 30))
}

// ------------------------------------------------------------------ editor

/// Settings dialog for a pin: name, icon, program, args, cwd, target.
pub struct PinEditor {
    name: String,
    emoji: String,
    program: String,
    args: String,
    cwd: String,
    to: PinTo,
    edit: Option<PinEdit>,
    sel: usize,
    editing: bool,
    status: String,
    ui: Controls<Btn>,
}

/// Rows: 0 name · 1 icon · 2 program · 3 args · 4 cwd · 5 target · 6 save · 7 cancel
const ED_ROWS: usize = 8;

impl PinEditor {
    pub fn new(pin: PinnedApp, to: PinTo, edit: Option<PinEdit>) -> Self {
        PinEditor {
            name: pin.name,
            emoji: pin.emoji,
            program: pin.program.to_string_lossy().to_string(),
            args: pin.args,
            cwd: pin
                .cwd
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            to,
            edit,
            sel: 0,
            editing: false,
            status: String::new(),
            ui: Controls::new(),
        }
    }

    fn buf_mut(&mut self, sel: usize) -> Option<&mut String> {
        match sel {
            0 => Some(&mut self.name),
            1 => Some(&mut self.emoji),
            2 => Some(&mut self.program),
            3 => Some(&mut self.args),
            4 => Some(&mut self.cwd),
            _ => None,
        }
    }

    fn save(&mut self) -> Action {
        let program_str = self.program.trim();
        if program_str.is_empty() {
            self.status = "✗ program is required".into();
            return Action::None;
        }
        let program = PathBuf::from(program_str);
        let name = if self.name.trim().is_empty() {
            program
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "app".into())
        } else {
            self.name.trim().to_string()
        };
        let emoji = if self.emoji.trim().is_empty() {
            "🚀".to_string()
        } else {
            self.emoji.trim().to_string()
        };
        let cwd = {
            let c = self.cwd.trim();
            if c.is_empty() {
                None
            } else {
                Some(PathBuf::from(c))
            }
        };
        Action::SavePin {
            pin: PinnedApp {
                name,
                emoji,
                program,
                args: self.args.trim().to_string(),
                cwd,
            },
            to: self.to,
            edit: self.edit,
        }
    }

    fn field_line<'a>(&self, label: &'a str, value: &'a str, row: usize) -> Line<'a> {
        let selected = self.sel == row;
        // unselected values still read as input boxes
        let style = if selected { sel_style() } else { field_style() };
        let cursor = if selected && self.editing { "▏" } else { "" };
        Line::from(vec![
            Span::styled(format!("   {label:<13}"), Style::default().fg(DIM)),
            Span::styled(format!(" {value}{cursor} "), style),
        ])
    }
}

impl AppWindow for PinEditor {
    fn title(&self) -> String {
        "Pin settings".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 4 {
            return;
        }
        let accent = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(DIM);
        let txt = Style::default().fg(TXT);

        let mut lines: Vec<Line> = vec![
            Line::default(),
            Line::from(Span::styled(
                "   📌 Pin application — runs in the built-in terminal",
                accent,
            )),
            Line::default(),
            self.field_line("Name:", &self.name, 0),
            self.field_line("Icon:", &self.emoji, 1),
            self.field_line("Program:", &self.program, 2),
            self.field_line("Arguments:", &self.args, 3),
            self.field_line("Working dir:", &self.cwd, 4),
            Line::from(Span::styled("   Pin to:      ", dim)),
            Line::default(),
            Line::default(),
            Line::default(),
            Line::default(),
            Line::from(Span::styled(
                "   {field} or {field=default} in arguments is asked at each launch",
                dim,
            )),
        ];
        lines.truncate(area.height.saturating_sub(1) as usize);
        f.render_widget(
            Paragraph::new(lines),
            Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1)),
        );
        let _ = txt;
        self.ui.begin();
        let buf = f.buffer_mut();
        let bottom = area.y + area.height.saturating_sub(1);
        let x0 = area.x + 18;
        if area.y + 8 < bottom {
            let mut x = x0;
            for t in [PinTo::Desktop, PinTo::Taskbar] {
                let on = self.to == t;
                let label = match (t, on) {
                    (PinTo::Desktop, true) => "◉ Desktop",
                    (PinTo::Desktop, false) => "○ Desktop",
                    (_, true) => "◉ Taskbar",
                    (_, false) => "○ Taskbar",
                };
                let look = if self.sel == 5 && on { Look::On } else { Look::Raised };
                x += self.ui.button(buf, x, area.y + 8, label, Btn::Target(t), look) + 1;
            }
        }
        if area.y + 10 < bottom {
            let save = if self.sel == 6 { Look::On } else { Look::Primary };
            let cancel = if self.sel == 7 { Look::On } else { Look::Raised };
            let w = self.ui.button(buf, x0, area.y + 10, "▶ Save pin", Btn::Ok, save);
            self.ui.button(buf, x0 + w + 2, area.y + 10, "Cancel", Btn::Cancel, cancel);
        }
        ui::status_bar(buf, Rect::new(area.x, bottom, area.width, 1), &self.status);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        if self.editing {
            let sel = self.sel;
            if let Some(buf) = self.buf_mut(sel) {
                match k.code {
                    KeyCode::Esc => self.editing = false,
                    KeyCode::Enter | KeyCode::Tab => {
                        self.editing = false;
                        self.sel = (self.sel + 1).min(ED_ROWS - 1);
                    }
                    _ => {
                        super::text_edit(buf, k);
                    }
                }
                return Action::None;
            }
            self.editing = false;
        }
        self.status.clear();
        match k.code {
            KeyCode::Up => self.sel = self.sel.checked_sub(1).unwrap_or(ED_ROWS - 1),
            KeyCode::Down | KeyCode::Tab => self.sel = (self.sel + 1) % ED_ROWS,
            KeyCode::Char(' ') if self.sel == 5 => {
                self.to = match self.to {
                    PinTo::Desktop => PinTo::Taskbar,
                    PinTo::Taskbar => PinTo::Desktop,
                };
            }
            KeyCode::Enter => match self.sel {
                0..=4 => self.editing = true,
                5 => {
                    self.to = match self.to {
                        PinTo::Desktop => PinTo::Taskbar,
                        PinTo::Taskbar => PinTo::Desktop,
                    };
                }
                6 => return self.save(),
                _ => return Action::Close,
            },
            KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        if self.editing {
            let sel = self.sel;
            if let Some(buf) = self.buf_mut(sel) {
                super::paste_line(buf, text);
            }
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        if let Some(hit) = self.ui.route(&m) {
            self.editing = false;
            return match hit {
                Some(Btn::Ok) => self.save(),
                Some(Btn::Cancel) => Action::Close,
                Some(Btn::Target(t)) => {
                    self.sel = 5;
                    self.to = t;
                    Action::None
                }
                None => Action::None,
            };
        }
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            let rel = m.row.saturating_sub(area.y);
            let sel = match rel {
                3..=7 => Some(rel as usize - 3),
                _ => None,
            };
            if let Some(sel) = sel {
                self.editing = false;
                self.sel = sel;
                return self.on_key(KeyEvent::from(KeyCode::Enter));
            }
        }
        Action::None
    }
}

// ------------------------------------------------------------------ prompt

/// Launch-time dialog asking for the pin's `{field}` values.
pub struct PinPrompt {
    pin: PinnedApp,
    values: Vec<(String, String)>,
    sel: usize,
    editing: bool,
    ui: Controls<Btn>,
}

impl PinPrompt {
    pub fn new(pin: PinnedApp) -> Self {
        let values = pin.fields();
        let editing = !values.is_empty();
        PinPrompt {
            pin,
            values,
            sel: 0,
            editing,
            ui: Controls::new(),
        }
    }

    fn rows(&self) -> usize {
        self.values.len() + 2 // fields + Run + Cancel
    }

    fn run_row(&self) -> usize {
        self.values.len()
    }
}

impl AppWindow for PinPrompt {
    fn title(&self) -> String {
        format!("Run {}", self.pin.name)
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 4 {
            return;
        }
        let accent = Style::default().fg(ACCENT).add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(DIM);
        let txt = Style::default().fg(TXT);

        let mut lines: Vec<Line> = vec![
            Line::default(),
            Line::from(Span::styled(
                format!("   {} {} — startup arguments", self.pin.emoji, self.pin.name),
                accent,
            )),
            Line::from(Span::styled(
                format!("   {} {}", self.pin.program.to_string_lossy(), self.pin.args),
                dim,
            )),
            Line::default(),
        ];
        for (i, (name, val)) in self.values.iter().enumerate() {
            let selected = self.sel == i;
            let style = if selected { sel_style() } else { field_style() };
            let cursor = if selected && self.editing { "▏" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(format!("   {name:<14}"), dim),
                Span::styled(format!(" {val}{cursor} "), style),
            ]));
        }
        let _ = txt;
        lines.truncate(area.height.saturating_sub(1) as usize);
        f.render_widget(
            Paragraph::new(lines),
            Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1)),
        );
        self.ui.begin();
        let buf = f.buffer_mut();
        let y = area.y + 5 + self.values.len() as u16;
        if y < area.y + area.height {
            let run = if self.sel == self.run_row() { Look::On } else { Look::Primary };
            let cancel = if self.sel == self.run_row() + 1 { Look::On } else { Look::Raised };
            let w = self.ui.button(buf, area.x + 3, y, "▶ Run in terminal", Btn::Ok, run);
            self.ui.button(buf, area.x + 5 + w, y, "Cancel", Btn::Cancel, cancel);
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        if self.editing {
            if let Some((_, val)) = self.values.get_mut(self.sel) {
                match k.code {
                    KeyCode::Esc => self.editing = false,
                    KeyCode::Enter | KeyCode::Tab => {
                        self.sel += 1;
                        self.editing = self.sel < self.values.len();
                    }
                    _ => {
                        super::text_edit(val, k);
                    }
                }
                return Action::None;
            }
            self.editing = false;
        }
        match k.code {
            KeyCode::Up => self.sel = self.sel.checked_sub(1).unwrap_or(self.rows() - 1),
            KeyCode::Down | KeyCode::Tab => self.sel = (self.sel + 1) % self.rows(),
            KeyCode::Enter => {
                if self.sel < self.values.len() {
                    self.editing = true;
                } else if self.sel == self.run_row() {
                    return Action::LaunchAndClose(self.pin.launch_with(&self.values));
                } else {
                    return Action::Close;
                }
            }
            KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        if self.editing {
            if let Some((_, val)) = self.values.get_mut(self.sel) {
                super::paste_line(val, text);
            }
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        match self.ui.route(&m) {
            Some(Some(Btn::Ok)) => return Action::LaunchAndClose(self.pin.launch_with(&self.values)),
            Some(Some(Btn::Cancel)) => return Action::Close,
            Some(_) => return Action::None,
            None => {}
        }
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            let rel = m.row.saturating_sub(area.y) as usize;
            let n = self.values.len();
            let sel = if rel >= 4 && rel < 4 + n {
                Some(rel - 4)
            } else {
                None
            };
            if let Some(sel) = sel {
                self.editing = false;
                self.sel = sel;
                return self.on_key(KeyEvent::from(KeyCode::Enter));
            }
        }
        Action::None
    }
}
