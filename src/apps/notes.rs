//! Sticky notes — the text editor bound to a fixed file in the home directory.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

use super::editor::Editor;
use super::{Action, AppWindow};

/// Seeded into a brand-new notes file so the first run is not a blank window.
const WELCOME: &[&str] = &[
    "Sticky notes",
    "",
    "Jot anything here — saved to ~/.texttop_notes.txt",
    "",
];

fn notes_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".texttop_notes.txt")
}

pub struct Notes {
    editor: Editor,
}

impl Notes {
    pub fn new() -> Self {
        let path = notes_path();
        // The file is only written on save, so a first run has nothing to read.
        let editor = if path.exists() {
            Editor::new(Some(path))
        } else {
            Editor::with_lines(
                Some(path),
                WELCOME.iter().map(|s| s.to_string()).collect(),
                "new notes file · Ctrl+S saves".into(),
            )
        };
        Notes { editor }
    }
}

impl AppWindow for Notes {
    fn title(&self) -> String {
        // Always "Notes", never the file name — the path is an implementation
        // detail of this app.
        if self.editor.is_modified() {
            "Notes •".into()
        } else {
            "Notes".into()
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        self.editor.render(f, area, focused);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        self.editor.on_key(k)
    }

    fn on_paste(&mut self, text: &str) -> Action {
        self.editor.on_paste(text)
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        self.editor.on_mouse(m, area)
    }
}
