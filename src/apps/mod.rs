pub mod about;
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
pub mod editor;
pub mod filebrowser;
pub mod hexviewer;
pub mod imageviewer;
pub mod launcher;
pub mod notes;
pub mod syntax;
pub mod sysinfo;
pub mod terminal;
pub mod videoplayer;
pub mod webbrowser;

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

/// What an app asks the desktop to do after handling input.
pub enum Action {
    None,
    Close,
    Launch(Launch),
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
    About,
    Calculator,
    Clock,
    Hex(Option<PathBuf>),
    Calendar,
    SysInfo,
    Notes,
}

pub trait AppWindow {
    fn title(&self) -> String;
    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool);
    fn on_key(&mut self, key: KeyEvent) -> Action;
    fn on_mouse(&mut self, _m: MouseEvent, _area: Rect) -> Action {
        Action::None
    }
    fn on_tick(&mut self) {}
}

/// Pick the right app for a file based on its extension.
pub fn launch_for_path(p: &Path) -> Launch {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
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
