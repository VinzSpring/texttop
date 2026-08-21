//! 2048 — slide and merge tiles with the arrow keys.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};
use crate::gfx;

const STATUS_FG: Color = Color::Rgb(140, 150, 175);
const STATUS_BG: Color = Color::Rgb(18, 22, 36);
const BOARD_BG: Color = Color::Rgb(30, 28, 38);
const EMPTY_BG: Color = Color::Rgb(44, 42, 54);

pub struct Game2048 {
    grid: [[u32; 4]; 4],
    score: u32,
    best: u32,
    over: bool,
    won: bool,
    rng: u64,
}

fn tile_color(v: u32) -> (Color, Color) {
    let bg = match v {
        2 => Color::Rgb(66, 72, 94),
        4 => Color::Rgb(82, 90, 120),
        8 => Color::Rgb(242, 177, 121),
        16 => Color::Rgb(245, 149, 99),
        32 => Color::Rgb(246, 124, 95),
        64 => Color::Rgb(246, 94, 59),
        128 => Color::Rgb(237, 207, 114),
        256 => Color::Rgb(237, 204, 97),
        512 => Color::Rgb(237, 200, 80),
        1024 => Color::Rgb(237, 197, 63),
        2048 => Color::Rgb(237, 194, 46),
        _ => Color::Rgb(90, 60, 140),
    };
    let fg = if v <= 4 {
        Color::Rgb(220, 224, 238)
    } else if v <= 64 {
        Color::White
    } else {
        Color::Rgb(60, 50, 30)
    };
    (fg, bg)
}

impl Game2048 {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x2048_2048)
            | 1;
        let mut g = Game2048 {
            grid: [[0; 4]; 4],
            score: 0,
            best: 0,
            over: false,
            won: false,
            rng: seed,
        };
        g.spawn_tile();
        g.spawn_tile();
        g
    }

    fn restart(&mut self) {
        self.best = self.best.max(self.score);
        self.grid = [[0; 4]; 4];
        self.score = 0;
        self.over = false;
        self.won = false;
        self.spawn_tile();
        self.spawn_tile();
    }

    fn spawn_tile(&mut self) {
        let empty: Vec<(usize, usize)> = (0..4)
            .flat_map(|y| (0..4).map(move |x| (x, y)))
            .filter(|&(x, y)| self.grid[y][x] == 0)
            .collect();
        if empty.is_empty() {
            return;
        }
        let (x, y) = empty[(super::xorshift(&mut self.rng) as usize) % empty.len()];
        self.grid[y][x] = if super::xorshift(&mut self.rng) % 10 == 0 { 4 } else { 2 };
    }

    /// Slide+merge one row leftward; returns (new row, gained score, moved).
    fn slide(row: [u32; 4]) -> ([u32; 4], u32, bool) {
        let vals: Vec<u32> = row.iter().copied().filter(|&v| v != 0).collect();
        let mut out = Vec::with_capacity(4);
        let mut gained = 0;
        let mut i = 0;
        while i < vals.len() {
            if i + 1 < vals.len() && vals[i] == vals[i + 1] {
                out.push(vals[i] * 2);
                gained += vals[i] * 2;
                i += 2;
            } else {
                out.push(vals[i]);
                i += 1;
            }
        }
        let mut new = [0u32; 4];
        new[..out.len()].copy_from_slice(&out);
        let moved = new != row;
        (new, gained, moved)
    }

    fn shift(&mut self, dir: (i32, i32)) {
        if self.over {
            return;
        }
        let mut moved = false;
        let mut gained = 0;
        for i in 0..4 {
            // extract the lane in slide order
            let lane: [u32; 4] = std::array::from_fn(|j| match dir {
                (-1, 0) => self.grid[i][j],
                (1, 0) => self.grid[i][3 - j],
                (0, -1) => self.grid[j][i],
                _ => self.grid[3 - j][i],
            });
            let (new, g, m) = Self::slide(lane);
            gained += g;
            moved |= m;
            for (j, &v) in new.iter().enumerate() {
                match dir {
                    (-1, 0) => self.grid[i][j] = v,
                    (1, 0) => self.grid[i][3 - j] = v,
                    (0, -1) => self.grid[j][i] = v,
                    _ => self.grid[3 - j][i] = v,
                }
            }
        }
        if moved {
            self.score += gained;
            if !self.won && self.grid.iter().flatten().any(|&v| v >= 2048) {
                self.won = true;
            }
            self.spawn_tile();
            if self.is_stuck() {
                self.over = true;
                self.best = self.best.max(self.score);
            }
        }
    }

    fn is_stuck(&self) -> bool {
        for y in 0..4 {
            for x in 0..4 {
                let v = self.grid[y][x];
                if v == 0 {
                    return false;
                }
                if x + 1 < 4 && self.grid[y][x + 1] == v {
                    return false;
                }
                if y + 1 < 4 && self.grid[y + 1][x] == v {
                    return false;
                }
            }
        }
        true
    }
}

