//! Minesweeper — mouse-first (left reveal · right flag), keyboard fallback.
//! Mines are placed after the first reveal so the opening click is always safe.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};
use crate::gfx;

const STATUS_FG: Color = Color::Rgb(140, 150, 175);
const STATUS_BG: Color = Color::Rgb(18, 22, 36);
const HIDDEN_BG: Color = Color::Rgb(46, 52, 74);
const OPEN_BG: Color = Color::Rgb(20, 24, 36);

const PRESETS: [(usize, usize, usize, &str); 3] = [
    (9, 9, 10, "easy"),
    (16, 12, 30, "medium"),
    (24, 16, 60, "hard"),
];

const NUM_COLORS: [Color; 8] = [
    Color::Rgb(100, 170, 255),
    Color::Rgb(110, 220, 140),
    Color::Rgb(255, 120, 120),
    Color::Rgb(180, 140, 255),
    Color::Rgb(255, 180, 90),
    Color::Rgb(90, 220, 220),
    Color::Rgb(235, 235, 240),
    Color::Rgb(160, 160, 175),
];

#[derive(PartialEq, Clone, Copy)]
enum State {
    Fresh, // board built, mines not yet placed
    Play,
    Won,
    Lost,
}

pub struct Minesweeper {
    w: usize,
    h: usize,
    mines: usize,
    preset: usize,
    mine: Vec<bool>,
    adj: Vec<u8>,
    open: Vec<bool>,
    flag: Vec<bool>,
    state: State,
    cursor: (usize, usize),
    started: Option<Instant>,
    final_secs: u64,
    /// Grid origin on screen from the last render, for mouse hit-testing.
    origin: (u16, u16),
    rng: u64,
}

impl Minesweeper {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xdeadbeef)
            | 1;
        let mut m = Minesweeper {
            w: 0,
            h: 0,
            mines: 0,
            preset: 1,
            mine: Vec::new(),
            adj: Vec::new(),
            open: Vec::new(),
            flag: Vec::new(),
            state: State::Fresh,
            cursor: (0, 0),
            started: None,
            final_secs: 0,
            origin: (0, 0),
            rng: seed,
        };
        m.new_game(1);
        m
    }

    fn new_game(&mut self, preset: usize) {
        let (w, h, mines, _) = PRESETS[preset.min(2)];
        self.preset = preset.min(2);
        self.w = w;
        self.h = h;
        self.mines = mines;
        let n = w * h;
        self.mine = vec![false; n];
        self.adj = vec![0; n];
        self.open = vec![false; n];
        self.flag = vec![false; n];
        self.state = State::Fresh;
        self.cursor = (w / 2, h / 2);
        self.started = None;
        self.final_secs = 0;
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }

    fn neighbors(&self, x: usize, y: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::with_capacity(8);
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && ny >= 0 && (nx as usize) < self.w && (ny as usize) < self.h {
                    out.push((nx as usize, ny as usize));
                }
            }
        }
        out
    }

    /// Lay mines, keeping the first-clicked cell and its ring clear.
    fn place_mines(&mut self, safe_x: usize, safe_y: usize) {
        let mut safe: Vec<usize> = vec![self.idx(safe_x, safe_y)];
        for (nx, ny) in self.neighbors(safe_x, safe_y) {
            safe.push(self.idx(nx, ny));
        }
        let n = self.w * self.h;
        let mut placed = 0;
        while placed < self.mines.min(n.saturating_sub(safe.len())) {
            let i = (super::xorshift(&mut self.rng) as usize) % n;
            if self.mine[i] || safe.contains(&i) {
                continue;
            }
            self.mine[i] = true;
            placed += 1;
        }
        for y in 0..self.h {
            for x in 0..self.w {
                let count = self
                    .neighbors(x, y)
                    .iter()
                    .filter(|&&(nx, ny)| self.mine[self.idx(nx, ny)])
                    .count();
                let i = self.idx(x, y);
                self.adj[i] = count as u8;
            }
        }
        self.state = State::Play;
        self.started = Some(Instant::now());
    }

    fn reveal(&mut self, x: usize, y: usize) {
        if self.state == State::Fresh {
            self.place_mines(x, y);
        }
        if self.state != State::Play {
            return;
        }
        let i = self.idx(x, y);
        if self.flag[i] {
            return;
        }
        if self.open[i] {
            self.chord(x, y);
            return;
        }
        if self.mine[i] {
            self.lose();
            return;
        }
        // flood-fill zeros
        let mut stack = vec![(x, y)];
        while let Some((cx, cy)) = stack.pop() {
            let ci = self.idx(cx, cy);
            if self.open[ci] || self.flag[ci] || self.mine[ci] {
                continue;
            }
            self.open[ci] = true;
            if self.adj[ci] == 0 {
                stack.extend(self.neighbors(cx, cy));
            }
        }
        self.check_win();
    }

    /// Click a satisfied number to open its remaining neighbors.
    fn chord(&mut self, x: usize, y: usize) {
        let i = self.idx(x, y);
        if self.adj[i] == 0 {
            return;
        }
        let flags = self
            .neighbors(x, y)
            .iter()
            .filter(|&&(nx, ny)| self.flag[self.idx(nx, ny)])
            .count() as u8;
        if flags != self.adj[i] {
            return;
        }
        for (nx, ny) in self.neighbors(x, y) {
            let ni = self.idx(nx, ny);
            if !self.open[ni] && !self.flag[ni] {
                if self.mine[ni] {
                    self.lose();
                    return;
                }
                self.reveal(nx, ny);
            }
        }
    }

    fn toggle_flag(&mut self, x: usize, y: usize) {
        if self.state != State::Play && self.state != State::Fresh {
            return;
        }
        let i = self.idx(x, y);
        if !self.open[i] {
            self.flag[i] = !self.flag[i];
        }
    }

    fn lose(&mut self) {
        self.state = State::Lost;
        self.final_secs = self.elapsed_secs();
        for i in 0..self.mine.len() {
            if self.mine[i] {
                self.open[i] = true;
            }
        }
    }

    fn check_win(&mut self) {
        let opened = self.open.iter().filter(|&&o| o).count();
        if opened == self.w * self.h - self.mines {
            self.state = State::Won;
            self.final_secs = self.elapsed_secs();
            for i in 0..self.mine.len() {
                if self.mine[i] {
                    self.flag[i] = true;
                }
            }
        }
    }

    fn elapsed_secs(&self) -> u64 {
        match self.state {
            State::Won | State::Lost => self.final_secs,
            _ => self
                .started
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0),
        }
    }

    fn flags_left(&self) -> i64 {
        self.mines as i64 - self.flag.iter().filter(|&&f| f).count() as i64
    }

    fn cell_at(&self, col: u16, row: u16) -> Option<(usize, usize)> {
        let (ox, oy) = self.origin;
        if col < ox || row < oy {
            return None;
        }
        let x = ((col - ox) / 2) as usize;
        let y = (row - oy) as usize;
        (x < self.w && y < self.h).then_some((x, y))
    }
}

