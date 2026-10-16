pub mod about;
pub mod apphost;
#[cfg(feature = "audio")]
pub mod audioplayer;
#[cfg(not(feature = "audio"))]
pub mod audioplayer {
    //! Stub shown when texttop is built without audio support.
    use std::path::PathBuf;

    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;
    use ratatui::Frame;

    use super::{Action, AppWindow};

    pub struct AudioPlayer {
        name: String,
    }

    impl AudioPlayer {
        pub fn new(path: PathBuf) -> Self {
            AudioPlayer {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
            }
        }
    }

    impl AppWindow for AudioPlayer {
        fn title(&self) -> String {
            self.name.clone()
        }

        fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
            f.render_widget(
                Paragraph::new(vec![
                    Line::default(),
                    Line::from(Span::styled(
                        "  ✗ audio support was not included in this build",
                        Style::default().fg(Color::Rgb(255, 110, 110)),
                    )),
                    Line::from(Span::styled(
                        "    rebuild with default features (Linux: install libasound2-dev / alsa-lib-devel)",
                        Style::default().fg(Color::Rgb(150, 155, 175)),
                    )),
                ]),
                area,
            );
        }

        fn on_key(&mut self, k: KeyEvent) -> Action {
            match k.code {
                KeyCode::Char('q') | KeyCode::Esc => Action::Close,
                _ => Action::None,
            }
        }
    }
}
pub mod calculator;
pub mod calendar;
pub mod clock;
pub mod compare;
pub mod contribute;
pub mod detect;
pub mod editor;
pub mod filebrowser;
pub mod game2048;
pub mod git;
pub mod gitutil;
pub mod group;
pub mod groupedit;
pub mod hexviewer;
pub mod imageviewer;
pub mod launcher;
pub mod minesweeper;
pub mod notes;
pub mod pin;
pub mod processes;
pub mod runhere;
pub mod scroll;
pub mod search;
pub mod snake;
pub mod snaputil;
pub mod snapshots;
pub mod syntax;
pub mod sysinfo;
pub mod terminal;
pub mod ui;
pub mod videoplayer;
pub mod wallpaper;
pub mod webbrowser;
pub mod webdata;
#[cfg(feature = "audio")]
pub mod studio;
#[cfg(not(feature = "audio"))]
pub mod studio {
    //! Stub shown when texttop is built without audio support.
    use std::path::PathBuf;

    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;
    use ratatui::Frame;

    use super::{Action, AppWindow};

    pub struct Studio;

    impl Studio {
        pub fn new(_path: Option<PathBuf>) -> Self {
            Studio
        }
    }

    impl AppWindow for Studio {
        fn title(&self) -> String {
            "Studio".into()
        }

        fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
            f.render_widget(
                Paragraph::new(vec![
                    Line::default(),
                    Line::from(Span::styled(
                        "  ✗ audio support was not included in this build",
                        Style::default().fg(Color::Rgb(255, 110, 110)),
                    )),
                    Line::from(Span::styled(
                        "    rebuild with default features (Linux: install libasound2-dev / alsa-lib-devel)",
                        Style::default().fg(Color::Rgb(150, 155, 175)),
                    )),
                ]),
                area,
            );
        }

        fn on_key(&mut self, k: KeyEvent) -> Action {
            match k.code {
                KeyCode::Char('q') | KeyCode::Esc => Action::Close,
                _ => Action::None,
            }
        }
    }
}

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

/// What an app asks the desktop to do after handling input.
pub enum Action {
    None,
    Close,
    Launch(Launch),
    /// Launch something and close the requesting window (one-shot dialogs).
    LaunchAndClose(Launch),
    /// Open several windows and close the requesting one (Run app here).
    LaunchManyAndClose(Vec<Launch>),
    /// A picker (File browser in path-picking mode) chose this folder or file.
    PickPath(PathBuf),
    /// Start dragging a file; desktop shows a ghost and handles drop targets.
    BeginFileDrag { path: PathBuf, cut: bool },
    /// Add a desktop shortcut for this path.
    PinToDesktop(PathBuf),
    /// Save (create or replace) a pinned application and close the editor.
    SavePin {
        pin: pin::PinnedApp,
        to: pin::PinTo,
        edit: Option<pin::PinEdit>,
    },
    /// Apply a wallpaper theme index (live preview from the chooser).
    SetTheme(usize),
    /// Apply a wallpaper theme and close the chooser.
    SetThemeAndClose(usize),
    /// Cover-fill an image as the desktop wallpaper.
    SetWallpaperImage(PathBuf),
    /// Cover-fill an image as wallpaper and close the requesting window.
    SetWallpaperImageAndClose(PathBuf),
    /// Pin one or more detected CLI apps (closes the chooser).
    AddDetectedPins {
        pins: Vec<pin::PinnedApp>,
        to: pin::PinTo,
    },
}