impl AppWindow for Game2048 {
    fn title(&self) -> String {
        "2048".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        if area.height < 6 || area.width < 24 {
            return;
        }
        gfx::fill(f.buffer_mut(), area, Color::Rgb(12, 14, 22));

        // tile geometry adapts to the window
        let avail = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
        let tile_w = ((avail.width.saturating_sub(5)) / 4).clamp(4, 9);
        let tile_h = ((avail.height.saturating_sub(5)) / 4).clamp(1, 3);
        let board_w = tile_w * 4 + 5;
        let board_h = tile_h * 4 + 5;
        let bx = avail.x + avail.width.saturating_sub(board_w) / 2;
        let by = avail.y + avail.height.saturating_sub(board_h) / 2;
        gfx::fill(f.buffer_mut(), Rect::new(bx, by, board_w, board_h), BOARD_BG);

        // header
        let head = format!(" 2048   score {}   best {}", self.score, self.best.max(self.score));
        f.buffer_mut().set_string(
            area.x + 1,
            area.y,
            &head,
            Style::default()
                .fg(Color::Rgb(220, 224, 238))
                .add_modifier(Modifier::BOLD),
        );

        let buf = f.buffer_mut();
        for y in 0..4u16 {
            for x in 0..4u16 {
                let v = self.grid[y as usize][x as usize];
                let (fg, bg) = if v == 0 {
                    (EMPTY_BG, EMPTY_BG)
                } else {
                    tile_color(v)
                };
                let tx = bx + 1 + x * (tile_w + 1);
                let ty = by + 1 + y * (tile_h + 1);
                for yy in ty..ty + tile_h {
                    for xx in tx..tx + tile_w {
                        if let Some(c) = buf.cell_mut((xx, yy)) {
                            c.set_symbol(" ");
                            c.set_bg(bg);
                        }
                    }
                }
                if v > 0 {
                    let label = v.to_string();
                    let lw = label.len() as u16;
                    if lw <= tile_w {
                        buf.set_string(
                            tx + (tile_w - lw) / 2,
                            ty + tile_h / 2,
                            &label,
                            Style::default()
                                .fg(fg)
                                .bg(bg)
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                }
            }
        }

        // end / win banner
        if self.over || self.won {
            let msg = if self.over {
                " game over — r restarts "
            } else {
                " 2048! keep going or r to restart "
            };
            let w = msg.chars().count() as u16;
            if w <= area.width {
                buf.set_string(
                    area.x + (area.width - w) / 2,
                    by + board_h.min(avail.height.saturating_sub(1)),
                    msg,
                    Style::default()
                        .fg(Color::White)
                        .bg(if self.over {
                            Color::Rgb(150, 40, 40)
                        } else {
                            Color::Rgb(30, 110, 60)
                        })
                        .add_modifier(Modifier::BOLD),
                );
            }
        }

        f.render_widget(
            Paragraph::new(Span::styled(
                " arrows/wasd slide · r restart · q close ",
                Style::default().fg(STATUS_FG).bg(STATUS_BG),
            ))
            .style(Style::default().bg(STATUS_BG)),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Left | KeyCode::Char('a') => self.shift((-1, 0)),
            KeyCode::Right | KeyCode::Char('d') => self.shift((1, 0)),
            KeyCode::Up | KeyCode::Char('w') => self.shift((0, -1)),
            KeyCode::Down | KeyCode::Char('s') => self.shift((0, 1)),
            KeyCode::Char('r') => self.restart(),
            KeyCode::Char('q') | KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }
}
