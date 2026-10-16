mod app;
mod apps;
mod clipboard;
mod engine;
mod gfx;
#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod snapshot;

use std::io::stdout;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;

#[cfg(all(target_os = "macos", feature = "web-engine"))]
fn main() -> Result<()> {
    // WKWebView work is funneled to the process main thread via
    // DispatchQueue.main; those blocks only run while the main thread pumps
    // the CFRunLoop. Dedicate the main thread to that and run the TUI on a
    // worker thread — otherwise every WebKit call stalls until the TUI loop's
    // next tick (~25ms each, dozens per frame = unusably slow).
    let tui = std::thread::spawn(tui_main);
    while !tui.is_finished() {
        let t = Instant::now();
        webkit::pump_run_loop(0.02);
        // run(mode:before:) returns immediately when no sources are attached
        // (e.g. before the first browser window opens) — avoid a hot spin.
        if t.elapsed() < Duration::from_millis(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    tui.join().expect("TUI thread panicked")
}

#[cfg(not(all(target_os = "macos", feature = "web-engine")))]
fn main() -> Result<()> {
    tui_main()
}

fn tui_main() -> Result<()> {
    let mut terminal = ratatui::init();
    #[cfg(unix)]
    quiet_stderr::start();
    execute!(stdout(), EnableMouseCapture)?;
    // Without bracketed paste the terminal replays pasted text as ordinary key
    // events, so a pasted path fires whatever single-key shortcuts its letters
    // happen to be bound to. Ask for it as one event instead; terminals that do
    // not support it simply ignore the sequence.
    let _ = execute!(stdout(), EnableBracketedPaste);
    let res = run(&mut terminal);
    let _ = execute!(stdout(), DisableBracketedPaste);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    #[cfg(unix)]
    quiet_stderr::stop();
    res
}

/// GTK, WebKit and other libraries print warnings straight to stderr, which
/// lands on top of the desktop and garbles it. While the TUI is up, stderr
/// goes to `<state home>/.texttop_stderr.log` instead; a panic puts it back
/// first so its message still reaches the screen.
#[cfg(unix)]
mod quiet_stderr {
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicI32, Ordering};

    static SAVED: AtomicI32 = AtomicI32::new(-1);

    pub fn start() {
        let path = crate::apps::state_home().join(".texttop_stderr.log");
        let Ok(log) = std::fs::File::create(path) else {
            return;
        };
        // SAFETY: plain fd juggling on fd 2; `log` stays open through the dup.
        let saved = unsafe { libc::dup(2) };
        if saved < 0 || unsafe { libc::dup2(log.as_raw_fd(), 2) } < 0 {
            return;
        }
        SAVED.store(saved, Ordering::SeqCst);
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            stop();
            prev(info);
        }));
    }

    pub fn stop() {
        let saved = SAVED.swap(-1, Ordering::SeqCst);
        if saved >= 0 {
            // SAFETY: `saved` is the duplicate of the original stderr.
            unsafe {
                libc::dup2(saved, 2);
                libc::close(saved);
            }
        }
    }
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let mut desktop = app::Desktop::new();
    if let Some(arg) = std::env::args().nth(1) {
        let p = std::path::PathBuf::from(&arg);
        if p.exists() {
            desktop.open_path(p);
        }
    }
    let tick = Duration::from_millis(25);
    let mut last_tick = Instant::now();

    // Ensure the screen starts completely blank.
    terminal.clear()?;

    loop {
        terminal.draw(|f| {
            desktop.render(f);
            // Apple_Terminal mangles \e[48;2;R;G;Bm — it reads the B byte as a
            // bare SGR code — so fold 24-bit colors down to the 256 palette
            // once the frame is complete.
            let area = f.area();
            gfx::adapt_buffer(f.buffer_mut(), area, gfx::truecolor());
        })?;

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            // drain every queued event before redrawing so bursts (mouse
            // drags, key repeat) cost one frame instead of one frame each
            loop {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        let k = altgr_as_text(k);
                        if k.code == KeyCode::Char('q')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            desktop.save_session();
                            return Ok(());
                        }
                        desktop.on_key(k);
                    }
                    Event::Mouse(m) => desktop.on_mouse(m),
                    Event::Paste(text) => desktop.on_paste(&text),
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        if last_tick.elapsed() >= tick {
            desktop.on_tick();
            last_tick = Instant::now();
        }
        if desktop.should_quit {
            return Ok(());
        }
    }
}

/// Windows reports AltGr as Ctrl+Alt, so on layouts like German the keys for
/// `\ @ { } [ ] | ~ €` arrive looking like shortcuts and every text field
/// dropped them. A Ctrl+Alt chord that produced a symbol rather than a letter
/// or digit is typed text: strip the two modifiers so apps see a plain char.
fn altgr_as_text(mut k: KeyEvent) -> KeyEvent {
    let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;
    if let KeyCode::Char(c) = k.code {
        if k.modifiers.contains(altgr) && !c.is_ascii_alphanumeric() && !c.is_control() {
            k.modifiers.remove(altgr);
        }
    }
    k
}

#[cfg(test)]
mod altgr_tests {
    use super::*;

    #[test]
    fn altgr_symbols_become_text_but_chords_stay() {
        let ev = |c, m| altgr_as_text(KeyEvent::new(KeyCode::Char(c), m));
        let ca = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert_eq!(ev('\\', ca).modifiers, KeyModifiers::NONE);
        assert_eq!(ev('{', ca).modifiers, KeyModifiers::NONE);
        assert_eq!(ev('€', ca).modifiers, KeyModifiers::NONE);
        // real Ctrl+Alt shortcuts on letters and digits are untouched
        assert_eq!(ev('t', ca).modifiers, ca);
        assert_eq!(ev('7', ca).modifiers, ca);
        // plain Ctrl chords are untouched
        assert_eq!(ev('[', KeyModifiers::CONTROL).modifiers, KeyModifiers::CONTROL);
    }
}
