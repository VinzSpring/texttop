//! "Run app here": opened from the File Manager, it lists the built-in apps
//! and the user's own pinned apps. Picking one starts it with the folder as
//! its working directory, or hands it the selected files.

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::pin::{fill_template, split_args, PinnedApp};
use super::scroll::ScrollState;
use super::ui::{self, Controls, Item, Look};
use super::{launch_for_path, Action, AppWindow, Launch};

const BG: Color = Color::Rgb(12, 14, 22);
const FG: Color = Color::Rgb(215, 218, 230);
const DIM: Color = Color::Rgb(110, 118, 145);
const HINT: Color = Color::Rgb(140, 150, 175);
const ACCENT: Color = Color::Rgb(0, 190, 255);
const SEL_BG: Color = Color::Rgb(28, 40, 66);

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Row(usize),
    PassFiles,
    Run,
    Cancel,
}

#[derive(Clone, Copy, PartialEq)]
enum Builtin {
    Default,
    Files,
    Terminal,
    Editor,
    Hex,
    Image,
    Audio,
    Video,
    Web,
    Studio,
    Git,
    Search,
    Snapshots,
    Compare,
}

/// Built-in apps that make sense with a folder or files.
const BUILTINS: &[(&str, &str, Builtin)] = &[
    ("✨", "Default app", Builtin::Default),
    ("🗂", "File Manager", Builtin::Files),
    ("🖥", "Terminal", Builtin::Terminal),
    ("📝", "Text Editor", Builtin::Editor),
    ("🔢", "Hex Viewer", Builtin::Hex),
    ("🖼", "Image Viewer", Builtin::Image),
    ("🎵", "Audio Player", Builtin::Audio),
    ("🎬", "Video Player", Builtin::Video),
    ("🌐", "Web Browser", Builtin::Web),
    ("🎛", "Studio", Builtin::Studio),
    ("🌿", "Git", Builtin::Git),
    ("🔎", "Project Search", Builtin::Search),
    ("⏱", "Snapshots", Builtin::Snapshots),
    ("↔", "Compare", Builtin::Compare),
];

enum Target {
    Builtin(Builtin),
    /// An app group, by id.
    Group(String),
    Pin(PinnedApp),
}

struct Choice {
    emoji: String,
    name: String,
    target: Target,
}

/// One line of the list: a group header or a choice index.
enum Row {
    Header(&'static str),
    Choice(usize),
}

pub struct RunHere {
    dir: PathBuf,
    files: Vec<PathBuf>,
    /// Hand the selected files to the app (otherwise just run in `dir`).
    pass_files: bool,
    choices: Vec<Choice>,
    rows: Vec<Row>,
    sel: usize,
    scroll: usize,
    view_h: usize,
    status: String,
    ui: Controls<Btn>,
    bar: ScrollState,
    /// Keep the selection in view (off while the wheel or scrollbar scroll).
    follow: bool,
}

impl RunHere {
    /// `pins` are the user's desktop / taskbar apps, already deduplicated.
    pub fn new(dir: PathBuf, files: Vec<PathBuf>, pins: Vec<PinnedApp>) -> Self {
        let mut choices: Vec<Choice> = BUILTINS
            .iter()
            .map(|&(e, n, b)| Choice {
                emoji: e.into(),
                name: n.into(),
                target: Target::Builtin(b),
            })
            .collect();
        let mut rows = vec![Row::Header("Built-in apps")];
        rows.extend((0..choices.len()).map(Row::Choice));
        let groups = super::group::all();
        if !groups.is_empty() {
            rows.push(Row::Header("App groups"));
            for g in groups {
                rows.push(Row::Choice(choices.len()));
                choices.push(Choice {
                    emoji: g.emoji,
                    name: g.name,
                    target: Target::Group(g.id),
                });
            }
        }
        if !pins.is_empty() {
            rows.push(Row::Header("Your apps"));
            for pin in pins {
                rows.push(Row::Choice(choices.len()));
                choices.push(Choice {
                    emoji: pin.emoji.clone(),
                    name: pin.name.clone(),
                    target: Target::Pin(pin),
                });
            }
        }
        let mut s = RunHere {
            pass_files: !files.is_empty(),
            dir,
            files,
            choices,
            rows,
            sel: 0,
            scroll: 0,
            view_h: 12,
            status: String::new(),
            ui: Controls::new(),
            bar: ScrollState::default(),
            follow: true,
        };
        // Start on the first app that can do something with the selection.
        s.sel = (0..s.choices.len()).find(|&i| s.plan(i).is_ok()).unwrap_or(0);
        s
    }

    fn files(&self) -> &[PathBuf] {
        if self.pass_files {
            &self.files
        } else {
            &[]
        }
    }

