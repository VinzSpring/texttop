use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;

use super::{Action, AppWindow};

// macOS calculator palette
/// Characters the expression evaluator understands.
const EXPR_CHARS: &str = "0123456789+-*/()^.% eE";

const BODY_BG: Color = Color::Rgb(38, 37, 42);
const BTN_DIGIT: Color = Color::Rgb(89, 88, 94);
const BTN_FUNC: Color = Color::Rgb(63, 62, 68);
const BTN_OP: Color = Color::Rgb(255, 159, 10);

#[derive(Clone, Copy, PartialEq)]
enum Key {
    Ch(char),
    Clear,
    Neg,
    Percent,
    Eq,
}

pub struct Calculator {
    expr: String,
    result: String,
    history: Vec<String>,
    buttons: Vec<(Rect, Key)>,
}

impl Calculator {
    pub fn new() -> Self {
        Calculator {
            expr: String::new(),
            result: String::from("0"),
            history: Vec::new(),
            buttons: Vec::new(),
        }
    }

    fn evaluate(&mut self) {
        let raw = self.expr.trim().to_string();
        if raw.is_empty() {
            return;
        }
        match eval_expr(&raw) {
            Ok(v) => {
                let formatted = format_num(v);
                if raw != formatted {
                    self.history.push(format!("{} = {}", pretty_ops(&raw), formatted));
                    if self.history.len() > 32 {
                        self.history.remove(0);
                    }
                }
                self.result = formatted.clone();
                self.expr = formatted;
            }
            Err(_) => {
                self.result = "Error".into();
                self.expr.clear();
            }
        }
    }

    fn clear(&mut self) {
        self.expr.clear();
        self.result = "0".into();
    }

    fn percent(&mut self) {
        if self.expr.trim().is_empty() {
            return;
        }
        match eval_expr(&format!("({})/100", self.expr)) {
            Ok(v) => {
                let formatted = format_num(v);
                self.result = formatted.clone();
                self.expr = formatted;
            }
            Err(_) => {
                self.result = "Error".into();
                self.expr.clear();
            }
        }
    }

    fn negate(&mut self) {
        if let Some(rest) = self.expr.strip_prefix('-') {
            self.expr = rest.to_string();
        } else if !self.expr.is_empty() {
            self.expr.insert(0, '-');
        }
    }

    fn press(&mut self, k: Key) {
        match k {
            Key::Ch(c) => self.expr.push(c),
            Key::Clear => self.clear(),
            Key::Neg => self.negate(),
            Key::Percent => self.percent(),
            Key::Eq => self.evaluate(),
        }
    }

    /// What the big display shows: the live entry, or the last result.
    fn display_text(&self) -> String {
        let src = if self.expr.is_empty() {
            &self.result
        } else {
            &self.expr
        };
        pretty_ops(src)
    }
}

fn pretty_ops(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '*' => '×',
            '/' => '÷',
            '-' => '−',
            _ => c,
        })
        .collect()
}

