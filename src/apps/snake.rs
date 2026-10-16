//! Snake — square-ish cells (2 terminal columns each), speeds up as you eat.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;

use super::ui::{Controls, Item, Look};
use super::{Action, AppWindow};
use crate::gfx;

const FIELD_BG: Color = Color::Rgb(8, 10, 16);

#[derive(Clone, Copy, PartialEq)]
enum Cmd {
    StartPause,
    NewGame,
    Turn(i16, i16),
    /// A click on the field steers toward it.
    Field,
}

#[derive(PartialEq)]
enum Phase {
    /// Waiting for the first arrow key.
    Ready,
    Running,
    Paused,
    Dead,
}

pub struct Snake {
    body: VecDeque<(i16, i16)>, // head at front
    dir: (i16, i16),
    /// Buffered turns so quick double-taps register on successive moves.
    pending: VecDeque<(i16, i16)>,
    food: (i16, i16),
    grid: (i16, i16),
    phase: Phase,
    score: u32,
    best: u32,
    ticks: u32,
    rng: u64,
    ui: Controls<Cmd>,
    field: Rect,
}

impl Snake {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e3779b9)
            | 1;
        let mut s = Snake {
            body: VecDeque::new(),
            dir: (1, 0),
            pending: VecDeque::new(),
            food: (0, 0),
            grid: (0, 0),
            phase: Phase::Ready,
            score: 0,
            best: 0,
            ticks: 0,
            rng: seed,
            ui: Controls::new(),
            field: Rect::default(),
        };
        s.reset_round((30, 20));
        s
    }

    fn reset_round(&mut self, grid: (i16, i16)) {
        self.grid = grid;
        self.body.clear();
        let cx = grid.0 / 2;
        let cy = grid.1 / 2;
        for i in 0..3 {
            self.body.push_back((cx - i, cy));
        }
        self.dir = (1, 0);
        self.pending.clear();
        self.score = 0;
        self.ticks = 0;
        self.phase = Phase::Ready;
        self.spawn_food();
    }

    fn spawn_food(&mut self) {
        let (gw, gh) = self.grid;
        if gw <= 0 || gh <= 0 {
            return;
        }
        for _ in 0..512 {
            let x = (super::xorshift(&mut self.rng) % gw as u32) as i16;
            let y = (super::xorshift(&mut self.rng) % gh as u32) as i16;
            if !self.body.contains(&(x, y)) {
                self.food = (x, y);
                return;
            }
        }
    }

    fn turn(&mut self, d: (i16, i16)) {
        if self.phase == Phase::Ready {
            self.phase = Phase::Running;
        }
        if self.phase == Phase::Paused {
            self.phase = Phase::Running;
        }
        let last = self.pending.back().copied().unwrap_or(self.dir);
        // ignore reversals and duplicates
        if (d.0 + last.0, d.1 + last.1) == (0, 0) || d == last {
            return;
        }
        if self.pending.len() < 2 {
            self.pending.push_back(d);
        }
    }

    fn step(&mut self) {
        if let Some(d) = self.pending.pop_front() {
            self.dir = d;
        }
        let head = self.body.front().copied().unwrap_or((0, 0));
        let next = (head.0 + self.dir.0, head.1 + self.dir.1);
        let (gw, gh) = self.grid;
        let hit_wall = next.0 < 0 || next.1 < 0 || next.0 >= gw || next.1 >= gh;
        // tail cell vacates this move unless we eat, so ignore it in the check
        let eats = next == self.food;
        let hit_self = self
            .body
            .iter()
            .take(self.body.len() - usize::from(!eats))
            .any(|&c| c == next);
        if hit_wall || hit_self {
            self.phase = Phase::Dead;
            self.best = self.best.max(self.score);
            return;
        }
        self.body.push_front(next);
        if eats {
            self.score += 1;
            self.spawn_food();
        } else {
            self.body.pop_back();
        }
    }

    fn start_pause(&mut self) {
        self.phase = match self.phase {
            Phase::Running => Phase::Paused,
            Phase::Paused | Phase::Ready => Phase::Running,
            Phase::Dead => {
                self.new_game();
                Phase::Running
            }
        };
    }

    fn new_game(&mut self) {
        let grid = self.grid;
        let best = self.best.max(self.score);
        self.reset_round(grid);
        self.best = best;
    }

    /// Turn toward a clicked cell: perpendicular to the current heading, to
    /// whichever side the click is on.
    fn steer_to(&mut self, col: u16, row: u16) {
        let Some(&(hx, hy)) = self.body.front() else { return };
        let tx = (col.saturating_sub(self.field.x) / 2) as i16;
        let ty = row.saturating_sub(self.field.y) as i16;
        let (dx, dy) = (tx - hx, ty - hy);
        let heading = self.pending.back().copied().unwrap_or(self.dir);
        let d = if self.phase == Phase::Ready {
            // first move: go wherever the click points most
            if dx.abs() >= dy.abs() { (dx.signum(), 0) } else { (0, dy.signum()) }
        } else if heading.0 != 0 {
            (0, dy.signum())
        } else {
            (dx.signum(), 0)
        };
        if d != (0, 0) {
            self.turn(d);
        }
    }

    /// Ticks between moves: 150 ms at start, down to 75 ms.
    fn interval(&self) -> u32 {
        6u32.saturating_sub(self.score / 8).max(3)
    }

    fn overlay(&self, f: &mut Frame, field: Rect, lines: &[&str]) {
        let w = (lines.iter().map(|l| l.chars().count()).max().unwrap_or(10) + 4) as u16;
        let h = lines.len() as u16 + 2;
        if field.width < w || field.height < h {
            return;
        }
        let x = field.x + (field.width - w) / 2;
        let y = field.y + (field.height - h) / 2;
        gfx::fill(f.buffer_mut(), Rect::new(x, y, w, h), Color::Rgb(16, 19, 32));
        let buf = f.buffer_mut();
        for (i, line) in lines.iter().enumerate() {
            let lw = line.chars().count() as u16;
            buf.set_string(
                x + (w - lw) / 2,
                y + 1 + i as u16,
                *line,
                Style::default()
                    .fg(Color::Rgb(150, 200, 255))
                    .bg(Color::Rgb(16, 19, 32))
                    .add_modifier(Modifier::BOLD),
            );
        }
    }
}