    /// The windows choice `i` would open, or why it cannot run right now.
    fn plan(&self, i: usize) -> Result<Vec<Launch>, String> {
        let files = self.files();
        let dir = self.dir.clone();
        let each = |f: fn(PathBuf) -> Launch| -> Result<Vec<Launch>, String> {
            if files.is_empty() {
                Err("needs a selected file".into())
            } else {
                Ok(files.iter().cloned().map(f).collect())
            }
        };
        let Some(choice) = self.choices.get(i) else {
            return Err(String::new());
        };
        match &choice.target {
            Target::Pin(pin) => Ok(vec![pin_launch(pin, &dir, files)]),
            Target::Group(id) => Ok(vec![Launch::Group {
                id: id.clone(),
                cwd: Some(dir),
                open: files.to_vec(),
            }]),
            Target::Builtin(b) => match b {
                Builtin::Default => each(|p| launch_for_path(&p)),
                Builtin::Files => Ok(vec![Launch::Files(dir)]),
                Builtin::Terminal => Ok(vec![Launch::Terminal {
                    program: None,
                    args: Vec::new(),
                    cwd: Some(dir),
                }]),
                Builtin::Editor if files.is_empty() => Ok(vec![Launch::Editor(None)]),
                Builtin::Editor => each(|p| Launch::Editor(Some(p))),
                Builtin::Hex if files.is_empty() => Ok(vec![Launch::Hex(None)]),
                Builtin::Hex => each(|p| Launch::Hex(Some(p))),
                Builtin::Image => each(Launch::Image),
                Builtin::Audio => each(Launch::Audio),
                Builtin::Video => each(Launch::Video),
                Builtin::Web if files.is_empty() => Ok(vec![Launch::Web(None)]),
                Builtin::Web => each(|p| Launch::Web(Some(p.to_string_lossy().to_string()))),
                Builtin::Studio if files.is_empty() => Ok(vec![Launch::Studio(None)]),
                Builtin::Studio => {
                    if files.iter().all(|p| has_ext(p, "ttstudio")) {
                        each(|p| Launch::Studio(Some(p)))
                    } else {
                        Err("opens .ttstudio projects only".into())
                    }
                }
                Builtin::Git => Ok(vec![Launch::Git(Some(dir))]),
                Builtin::Search => Ok(vec![Launch::Search(Some(dir))]),
                Builtin::Snapshots => each(Launch::Snapshots),
                Builtin::Compare => {
                    if files.len() >= 2 {
                        Ok(vec![Launch::Compare(files.to_vec())])
                    } else {
                        Err("needs 2+ selected files".into())
                    }
                }
            },
        }
    }

    /// What picking choice `i` does, in a few words.
    fn hint(&self, i: usize) -> (String, bool) {
        let n = self.files().len();
        let opens = |n: usize| {
            if n == 1 {
                "opens the file".to_string()
            } else {
                format!("opens {n} files")
            }
        };
        match self.plan(i) {
            Err(why) => (why, false),
            Ok(_) => {
                let text = match &self.choices[i].target {
                    Target::Pin(_) if n > 0 => format!("runs here with {}", plural(n, "file")),
                    Target::Pin(_) => "runs in this folder".into(),
                    Target::Group(_) if n > 0 => format!("opens here with {}", plural(n, "file")),
                    Target::Group(_) => "opens in this folder".into(),
                    Target::Builtin(
                        Builtin::Files | Builtin::Terminal | Builtin::Git | Builtin::Search,
                    ) => "starts in this folder".into(),
                    Target::Builtin(Builtin::Compare) => format!("compares {n} files"),
                    Target::Builtin(_) if n > 0 => opens(n),
                    Target::Builtin(_) => "starts empty".into(),
                };
                (text, true)
            }
        }
    }

    fn run(&mut self, i: usize) -> Action {
        match self.plan(i) {
            Ok(launches) => Action::LaunchManyAndClose(launches),
            Err(why) => {
                self.status = format!("{}: {why}", self.choices[i].name);
                Action::None
            }
        }
    }

    fn row_of(&self, choice: usize) -> usize {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Choice(c) if *c == choice))
            .unwrap_or(0)
    }

    fn move_sel(&mut self, delta: isize) {
        let n = self.choices.len() as isize;
        if n > 0 {
            self.sel = (self.sel as isize + delta).rem_euclid(n) as usize;
            self.status.clear();
            self.follow = true;
            self.ensure_visible();
        }
    }

    fn ensure_visible(&mut self) {
        let mut row = self.row_of(self.sel);
        // Keep a group header in view above its first entry.
        if row > 0 && matches!(self.rows[row - 1], Row::Header(_)) {
            row -= 1;
        }
        if row < self.scroll {
            self.scroll = row;
        }
        let r = self.row_of(self.sel);
        if r >= self.scroll + self.view_h {
            self.scroll = r + 1 - self.view_h;
        }
    }
}