#[derive(Clone, Debug)]
pub enum Launch {
    Files(PathBuf),
    Web(Option<String>),
    Audio(PathBuf),
    Image(PathBuf),
    Video(PathBuf),
    Editor(Option<PathBuf>),
    Terminal {
        program: Option<PathBuf>,
        args: Vec<String>,
        cwd: Option<PathBuf>,
    },
    AppLauncher(PathBuf),
    /// Settings dialog for a new or existing pinned application.
    PinEditor {
        pin: pin::PinnedApp,
        to: pin::PinTo,
        edit: Option<pin::PinEdit>,
    },
    /// Launch-time dialog querying `{field}` values of a pinned application.
    PinPrompt(pin::PinnedApp),
    About,
    Calculator,
    Clock,
    Hex(Option<PathBuf>),
    Calendar,
    SysInfo,
    Notes,
    Git(Option<PathBuf>),
    Search(Option<PathBuf>),
    Processes,
    /// Per-file snapshot history viewer.
    Snapshots(PathBuf),
    /// Visual diff of N files (pick any two).
    Compare(Vec<PathBuf>),
    Snake,
    Minesweeper,
    Game2048,
    /// Mirror a real desktop application into a texttop window (Windows).
    /// Optional `cmd` is launched immediately (e.g. VS Code path).
    AppHost { cmd: Option<String> },
    /// Wallpaper theme picker with gradient previews.
    Wallpaper,
    /// File browser in "pick an image for wallpaper" mode.
    WallpaperPicker { start: PathBuf },
    /// Chooser for CLI tools found on PATH.
    DetectedApps,
    /// Electronic music studio (session / arrange / piano).
    Studio(Option<PathBuf>),
    /// Chooser that runs any app in `dir`, or opens `files` with it.
    RunHere { dir: PathBuf, files: Vec<PathBuf> },
    /// An app group ("virtual app") started in `cwd`, with `open` as tabs.
    Group {
        id: String,
        cwd: Option<PathBuf>,
        open: Vec<PathBuf>,
    },
    /// App Groups settings, optionally with one group selected.
    GroupEditor(Option<String>),
    /// Download texttop's source and open it in the IDE.
    Contribute,
}

pub trait AppWindow {
    fn title(&self) -> String;
    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool);
    fn on_key(&mut self, key: KeyEvent) -> Action;
    fn on_mouse(&mut self, _m: MouseEvent, _area: Rect) -> Action {
        Action::None
    }
    fn on_tick(&mut self) {}
    /// Something the app wants done that did not come from input (a
    /// background job finished). Polled after every tick.
    fn take_action(&mut self) -> Action {
        Action::None
    }
    /// Text pasted into this window as a single clipboard event.
    fn on_paste(&mut self, _text: &str) -> Action {
        Action::None
    }
    /// Whether this window can accept a file drop.
    fn accepts_drop(&self) -> bool {
        false
    }
    /// Accept a file dropped onto this window (e.g. File Manager paste / open).
    fn on_drop(&mut self, _paths: &[PathBuf], _cut: bool) -> Action {
        Action::None
    }
    /// Path the desktop should treat as the active workspace (git / search).
    fn workspace_path(&self) -> Option<PathBuf> {
        None
    }
    /// Desktop wallpaper changed — pickers can refresh their applied state.
    fn on_wallpaper_changed(&mut self, _image: Option<PathBuf>, _theme: usize) {}
    /// How to reopen this window on the next start, when that has changed
    /// since launch (e.g. a document saved under a new path).
    fn session_launch(&self) -> Option<Launch> {
        None
    }
}

