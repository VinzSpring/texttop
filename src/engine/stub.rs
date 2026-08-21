//! No-op engine for platforms/builds without an embedded web engine.

use std::sync::Arc;

use super::{Cmd, CookieInfo, Frame, PageInfo, Status, TextRun};

pub struct Engine;

impl Engine {
    pub fn start(_initial: Cmd, _zoom: f64, _w: i32, _h: i32, _private: bool) -> Engine {
        Engine
    }

    pub fn send(&self, _cmd: Cmd) {}

    pub fn frame(&self) -> Option<Arc<Frame>> {
        None
    }

    pub fn info(&self) -> PageInfo {
        PageInfo::default()
    }

    pub fn texts(&self) -> Vec<TextRun> {
        Vec::new()
    }

    pub fn cookies(&self) -> Vec<CookieInfo> {
        Vec::new()
    }

    pub fn status(&self) -> Status {
        Status::Error(
            "web engine not available in this build. Rebuild with default features. \
             Linux needs WebKitGTK dev packages (Ubuntu/Debian: libwebkit2gtk-4.1-dev · \
             Fedora: webkit2gtk4.1-devel). macOS needs Xcode Command Line Tools \
             (`xcode-select --install`) on macOS 13+."
                .into(),
        )
    }
}