/// A pinned app started in `dir`, with `files` appended to its arguments.
fn pin_launch(pin: &PinnedApp, dir: &Path, files: &[PathBuf]) -> Launch {
    let files = files.iter().map(|p| p.to_string_lossy().to_string());
    if pin.fields().is_empty() {
        let mut args = split_args(&fill_template(&pin.args, &[]));
        args.extend(files);
        return Launch::Terminal {
            program: Some(pin.program.clone()),
            args,
            cwd: Some(dir.to_path_buf()),
        };
    }
    // Placeholders still need asking: let the pin prompt run it.
    let mut pin = pin.clone();
    pin.cwd = Some(dir.to_path_buf());
    for f in files {
        pin.args.push_str(&format!(" \"{f}\""));
    }
    Launch::PinPrompt(pin)
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// Keep the tail of a long path, which is the part that tells folders apart.
fn shorten(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        let tail: String = s.chars().skip(n + 1 - max.max(2)).collect();
        format!("…{tail}")
    }
}

impl AppWindow for RunHere {
    fn title(&self) -> String {
        "Run app here".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        let buf = f.buffer_mut();
        for y in area.y..area.y + area.height {
            ui::fill_row(buf, Rect::new(area.x, y, area.width, 1), BG);
        }
        self.ui.begin();
        if area.height < 4 {
            return;
        }

        // Where it runs, and whether the selection goes along.
        let right: Vec<Item<Btn>> = if self.files.is_empty() {
            Vec::new()
        } else {
            let mark = if self.pass_files { "☑" } else { "☐" };
            vec![Item::with(
                format!("{mark} Open {}", plural(self.files.len(), "selected file")),
                Btn::PassFiles,
                Look::on_if(self.pass_files),
            )]
        };
        let toggle_w: u16 = right
            .iter()
            .map(|it| match it {
                Item::Button(l, ..) => l.width() as u16 + 3,
                _ => 0,
            })
            .sum();
        let dir = self.dir.to_string_lossy();
        let room = area.width.saturating_sub(toggle_w + 4) as usize;
        self.ui.toolbar(
            buf,
            Rect::new(area.x, area.y, area.width, 1),
            &[Item::Label(format!(" 📁 {}", shorten(&dir, room.max(8))), FG)],
            &right,
        );

        // What is selected, under the toolbar.
        let names: Vec<String> = self
            .files()
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        let sub = if names.is_empty() {
            " Pick an app to start in this folder".to_string()
        } else {
            format!(" Pick an app for {}", names.join(", "))
        };
        let sub = shorten_end(&sub, area.width as usize);
        buf.set_string(area.x, area.y + 1, &sub, Style::default().fg(HINT).bg(BG));

        // App list.
        let list_top = area.y + 2;
        let list_h = area.height.saturating_sub(3).max(1) as usize;
        self.view_h = list_h;
        if self.follow {
            self.ensure_visible();
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(list_h));
        let name_w = self
            .choices
            .iter()
            .map(|c| c.name.width())
            .max()
            .unwrap_or(10)
            .min(22);
        for (r, row) in self.rows.iter().enumerate().skip(self.scroll).take(list_h) {
            let y = list_top + (r - self.scroll) as u16;
            let line = Rect::new(area.x, y, area.width, 1);
            match row {
                Row::Header(h) => {
                    buf.set_string(
                        area.x + 1,
                        y,
                        *h,
                        Style::default()
                            .fg(ACCENT)
                            .bg(BG)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                Row::Choice(i) => {
                    let c = &self.choices[*i];
                    let (hint, ok) = self.hint(*i);
                    let hot = ok && self.ui.hot(line);
                    let bg = if *i == self.sel || hot { SEL_BG } else { BG };
                    ui::fill_row(buf, line, bg);
                    let mut name = c.name.clone();
                    while name.width() > name_w {
                        name.pop();
                    }
                    let pad = name_w - name.width();
                    let fg = if ok { FG } else { DIM };
                    let mut st = Style::default().fg(fg).bg(bg);
                    if *i == self.sel {
                        st = st.add_modifier(Modifier::BOLD);
                        buf.set_string(area.x, y, "▸", Style::default().fg(ACCENT).bg(bg));
                    }
                    buf.set_string(area.x + 2, y, &c.emoji, st);
                    let nx = area.x + 5;
                    buf.set_string(nx, y, format!("{name}{:pad$}", ""), st);
                    let hx = nx + name_w as u16 + 2;
                    if hx < area.x + area.width {
                        let max = (area.x + area.width - hx) as usize;
                        buf.set_string(
                            hx,
                            y,
                            shorten_end(&hint, max),
                            Style::default().fg(if ok { HINT } else { DIM }).bg(bg),
                        );
                    }
                    if ok {
                        self.ui.add(line, Btn::Row(*i));
                    }
                }
            }
        }

        self.bar.offset = self.scroll as u16;
        let bar = (self.rows.len() > list_h)
            .then(|| Rect::new(area.x + area.width - 1, list_top, 1, list_h as u16));
        self.bar.show_bar(buf, bar, self.rows.len(), list_h as u16);

        // Bottom bar: status on the left, Run / Cancel on the right.
        let (_, ok) = self.hint(self.sel);
        let run = format!(
            "▶ Run {}",
            self.choices.get(self.sel).map(|c| c.name.as_str()).unwrap_or("")
        );
        let bottom = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        let status = if self.status.is_empty() {
            Vec::new()
        } else {
            vec![Item::Label(format!(" {}", self.status), Color::Rgb(255, 170, 110))]
        };
        self.ui.toolbar(
            buf,
            bottom,
            &status,
            &[
                Item::with(run, Btn::Run, if ok { Look::Primary } else { Look::Disabled }),
                Item::with("Cancel", Btn::Cancel, Look::Raised),
            ],
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc => Action::Close,
            KeyCode::Up => {
                self.move_sel(-1);
                Action::None
            }
            KeyCode::Down => {
                self.move_sel(1);
                Action::None
            }
            KeyCode::Home => {
                self.sel = 0;
                self.ensure_visible();
                Action::None
            }
            KeyCode::End => {
                self.sel = self.choices.len().saturating_sub(1);
                self.ensure_visible();
                Action::None
            }
            KeyCode::Enter => self.run(self.sel),
            _ => Action::None,
        }
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        self.bar.offset = self.scroll as u16;
        if self.bar.on_mouse(m, self.rows.len(), self.view_h as u16) {
            self.scroll = self.bar.offset as usize;
            self.follow = false;
            return Action::None;
        }
        match self.ui.route(&m) {
            Some(Some(Btn::Row(i))) => {
                self.sel = i;
                return self.run(i);
            }
            Some(Some(Btn::PassFiles)) => {
                self.pass_files = !self.pass_files;
                self.status.clear();
            }
            Some(Some(Btn::Run)) => return self.run(self.sel),
            Some(Some(Btn::Cancel)) => return Action::Close,
            Some(None) => {}
            None => match m.kind {
                _ => {}
            },
        }
        Action::None
    }
}

fn shorten_end(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if out.width() + 2 > max {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chooser(files: &[&str]) -> RunHere {
        let pin = PinnedApp {
            name: "tool".into(),
            emoji: "🚀".into(),
            program: PathBuf::from("tool"),
            args: "--flag".into(),
            cwd: None,
        };
        RunHere::new(
            PathBuf::from("work"),
            files.iter().map(PathBuf::from).collect(),
            vec![pin],
        )
    }

    fn index(r: &RunHere, name: &str) -> usize {
        r.choices.iter().position(|c| c.name == name).unwrap()
    }

    #[test]
    fn folder_apps_start_in_the_folder() {
        let r = chooser(&[]);
        match r.plan(index(&r, "Terminal")).unwrap().as_slice() {
            [Launch::Terminal { cwd, .. }] => assert_eq!(cwd.as_deref(), Some(Path::new("work"))),
            _ => panic!("expected a terminal"),
        }
        assert!(r.plan(index(&r, "Image Viewer")).is_err());
    }

    #[test]
    fn each_selected_file_opens_in_its_own_window() {
        let r = chooser(&["a.txt", "b.txt"]);
        assert_eq!(r.plan(index(&r, "Text Editor")).unwrap().len(), 2);
        assert_eq!(r.plan(index(&r, "Compare")).unwrap().len(), 1);
        assert!(r.plan(index(&r, "Studio")).is_err());
    }

    #[test]
    fn pins_get_the_folder_and_the_files() {
        let r = chooser(&["a.txt"]);
        match r.plan(index(&r, "tool")).unwrap().as_slice() {
            [Launch::Terminal { args, cwd, .. }] => {
                assert_eq!(args, &["--flag".to_string(), "a.txt".to_string()]);
                assert_eq!(cwd.as_deref(), Some(Path::new("work")));
            }
            _ => panic!("expected a terminal"),
        }
    }

    #[test]
    fn unticking_files_runs_in_the_folder_only() {
        let mut r = chooser(&["a.txt"]);
        r.pass_files = false;
        match r.plan(index(&r, "Text Editor")).unwrap().as_slice() {
            [Launch::Editor(None)] => {}
            _ => panic!("expected an empty editor"),
        }
    }
}
