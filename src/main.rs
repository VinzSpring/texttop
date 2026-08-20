mod app;
mod apps;
mod engine;
mod gfx;

use std::io::stdout;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
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
    execute!(stdout(), EnableMouseCapture)?;
    let res = run(&mut terminal);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    res
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
            // On terminals that don't handle 24-bit colour (e.g. Apple_Terminal
            // mangles \e[48;2;R;G;Bm — it reads the B byte as a bare SGR code),
            // remap every Color::Rgb cell in the buffer to the nearest xterm-256
            // index.  This single pass covers every app in the codebase.
            if !gfx::truecolor() {
                let area = f.area();
                let buf = f.buffer_mut();
                for y in area.top()..area.bottom() {
                    for x in area.left()..area.right() {
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_fg(gfx::adapt(cell.fg));
                            cell.set_bg(gfx::adapt(cell.bg));
                        }
                    }
                }
            }
        })?;

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            // drain every queued event before redrawing so bursts (mouse
            // drags, key repeat) cost one frame instead of one frame each
            loop {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        if k.code == KeyCode::Char('q')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            desktop.save_session();
                            return Ok(());
                        }
                        desktop.on_key(k);
                    }
                    Event::Mouse(m) => desktop.on_mouse(m),
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
