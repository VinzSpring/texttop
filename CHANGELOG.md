# Changelog

## 0.2.0 — 2026-09-09

### Added
- **Studio** — electronic music app: genre templates, session clip grid, piano roll, arrangement, FX chips, `.ttstudio` save/load, WAV export (requires the `audio` feature).
  - Piano roll: note lengths (drag or `[` `]`), audition on entry, clips up to 4 bars with scrolling, octave paging.
  - Arrangement: place the selected clip on a lane, drag to move (across compatible tracks), `[` `]` to resize, Delete / right-click to remove; playback loops the arrangement.
  - Tracks: add (`A` or `+`) and delete (`X`); mixer volume, pan and filter cutoff by scroll wheel.
  - Session: clicking a playing clip stops it, `D` duplicates into the next scene, browser presets arm and fill empty cells.
  - WAV export runs in the background.
- Wallpaper chooser with gradient themes and image pick via Files.
- Desktop clipboard (OSC 52) used by Terminal and other apps.
- Browser cookie inventory, cookie policy, site-data clearing and private windows on Windows (WebView2), macOS and Linux (WebKitGTK; cookies now persist between runs there).
- Auto-detect apps on PATH and pin them to the desktop.
- Hybrid web rendering: page text drawn as terminal characters over pixel frames.

### Changed
- Session restore no longer auto-spawns a hosted desktop app; the command is offered in the AppHost picker for confirmation.
- Session restore reopens a Studio project under the name it was last saved as.
- Snapshot blob ids are validated before read/restore.
- Git checkout / create-branch reject names that look like flags.

### Fixed
- Studio: resuming after a pause could leave the transport stuck; held notes and looped samples kept sounding after Stop or when a clip was stopped; loaded projects played at the previous tempo; pressing `0` on the template screen crashed; drawing could spill outside the window; new projects all saved over the same `untitled.ttstudio`; quitting or opening a project discarded unsaved changes without asking; long recordings could produce a file that would not load.
- Flaky image-viewer snapshot test (tests shared one image folder).
- AltGr characters (`\ @ { } [ ] | ~ €` on German and similar layouts) were dropped by the Editor, Terminal and every text field, because Windows reports AltGr as Ctrl+Alt.
- `TEXTTOP_HOME` now isolates everything texttop stores — browser history and settings, notes, snapshots and Studio projects — not just the desktop layout and session.

### Docs
- `scripts/screenshots.py` drives the real app and regenerates `docs/screenshots/`, `docs/HANDBOOK.md` and the README gallery.

### Known limitations
See the README. Notable: macOS YouTube video grid stays empty in the off-screen WebView; AppHost is Windows-only.
