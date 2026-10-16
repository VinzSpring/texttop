# Changelog

## 0.3.0 — 2026-10-17

### Added
- **App groups** — "virtual apps" made of several frames (file explorer, terminal, editor, git, …) docked around a tab area and started together in one folder. Listed in the start menu and in Run app here; edited in the new **App Groups** settings with a live layout sketch. A group's top bar has **Open folder/file** (a file-explorer picker) and **Settings**; frames resize by dragging their edge.
- **IDE** — built-in app group (explorer left, terminal bottom, everything opened inside lands in a tab), with its own desktop icon.
- **Contribute / Customize** — downloads texttop's source with git into its data folder and opens it in the IDE.
- **Run app here…** in Files — start any built-in app, app group or pinned app in the current folder, or open the selected files with it.
- Text editor: **find and replace** bar (Ctrl+F, Ctrl+H, F3 / Shift+F3, match case, replace one or all), double-click selects a word, triple-click a line, Shift+click extends the selection, Ctrl+←/→ jump by word, Ctrl+Home/End.
- Text editor: **live reload** — a file changed on disk reloads by itself; with unsaved edits a warning bar offers **Show conflicts** (both versions marked inline), **Use disk version** or **Keep mine**.
- Scrollbars in the editor, Hex Viewer, Detected Apps, Run app here, the App Launcher form, the App Host window list and the browser's History / Bookmarks / Cookies panels.
- Files follows the folder live: changes made elsewhere show up on their own.
- Linux test container (`tests/linux`) that builds with WebKitGTK and ALSA, runs the tests and drives the real binary end to end.
- Manually run **Release** workflow that builds Windows, Linux and macOS and publishes a GitHub release.

### Changed
- Every built-in app's actions live in visible buttons and menus (shared toolkit), with keys as accelerators only.
- Narrow file lists drop the date, then the size column, so names stay readable.
- Ctrl+H in the editor now opens Replace; syntax colors stay on the toolbar and View menu.
- The mouse wheel in the editor scrolls the view instead of moving the cursor.

### Fixed
- Library warnings (GTK, WebKit) no longer print over the desktop on Linux and macOS; they go to `.texttop_stderr.log`.
- git can no longer stop on a password prompt drawn over the desktop.
- Clicking in the editor after scrolling sideways put the cursor in the wrong column.

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