impl AppWindow for Snake {
    fn title(&self) -> String {
        "Snake".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 5 || area.width < 16 {
            return;
        }
        let field = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        self.field = field;
        let gw = (field.width / 2) as i16;
        let gh = field.height as i16;
        if self.grid != (gw, gh) {
            let best = self.best.max(self.score);
            self.reset_round((gw, gh));
            self.best = best;
        }

        gfx::fill(f.buffer_mut(), field, FIELD_BG);
        let buf = f.buffer_mut();
        let cell = |x: i16, y: i16| (field.x + (x as u16) * 2, field.y + y as u16);

        // food
        let (fx, fy) = cell(self.food.0, self.food.1);
        for dx in 0..2 {
            if let Some(c) = buf.cell_mut((fx + dx, fy)) {
                c.set_symbol(if dx == 0 { "◖" } else { "◗" });
                c.set_fg(Color::Rgb(255, 110, 110));
                c.set_bg(FIELD_BG);
            }
        }

        // snake body, head brightest
        let n = self.body.len().max(1);
        for (i, &(x, y)) in self.body.iter().enumerate() {
            let t = i as f32 / n as f32;
            let g = (
                gfx::lerp(120, 40, t),
                gfx::lerp(230, 130, t),
                gfx::lerp(160, 90, t),
            );
            let (cx, cy) = cell(x, y);
            for dx in 0..2 {
                if let Some(c) = buf.cell_mut((cx + dx, cy)) {
                    c.set_symbol(" ");
                    c.set_bg(Color::Rgb(g.0, g.1, g.2));
                }
            }
        }

        match self.phase {
            Phase::Ready => self.overlay(
                f,
                field,
                &["🐍 S N A K E", "", "click where to go, or press Start", "click beside the head to turn"],
            ),
            Phase::Paused => self.overlay(f, field, &["paused"]),
            Phase::Dead => self.overlay(
                f,
                field,
                &[
                    "game over",
                    "",
                    &format!("score {}  ·  best {}", self.score, self.best),
                ][..]
                    .as_ref(),
            ),
            Phase::Running => {}
        }

        self.ui.begin();
        self.ui.add(field, Cmd::Field);
        let start = match self.phase {
            Phase::Running => ("❚❚ Pause", Look::Plain),
            Phase::Paused => ("▶ Resume", Look::Primary),
            Phase::Ready => ("▶ Start", Look::Primary),
            Phase::Dead => ("▶ Play again", Look::Primary),
        };
        let steer = if self.phase == Phase::Dead { Look::Disabled } else { Look::Plain };
        self.ui.toolbar(
            f.buffer_mut(),
            Rect::new(area.x, area.y, area.width, 1),
            &[
                Item::with(start.0, Cmd::StartPause, start.1),
                Item::btn("✦ New game", Cmd::NewGame),
                Item::Sep,
                Item::with("◀", Cmd::Turn(-1, 0), steer),
                Item::with("▲", Cmd::Turn(0, -1), steer),
                Item::with("▼", Cmd::Turn(0, 1), steer),
                Item::with("▶", Cmd::Turn(1, 0), steer),
            ],
            &[Item::Label(
                format!("score {}  best {}", self.score, self.best.max(self.score)),
                Color::Rgb(220, 224, 238),
            )],
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Up | KeyCode::Char('w') => self.turn((0, -1)),
            KeyCode::Down | KeyCode::Char('s') => self.turn((0, 1)),
            KeyCode::Left | KeyCode::Char('a') => self.turn((-1, 0)),
            KeyCode::Right | KeyCode::Char('d') => self.turn((1, 0)),
            KeyCode::Char(' ') if self.phase != Phase::Dead => self.start_pause(),
            KeyCode::Char('r') => self.new_game(),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        match self.ui.route(&m) {
            Some(Some(Cmd::StartPause)) => self.start_pause(),
            Some(Some(Cmd::NewGame)) => self.new_game(),
            Some(Some(Cmd::Turn(dx, dy))) if self.phase != Phase::Dead => self.turn((dx, dy)),
            Some(Some(Cmd::Field)) if self.phase != Phase::Dead => self.steer_to(m.column, m.row),
            _ => {}
        }
        Action::None
    }

    fn on_tick(&mut self) {
        if self.phase != Phase::Running {
            return;
        }
        self.ticks += 1;
        if self.ticks >= self.interval() {
            self.ticks = 0;
            self.step();
        }
    }
}
