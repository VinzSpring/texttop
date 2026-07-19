# texttop

A colorful desktop environment that lives entirely in your terminal — windows,
taskbar, start menu, wallpaper, and a set of built-in apps, rendered with
[ratatui](https://ratatui.rs). Runs on **Windows, Linux, and macOS**. Fully
self-contained: no external programs required — the browser/video engine uses
the platform's built-in webview (WebView2 / WebKitGTK / WKWebView).

```
cargo run --release            # open the desktop
cargo run --release -- <file>  # open the desktop with a file
```

Use a truecolor terminal with mouse support (Windows Terminal, iTerm2,
Terminal.app, GNOME Terminal, Konsole, kitty, alacritty, …) and make the
window big.

## Building

**Windows** — just `cargo build --release`. WebView2 ships with Windows 10/11.

**macOS** (13+)
```
xcode-select --install          # once, for Swift/WebKit tooling
cargo build --release
```

**Ubuntu / Debian**
```
sudo apt install build-essential pkg-config libasound2-dev libwebkit2gtk-4.1-dev
cargo build --release
```

**Fedora**
```
sudo dnf install gcc pkg-config alsa-lib-devel webkit2gtk4.1-devel
cargo build --release
```

Minimal build without audio / web engine (no system deps at all):
```
cargo build --release --no-default-features
```
The web/video engine needs a graphical session on Linux (X11/Wayland);
on headless boxes use `xvfb-run`.

## Apps

| App | What it does |
|-----|--------------|
| 🗂 Files | Browse drives/folders with filter, rename, delete, mkdir, copy/cut/paste, sort, preview, bookmarks; Enter opens with the right app; F3 hex · F4 edit · Ctrl+T terminal here |
| 🌐 Web | **Real graphical browser** (Edge / WebKit): JS, CSS, media — with **hybrid rendering**: pixels for images/layout, and the page's text extracted and drawn as crisp terminal characters (F3 toggles). F4 for URL, click links, type into forms, F9/F10 zoom |
| 🖥 Terminal | Full shell (`portable-pty` + vt100) inside a desktop window |
| 🧩 Programs | Run executables / `.app` bundles: terminal, args form, or detached |
| 🎵 Audio | Plays mp3/wav/flac/ogg (rodio) with visualizer, seek + volume |
| 🖼 Images | Quadrant-pixel truecolor rendering (2×2 pixels per cell) |
| 🎬 Video | Engine-based playback **with sound** — any format the platform webview plays |
| 📝 Editor | Text editor with line numbers and syntax highlighting (Rust, Python, JS/TS, C/C++, Go, Java, JSON, TOML, YAML, HTML, CSS, SQL, Shell, Markdown, …). Ctrl+S saves, Ctrl+H toggles highlight |
| 🧮 Calculator | Expression calculator (`+ − * / ^ %` and parentheses) with history |
| ⏱ Clock | Live clock, stopwatch, and countdown timer |
| 📅 Calendar | Month calendar — jump by day/week/month/year |
| 📌 Notes | Sticky notepad persisted to `~/.texttop_notes.txt` |
| 🔢 Hex | Hex/ASCII dump viewer (Files → F3) |
| 💻 System | OS/CPU info, special folders, environment snapshot |

## Desktop controls

- **F1** start menu · **F6** cycle windows · **F11** maximize
- **Ctrl+W** close window · **Ctrl+arrows** move window · **Ctrl+Q** quit
- Mouse: drag title bars, drag the `◢` corner to resize, `─ □ ✕` buttons,
  taskbar buttons to focus/minimize, double-click desktop icons
- Start menu → Wallpaper cycles color themes

## Architecture notes

- `src/engine/` — off-screen webview per platform (WebView2 on Windows,
  WebKitGTK on Linux, WKWebView on macOS) on its own thread; captures
  frames, decodes to RGB, shares them with the TUI. Input is forwarded as
  native mouse messages (Windows) or simulated pointer events (Linux/macOS),
  plus typed text via JavaScript. A periodic in-page script extracts every
  visible word's rect/color/weight (hiding pixel text from captures), and
  the TUI overlays them as real terminal characters — readable text at any
  zoom, while images/video stay graphical.
- `src/gfx.rs` — quadrant-block renderer: each cell carries a 2×2 pixel
  pattern with a best two-color fit (16 glyphs ` ▘▝▀▖▌▞▛▗▚▐▜▄▙▟█`).
- `src/apps/terminal.rs` — PTY via portable-pty, screen state via vt100.
- `src/apps/launcher.rs` — `--help` text parser (options, subcommands) that
  recurses into `prog <sub> --help` when you select a subcommand.