impl AppWindow for Calculator {
    fn title(&self) -> String {
        "Calculator".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        self.buttons.clear();
        if area.width < 11 || area.height < 3 {
            return;
        }
        let buf = f.buffer_mut();
        let blank = " ".repeat(area.width as usize);
        for y in area.top()..area.bottom() {
            buf.set_string(area.x, y, &blank, Style::default().bg(BODY_BG));
        }

        // ------- layout: display strip on top, 5 rows x 4 cols of buttons
        let hgap: u16 = 1;
        let vgap: u16 = u16::from(area.height >= 17);
        let btn_h = ((area.height.saturating_sub(2 + vgap * 4)) / 5).clamp(1, 3);
        let grid_h = btn_h * 5 + vgap * 4;
        let display_h = area.height.saturating_sub(grid_h).max(2);
        let col_w = (area.width - 3 * hgap) / 4;
        let used = col_w * 4 + 3 * hgap;
        let left = area.x + (area.width - used) / 2;

        // ------- history tape above the readout (when the window is tall)
        if display_h > 3 {
            let tape_rows = (display_h - 3) as usize;
            let dim = Style::default().fg(Color::Rgb(140, 140, 150)).bg(BODY_BG);
            for (row, entry) in self.history.iter().rev().take(tape_rows).enumerate() {
                let y = area.y + display_h - 4 - row as u16;
                let max = used.saturating_sub(2) as usize;
                let shown: String = if entry.chars().count() > max {
                    let chars: Vec<char> = entry.chars().collect();
                    chars[chars.len() - max..].iter().collect()
                } else {
                    entry.clone()
                };
                let tw = shown.chars().count() as u16;
                buf.set_string(left + used - tw - 1, y, &shown, dim);
            }
        }

        // ------- display: big right-aligned readout
        let text = self.display_text();
        let max = used.saturating_sub(2) as usize;
        let chars: Vec<char> = text.chars().collect();
        let shown: String = if chars.len() > max {
            chars[chars.len() - max..].iter().collect()
        } else {
            text
        };
        let tw = shown.chars().count() as u16;
        buf.set_string(
            left + used - tw - 1,
            area.y + display_h - 2,
            &shown,
            Style::default()
                .fg(Color::White)
                .bg(BODY_BG)
                .add_modifier(Modifier::BOLD),
        );

        // ------- button grid
        let ac: &str = if self.expr.is_empty() { "AC" } else { "C" };
        let rows: [&[(&str, Key, u16)]; 5] = [
            &[
                (ac, Key::Clear, 1),
                ("±", Key::Neg, 1),
                ("%", Key::Percent, 1),
                ("÷", Key::Ch('/'), 1),
            ],
            &[
                ("7", Key::Ch('7'), 1),
                ("8", Key::Ch('8'), 1),
                ("9", Key::Ch('9'), 1),
                ("×", Key::Ch('*'), 1),
            ],
            &[
                ("4", Key::Ch('4'), 1),
                ("5", Key::Ch('5'), 1),
                ("6", Key::Ch('6'), 1),
                ("−", Key::Ch('-'), 1),
            ],
            &[
                ("1", Key::Ch('1'), 1),
                ("2", Key::Ch('2'), 1),
                ("3", Key::Ch('3'), 1),
                ("+", Key::Ch('+'), 1),
            ],
            &[
                ("0", Key::Ch('0'), 2),
                (".", Key::Ch('.'), 1),
                ("=", Key::Eq, 1),
            ],
        ];

        let mut y = area.y + display_h;
        for row in rows {
            let mut x = left;
            for &(label, key, span) in row {
                let bw = col_w * span + hgap * (span - 1);
                let bg = match key {
                    Key::Clear | Key::Neg | Key::Percent => BTN_FUNC,
                    Key::Eq | Key::Ch('+' | '-' | '*' | '/') => BTN_OP,
                    _ => BTN_DIGIT,
                };
                let style = Style::default()
                    .fg(Color::White)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD);
                let pad = " ".repeat(bw as usize);
                for yy in y..y + btn_h {
                    buf.set_string(x, yy, &pad, style);
                }
                let lw = label.chars().count() as u16;
                buf.set_string(x + (bw.saturating_sub(lw)) / 2, y + btn_h / 2, label, style);
                self.buttons.push((Rect::new(x, y, bw, btn_h), key));
                x += bw + hgap;
            }
            y += btn_h + vgap;
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => self.clear(),
            KeyCode::Char('c' | 'C') if !ctrl => self.clear(),
            KeyCode::Enter => self.evaluate(),
            KeyCode::Backspace => {
                self.expr.pop();
            }
            KeyCode::Char('=') if !ctrl => self.evaluate(),
            KeyCode::Char(c) if !ctrl => {
                if EXPR_CHARS.contains(c) {
                    self.expr.push(c);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn on_paste(&mut self, text: &str) -> Action {
        // Same filter the keyboard uses, so a pasted expression cannot smuggle
        // in characters the evaluator does not understand.
        self.expr
            .extend(text.chars().filter(|c| EXPR_CHARS.contains(*c)));
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, _area: Rect) -> Action {
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            let hit = self.buttons.iter().find(|(r, _)| {
                m.column >= r.x
                    && m.column < r.x + r.width
                    && m.row >= r.y
                    && m.row < r.y + r.height
            });
            if let Some(&(_, key)) = hit {
                self.press(key);
            }
        }
        Action::None
    }
}

fn format_num(v: f64) -> String {
    if !v.is_finite() {
        return "∞".into();
    }
    if (v - v.round()).abs() < 1e-12 && v.abs() < 1e15 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.10}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Recursive-descent parser for + - * / ^ % and parentheses.
fn eval_expr(s: &str) -> Result<f64, String> {
    let tokens = tokenize(s)?;
    let mut p = Parser { tokens, i: 0 };
    let v = p.parse_expr()?;
    if p.i < p.tokens.len() {
        return Err("unexpected trailing input".into());
    }
    Ok(v)
}

#[derive(Clone)]
enum Tok {
    Num(f64),
    Op(char),
    LParen,
    RParen,
}

struct Parser {
    tokens: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.i)
    }
    fn bump(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.i).cloned();
        if t.is_some() {
            self.i += 1;
        }
        t
    }

    fn parse_expr(&mut self) -> Result<f64, String> {
        let mut v = self.parse_term()?;
        while let Some(Tok::Op(op)) = self.peek().cloned() {
            if op != '+' && op != '-' {
                break;
            }
            self.bump();
            let r = self.parse_term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Ok(v)
    }

    fn parse_term(&mut self) -> Result<f64, String> {
        let mut v = self.parse_power()?;
        while let Some(Tok::Op(op)) = self.peek().cloned() {
            if op != '*' && op != '/' && op != '%' {
                break;
            }
            self.bump();
            let r = self.parse_power()?;
            v = match op {
                '*' => v * r,
                '/' => {
                    if r == 0.0 {
                        return Err("division by zero".into());
                    }
                    v / r
                }
                '%' => v % r,
                _ => v,
            };
        }
        Ok(v)
    }

    fn parse_power(&mut self) -> Result<f64, String> {
        let base = self.parse_unary()?;
        if let Some(Tok::Op('^')) = self.peek().cloned() {
            self.bump();
            let exp = self.parse_power()?; // right-assoc
            Ok(base.powf(exp))
        } else {
            Ok(base)
        }
    }

    fn parse_unary(&mut self) -> Result<f64, String> {
        match self.peek().cloned() {
            Some(Tok::Op('+')) => {
                self.bump();
                self.parse_unary()
            }
            Some(Tok::Op('-')) => {
                self.bump();
                Ok(-self.parse_unary()?)
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<f64, String> {
        match self.bump() {
            Some(Tok::Num(n)) => Ok(n),
            Some(Tok::LParen) => {
                let v = self.parse_expr()?;
                match self.bump() {
                    Some(Tok::RParen) => Ok(v),
                    _ => Err("missing ')'".into()),
                }
            }
            _ => Err("expected number".into()),
        }
    }
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '+' | '-' | '*' | '/' | '^' | '%' | '×' | '÷' => {
                let op = match c {
                    '×' => '*',
                    '÷' => '/',
                    _ => c,
                };
                out.push(Tok::Op(op));
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '0'..='9' | '.' => {
                let start = i;
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    i += 1;
                    if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                        i += 1;
                    }
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let num: String = chars[start..i].iter().collect();
                let v: f64 = num.parse().map_err(|_| format!("bad number: {num}"))?;
                out.push(Tok::Num(v));
            }
            _ => return Err(format!("bad char '{c}'")),
        }
    }
    Ok(out)
}
