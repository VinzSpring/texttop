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

fn main() -> Result<()> {
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

    loop {
        terminal.draw(|f| desktop.render(f))?;

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