impl AppWindow for Minesweeper {
    fn title(&self) -> String {
        "Minesweeper".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 6 || area.width < 20 {
            return;
        }
        gfx::fill(f.buffer_mut(), area, Color::Rgb(12, 14, 22));

        let grid_w = (self.w * 2) as u16;
        let grid_h = self.h as u16;
        let avail = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
        let ox = avail.x + avail.width.saturating_sub(grid_w) / 2;
        let oy = avail.y + avail.height.saturating_sub(grid_h) / 2;
        self.origin = (ox, oy);

        // header
        let face = match self.state {
            State::Won => "😎",
            State::Lost => "💥",
            _ => "🙂",
        };
        let header = format!(
            " {face}  💣 {:>2}   ⏱ {:>3}s   {}",
            self.flags_left(),
            self.elapsed_secs(),
            PRESETS[self.preset].3,
        );
        f.buffer_mut().set_string(
            area.x + 1,
            area.y,
            &header,
            Style::default()
                .fg(Color::Rgb(220, 224, 238))
                .add_modifier(Modifier::BOLD),
        );

        let buf = f.buffer_mut();
        for y in 0..self.h {
            for x in 0..self.w {
                let i = self.idx(x, y);
                let sx = ox + (x as u16) * 2;
                let sy = oy + y as u16;
                if sx + 1 >= area.x + area.width || sy >= area.y + area.height - 1 {
                    continue;
                }
                let is_cursor = self.cursor == (x, y)
                    && matches!(self.state, State::Play | State::Fresh);
                let (text, fg, bg): (String, Color, Color) = if self.open[i] {
                    if self.mine[i] {
                        ("✹ ".into(), Color::Rgb(255, 230, 120), Color::Rgb(150, 40, 40))
                    } else if self.adj[i] == 0 {
                        ("  ".into(), OPEN_BG, OPEN_BG)
                    } else {
                        (
                            format!("{} ", self.adj[i]),
                            NUM_COLORS[(self.adj[i] - 1) as usize],
                            OPEN_BG,
                        )
                    }
                } else if self.flag[i] {
                    ("⚑ ".into(), Color::Rgb(255, 120, 120), HIDDEN_BG)
                } else {
                    ("░░".into(), Color::Rgb(70, 78, 106), HIDDEN_BG)
                };
                let bg = if is_cursor { Color::Rgb(0, 110, 170) } else { bg };
                let mut chars = text.chars();
                for dx in 0..2u16 {
                    if let Some(c) = buf.cell_mut((sx + dx, sy)) {
                        let ch = chars.next().unwrap_or(' ');
                        c.set_symbol(&ch.to_string());
                        c.set_fg(fg);
                        c.set_bg(bg);
                    }
                }
            }
        }

        let msg = match self.state {
            State::Won => format!(" solved in {}s! 🎉 · r replay · 1/2/3 difficulty · q ", self.final_secs),
            State::Lost => " boom · r replay · 1/2/3 difficulty · q ".into(),
            _ => " click reveal · right-click flag · arrows+Enter/f · 1/2/3 difficulty · q ".into(),
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                msg,
                Style::default().fg(STATUS_FG).bg(STATUS_BG),
            ))
            .style(Style::default().bg(STATUS_BG)),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Up => self.cursor.1 = self.cursor.1.saturating_sub(1),
            KeyCode::Down => self.cursor.1 = (self.cursor.1 + 1).min(self.h - 1),
            KeyCode::Left => self.cursor.0 = self.cursor.0.saturating_sub(1),
            KeyCode::Right => self.cursor.0 = (self.cursor.0 + 1).min(self.w - 1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let (x, y) = self.cursor;
                self.reveal(x, y);
            }
            KeyCode::Char('f') => {
                let (x, y) = self.cursor;
                self.toggle_flag(x, y);
            }
            KeyCode::Char('r') => self.new_game(self.preset),
            KeyCode::Char('1') => self.new_game(0),
            KeyCode::Char('2') => self.new_game(1),
            KeyCode::Char('3') => self.new_game(2),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((x, y)) = self.cell_at(m.column, m.row) {
                    self.cursor = (x, y);
                    self.reveal(x, y);
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some((x, y)) = self.cell_at(m.column, m.row) {
                    self.cursor = (x, y);
                    self.toggle_flag(x, y);
                }
            }
            _ => {}
        }
        Action::None
    }
}
