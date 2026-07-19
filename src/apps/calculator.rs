use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Action, AppWindow};

pub struct Calculator {
    expr: String,
    result: String,
    history: Vec<String>,
    scroll: usize,
}

impl Calculator {
    pub fn new() -> Self {
        Calculator {
            expr: String::new(),
            result: String::from("0"),
            history: Vec::new(),
            scroll: 0,
        }
    }

    fn evaluate(&mut self) {
        let raw = self.expr.trim();
        if raw.is_empty() {
            return;
        }
        match eval_expr(raw) {
            Ok(v) => {
                let formatted = format_num(v);
                self.history.push(format!("{raw} = {formatted}"));
                if self.history.len() > 40 {
                    self.history.remove(0);
                }
                self.result = formatted.clone();
                self.expr = formatted;
            }
            Err(e) => self.result = format!("✗ {e}"),
        }
    }
}

impl AppWindow for Calculator {
    fn title(&self) -> String {
        "Calculator".into()
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        if area.height < 4 {
            return;
        }
        let accent = Style::default()
            .fg(Color::Rgb(0, 210, 180))
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::Rgb(130, 140, 165));
        let val = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);

        let mut lines = vec![
            Line::from(Span::styled("  + − × ÷  ·  ( )  ·  ^  ·  %", dim)),
            Line::default(),
            Line::from(vec![
                Span::styled("  ▶ ", accent),
                Span::styled(
                    format!("{}{}", self.expr, if focused { "█" } else { "" }),
                    val,
                ),
            ]),
            Line::from(Span::styled(format!("  = {}", self.result), accent)),
            Line::default(),
            Line::from(Span::styled("  History", accent)),
        ];

        let hist_h = area.height.saturating_sub(8) as usize;
        let start = self.history.len().saturating_sub(hist_h + self.scroll);
        let end = self.history.len().saturating_sub(self.scroll);
        for item in self.history.get(start..end).into_iter().flatten() {
            lines.push(Line::from(Span::styled(
                format!("  {item}"),
                Style::default().fg(Color::Rgb(180, 190, 210)),
            )));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  Enter =  ·  Esc/C clear  ·  Backspace  ·  Ctrl+L clear hist",
            dim,
        )));

        f.render_widget(Paragraph::new(lines), area);
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => {
                self.expr.clear();
                self.result = "0".into();
            }
            KeyCode::Char('c' | 'C') if !ctrl => {
                self.expr.clear();
                self.result = "0".into();
            }
            KeyCode::Char('l') if ctrl => {
                self.history.clear();
                self.scroll = 0;
            }
            KeyCode::Enter => self.evaluate(),
            KeyCode::Backspace => {
                self.expr.pop();
            }
            KeyCode::Up => {
                self.scroll = (self.scroll + 1).min(self.history.len());
            }
            KeyCode::Down => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            KeyCode::Char(c) if !ctrl => {
                if "0123456789+-*/()^.% ".contains(c) || c == 'e' || c == 'E' {
                    let mapped = match c {
                        '*' => '×',
                        '/' => '÷',
                        _ => c,
                    };
                    // store ASCII ops internally
                    let store = match c {
                        '×' => '*',
                        '÷' => '/',
                        _ => c,
                    };
                    let _ = mapped;
                    self.expr.push(store);
                }
            }
            _ => {}
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