/// Where texttop keeps its own data: desktop layout, session, notes,
/// snapshots, browser history and Studio projects.
///
/// `TEXTTOP_HOME` overrides it, which gives an isolated profile for tests,
/// screenshots and trying things without disturbing a real desktop. Note this
/// cannot go through `$HOME`: `dirs::home_dir` resolves the account's home
/// directly on macOS and Windows and ignores the variable.
pub fn state_home() -> PathBuf {
    if let Some(p) = std::env::var_os("TEXTTOP_HOME") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Apply a keypress to a single-line text field, returning true when the key
/// was consumed so callers can fall through to their own bindings otherwise.
/// Characters typed with Ctrl or Alt held are shortcuts, not text, and are
/// left for the caller.
pub fn text_edit(buf: &mut String, k: KeyEvent) -> bool {
    match k.code {
        KeyCode::Backspace => {
            buf.pop();
            true
        }
        KeyCode::Char(c)
            if !k
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            buf.push(c);
            true
        }
        _ => false,
    }
}

/// Insert clipboard text into a single-line field. A newline would break the
/// field, so only the first pasted line is taken.
pub fn paste_line(buf: &mut String, text: &str) {
    buf.push_str(text.lines().next().unwrap_or("").trim_end());
}

/// Xorshift64 PRNG step. The games only need cheap, non-cryptographic
/// randomness, which is not worth a dependency.
pub fn xorshift(s: &mut u64) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    (*s >> 32) as u32
}

/// Pick the right app for a file based on its extension.
pub fn launch_for_path(p: &Path) -> Launch {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "ttstudio" => Launch::Studio(Some(p.to_path_buf())),
        "mp3" | "wav" | "flac" | "ogg" => Launch::Audio(p.to_path_buf()),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico" => {
            Launch::Image(p.to_path_buf())
        }
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "m4v" | "wmv" | "flv" => {
            Launch::Video(p.to_path_buf())
        }
        "html" | "htm" => Launch::Web(Some(p.to_string_lossy().to_string())),
        "exe" | "com" | "bat" | "cmd" | "ps1" | "sh" | "run" | "appimage" | "command" => {
            Launch::AppLauncher(p.to_path_buf())
        }
        _ => {
            // macOS .app bundles are directories
            #[cfg(target_os = "macos")]
            {
                if p.is_dir() && ext == "app" {
                    return Launch::AppLauncher(p.to_path_buf());
                }
            }
            // Unix: extensionless executables get the launcher too
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if p.metadata()
                    .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
                {
                    return Launch::AppLauncher(p.to_path_buf());
                }
            }
            Launch::Editor(Some(p.to_path_buf()))
        }
    }
}

pub fn file_icon(name: &str, is_dir: bool) -> (&'static str, ratatui::style::Color) {
    use ratatui::style::Color;
    if is_dir {
        return ("📁", Color::Rgb(120, 190, 255));
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "ttstudio" => ("🎛", Color::Rgb(255, 140, 200)),
        "mp3" | "wav" | "flac" | "ogg" => ("🎵", Color::Rgb(190, 140, 255)),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico" => {
            ("🖼", Color::Rgb(120, 220, 160))
        }
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "m4v" | "wmv" | "flv" => {
            ("🎬", Color::Rgb(255, 170, 90))
        }
        "html" | "htm" => ("🌐", Color::Rgb(90, 200, 255)),
        "rs" | "py" | "js" | "ts" | "c" | "cpp" | "h" | "java" | "go" | "toml" | "json"
        | "yml" | "yaml" => ("📜", Color::Rgb(255, 220, 120)),
        "zip" | "7z" | "rar" | "gz" | "tar" => ("📦", Color::Rgb(200, 160, 120)),
        "exe" | "msi" | "bat" | "cmd" | "ps1" | "command" | "app" | "appimage" => {
            ("⚙", Color::Rgb(200, 200, 210))
        }
        _ => ("📄", Color::Rgb(180, 185, 200)),
    }
}
