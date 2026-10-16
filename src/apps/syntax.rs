//! Lightweight syntax highlighting for the built-in editor.
//! Tokenizes per line with a small language table — no external grammar packs.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Plain,
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Json,
    Toml,
    Yaml,
    Markdown,
    C,
    Cpp,
    Go,
    Java,
    Shell,
    Html,
    Css,
    Sql,
    Ini,
}

impl Lang {
    pub fn from_path(path: Option<&std::path::Path>) -> Self {
        let Some(path) = path else {
            return Lang::Plain;
        };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "makefile" | "dockerfile" | "cmakelists.txt" | ".gitignore" | ".env"
        ) {
            return match name.as_str() {
                "dockerfile" => Lang::Shell,
                ".env" | ".gitignore" => Lang::Ini,
                _ => Lang::Shell,
            };
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "rs" => Lang::Rust,
            "py" | "pyw" | "pyi" => Lang::Python,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "ts" | "tsx" | "mts" | "cts" => Lang::TypeScript,
            "json" | "jsonc" => Lang::Json,
            "toml" => Lang::Toml,
            "yml" | "yaml" => Lang::Yaml,
            "md" | "markdown" => Lang::Markdown,
            "c" | "h" => Lang::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Lang::Cpp,
            "go" => Lang::Go,
            "java" | "kt" | "kts" => Lang::Java,
            "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" => Lang::Shell,
            "html" | "htm" | "xml" | "svg" | "xhtml" => Lang::Html,
            "css" | "scss" | "less" => Lang::Css,
            "sql" => Lang::Sql,
            "ini" | "cfg" | "conf" | "properties" => Lang::Ini,
            _ => Lang::Plain,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Lang::Plain => "text",
            Lang::Rust => "rust",
            Lang::Python => "python",
            Lang::JavaScript => "javascript",
            Lang::TypeScript => "typescript",
            Lang::Json => "json",
            Lang::Toml => "toml",
            Lang::Yaml => "yaml",
            Lang::Markdown => "markdown",
            Lang::C => "c",
            Lang::Cpp => "c++",
            Lang::Go => "go",
            Lang::Java => "java",
            Lang::Shell => "shell",
            Lang::Html => "html",
            Lang::Css => "css",
            Lang::Sql => "sql",
            Lang::Ini => "ini",
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct HlState {
    /// Inside `/* … */` (C-like) or `<!-- … -->` (HTML).
    in_block: bool,
    /// HTML/XML: inside a tag.
    in_tag: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Keyword,
    Type,
    Builtin,
    String,
    Comment,
    Number,
    Operator,
    Punct,
    Function,
    Property,
    Constant,
    Escape,
}

fn style(kind: Kind) -> Style {
    match kind {
        Kind::Text => Style::default().fg(Color::Rgb(210, 214, 228)),
        Kind::Keyword => Style::default()
            .fg(Color::Rgb(198, 120, 221))
            .add_modifier(Modifier::BOLD),
        Kind::Type => Style::default().fg(Color::Rgb(229, 192, 123)),
        Kind::Builtin => Style::default().fg(Color::Rgb(86, 182, 194)),
        Kind::String => Style::default().fg(Color::Rgb(152, 195, 121)),
        Kind::Comment => Style::default()
            .fg(Color::Rgb(110, 118, 140))
            .add_modifier(Modifier::ITALIC),
        Kind::Number => Style::default().fg(Color::Rgb(209, 154, 102)),
        Kind::Operator => Style::default().fg(Color::Rgb(97, 175, 239)),
        Kind::Punct => Style::default().fg(Color::Rgb(170, 178, 191)),
        Kind::Function => Style::default().fg(Color::Rgb(97, 175, 239)),
        Kind::Property => Style::default().fg(Color::Rgb(224, 108, 117)),
        Kind::Constant => Style::default().fg(Color::Rgb(229, 192, 123)),
        Kind::Escape => Style::default().fg(Color::Rgb(209, 154, 102)),
    }
}

/// Highlight `line` for `lang`, updating `state` for multi-line constructs.
/// Returns spans covering the full line (may be empty for empty lines).
pub fn highlight_line<'a>(lang: Lang, line: &'a str, state: &mut HlState) -> Vec<Span<'a>> {
    if lang == Lang::Plain {
        return vec![Span::styled(line, style(Kind::Text))];
    }
    if lang == Lang::Markdown {
        return highlight_markdown(line);
    }

    let mut spans = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    let n = bytes.len();

    while i < n {
        // Continue block comment
        if state.in_block {
            let start = i;
            let end = match lang {
                Lang::Html => {
                    if let Some(rel) = line[i..].find("-->") {
                        state.in_block = false;
                        i + rel + 3
                    } else {
                        n
                    }
                }
                _ => {
                    if let Some(rel) = line[i..].find("*/") {
                        state.in_block = false;
                        i + rel + 2
                    } else {
                        n
                    }
                }
            };
            spans.push(Span::styled(&line[start..end], style(Kind::Comment)));
            i = end;
            continue;
        }

        // HTML-ish
        if matches!(lang, Lang::Html) {
            if bytes[i] == b'<' {
                if line[i..].starts_with("<!--") {
                    state.in_block = true;
                    let end = if let Some(rel) = line[i..].find("-->") {
                        state.in_block = false;
                        i + rel + 3
                    } else {
                        n
                    };
                    spans.push(Span::styled(&line[i..end], style(Kind::Comment)));
                    i = end;
                    continue;
                }
                state.in_tag = true;
                let start = i;
                i += 1;
                // tag name
                let name_start = i;
                while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'/' || bytes[i] == b'!' || bytes[i] == b'?') {
                    i += 1;
                }
                spans.push(Span::styled(&line[start..name_start], style(Kind::Punct)));
                if name_start < i {
                    spans.push(Span::styled(&line[name_start..i], style(Kind::Keyword)));
                }
                continue;
            }
            if state.in_tag {
                if bytes[i] == b'>' {
                    spans.push(Span::styled(&line[i..i + 1], style(Kind::Punct)));
                    i += 1;
                    state.in_tag = false;
                    continue;
                }
                if bytes[i] == b'=' {
                    spans.push(Span::styled(&line[i..i + 1], style(Kind::Operator)));
                    i += 1;
                    continue;
                }
                if bytes[i] == b'"' || bytes[i] == b'\'' {
                    let q = bytes[i];
                    let start = i;
                    i += 1;
                    while i < n && bytes[i] != q {
                        i += 1;
                    }
                    if i < n {
                        i += 1;
                    }
                    spans.push(Span::styled(&line[start..i], style(Kind::String)));
                    continue;
                }
                if bytes[i].is_ascii_alphabetic() || bytes[i] == b'-' || bytes[i] == b':' {
                    let start = i;
                    i += 1;
                    while i < n
                        && (bytes[i].is_ascii_alphanumeric()
                            || bytes[i] == b'-'
                            || bytes[i] == b':'
                            || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    spans.push(Span::styled(&line[start..i], style(Kind::Property)));
                    continue;
                }
            }
        }

        // Whitespace
        if bytes[i].is_ascii_whitespace() {
            let start = i;
            while i < n && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            spans.push(Span::styled(&line[start..i], style(Kind::Text)));
            continue;
        }

        // Line comments
        if let Some(end) = try_line_comment(lang, line, i) {
            spans.push(Span::styled(&line[i..end], style(Kind::Comment)));
            i = end;
            continue;
        }

        // Block comment start
        if matches!(
            lang,
            Lang::Rust
                | Lang::JavaScript
                | Lang::TypeScript
                | Lang::C
                | Lang::Cpp
                | Lang::Go
                | Lang::Java
                | Lang::Css
                | Lang::Sql
        ) && line[i..].starts_with("/*")
        {
            state.in_block = true;
            let start = i;
            i += 2;
            if let Some(rel) = line[i..].find("*/") {
                state.in_block = false;
                i += rel + 2;
            } else {
                i = n;
            }
            spans.push(Span::styled(&line[start..i], style(Kind::Comment)));
            continue;
        }

        // Strings
        if let Some((end, kinds)) = try_string(lang, line, i) {
            for (a, b, k) in kinds {
                spans.push(Span::styled(&line[a..b], style(k)));
            }
            i = end;
            continue;
        }

        // Numbers
        if bytes[i].is_ascii_digit()
            || (bytes[i] == b'.' && i + 1 < n && bytes[i + 1].is_ascii_digit())
        {
            let start = i;
            i += 1;
            while i < n
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'.'
                    || bytes[i] == b'_'
                    || bytes[i] == b'x'
                    || bytes[i] == b'X'
                    || bytes[i] == b'b'
                    || bytes[i] == b'B'
                    || bytes[i] == b'o'
                    || bytes[i] == b'O'
                    || bytes[i] == b'-'
                    || bytes[i] == b'+')
            {
                // stop on second sign mid-number unless exponent
                if (bytes[i] == b'+' || bytes[i] == b'-')
                    && !matches!(bytes.get(i.wrapping_sub(1)), Some(b'e' | b'E' | b'p' | b'P'))
                {
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(&line[start..i], style(Kind::Number)));
            continue;
        }

        // Identifiers / keywords
        if is_ident_start(lang, bytes[i]) {
            let start = i;
            i += 1;
            while i < n && is_ident_continue(lang, bytes[i]) {
                i += 1;
            }
            let word = &line[start..i];
            let mut kind = classify_word(lang, word);

            // function call: ident followed by '('
            if kind == Kind::Text {
                let mut j = i;
                while j < n && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < n && bytes[j] == b'(' {
                    kind = Kind::Function;
                }
            }
            // YAML/TOML key before ':'
            if matches!(lang, Lang::Yaml | Lang::Toml | Lang::Json)
                && kind == Kind::Text
            {
                let mut j = i;
                while j < n && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < n && bytes[j] == b':' {
                    kind = Kind::Property;
                }
            }
            // CSS property before ':'
            if lang == Lang::Css && kind == Kind::Text {
                let mut j = i;
                while j < n && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < n && bytes[j] == b':' {
                    kind = Kind::Property;
                }
            }

            spans.push(Span::styled(word, style(kind)));
            continue;
        }

        // Operators / punctuation
        let start = i;
        let rest = &line[i..];
        let (len, is_op) = if rest.starts_with("..=")
            || rest.starts_with("...")
            || rest.starts_with("<<=")
            || rest.starts_with(">>=")
        {
            (3, true)
        } else if matches!(
                rest.get(..2).unwrap_or(""),
                "==" | "!="
                    | "<=" | ">="
                    | "&&" | "||"
                    | "->" | "=>"
                    | "::" | "+="
                    | "-=" | "*="
                    | "/=" | "%="
                    | "<<" | ">>"
                    | ".." | "**"
                    | "??" | "?."
                    | "|=" | "&="
                    | "^="
            )
        {
            (2, true)
        } else if matches!(
            bytes[i] as char,
            '+' | '-' | '*' | '/' | '%' | '=' | '<' | '>' | '!' | '&' | '|' | '^' | '~' | '?' | ':'
                | '.'
        ) {
            (1, true)
        } else {
            // Whole char, so a stray non-ASCII symbol never splits a code point.
            (rest.chars().next().map_or(1, char::len_utf8), false)
        };
        i += len;
        let tok = &line[start..i];
        spans.push(Span::styled(
            tok,
            style(if is_op { Kind::Operator } else { Kind::Punct }),
        ));
    }

    if spans.is_empty() {
        spans.push(Span::styled(line, style(Kind::Text)));
    }
    spans
}

/// Slice spans to the character range `[from, from+take)`.
pub fn slice_spans(spans: Vec<Span<'_>>, from: usize, take: usize) -> Vec<Span<'static>> {
    if take == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut pos = 0usize;
    let end = from + take;
    for sp in spans {
        let len = sp.content.chars().count();
        let next = pos + len;
        if next <= from {
            pos = next;
            continue;
        }
        if pos >= end {
            break;
        }
        let skip = from.saturating_sub(pos);
        let keep = end.min(next) - pos.max(from);
        if keep > 0 {
            let s: String = sp.content.chars().skip(skip).take(keep).collect();
            out.push(Span::styled(s, sp.style));
        }
        pos = next;
        if pos >= end {
            break;
        }
    }
    out
}

fn highlight_markdown(line: &str) -> Vec<Span<'_>> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return vec![Span::styled(line, style(Kind::Keyword))];
    }
    if trimmed.starts_with('#') {
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if hashes <= 6 && (trimmed.len() == hashes || trimmed.as_bytes().get(hashes) == Some(&b' '))
        {
            return vec![Span::styled(line, style(Kind::Keyword))];
        }
    }
    if trimmed.starts_with('>') {
        return vec![Span::styled(line, style(Kind::Comment))];
    }
    if trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || (trimmed.len() > 2
            && trimmed.as_bytes()[0].is_ascii_digit()
            && trimmed.contains(". "))
    {
        // list marker tint
        let lead = line.len() - trimmed.len();
        let mut spans = Vec::new();
        if lead > 0 {
            spans.push(Span::styled(&line[..lead], style(Kind::Text)));
        }
        let marker_end = if trimmed.as_bytes()[0].is_ascii_digit() {
            trimmed.find(". ").map(|i| lead + i + 2).unwrap_or(lead + 2)
        } else {
            lead + 2
        };
        spans.push(Span::styled(&line[lead..marker_end.min(line.len())], style(Kind::Operator)));
        if marker_end < line.len() {
            spans.push(Span::styled(&line[marker_end..], style(Kind::Text)));
        }
        return spans;
    }
    // inline: highlight `code` and **bold** roughly
    let mut spans = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    let n = bytes.len();
    while i < n {
        if bytes[i] == b'`' {
            let start = i;
            i += 1;
            while i < n && bytes[i] != b'`' {
                i += 1;
            }
            if i < n {
                i += 1;
            }
            spans.push(Span::styled(&line[start..i], style(Kind::String)));
            continue;
        }
        if line[i..].starts_with("**") || line[i..].starts_with("__") {
            let mark = if line[i..].starts_with("**") { "**" } else { "__" };
            let start = i;
            i += 2;
            if let Some(rel) = line[i..].find(mark) {
                i += rel + 2;
                spans.push(Span::styled(
                    &line[start..i],
                    style(Kind::Keyword),
                ));
            } else {
                spans.push(Span::styled(&line[start..start + 2], style(Kind::Text)));
            }
            continue;
        }
        if bytes[i] == b'[' {
            let start = i;
            if let Some(rel) = line[i..].find("](") {
                if let Some(rel2) = line[i + rel..].find(')') {
                    i = i + rel + rel2 + 1;
                    spans.push(Span::styled(&line[start..i], style(Kind::Function)));
                    continue;
                }
            }
        }
        let start = i;
        i += 1;
        while i < n
            && bytes[i] != b'`'
            && !(bytes[i..].starts_with(b"**") || bytes[i..].starts_with(b"__"))
            && bytes[i] != b'['
        {
            i += 1;
        }
        spans.push(Span::styled(&line[start..i], style(Kind::Text)));
    }
    if spans.is_empty() {
        spans.push(Span::styled(line, style(Kind::Text)));
    }
    spans
}

fn try_line_comment(lang: Lang, line: &str, i: usize) -> Option<usize> {
    let rest = &line[i..];
    match lang {
        Lang::Python | Lang::Toml | Lang::Yaml | Lang::Shell | Lang::Ini => {
            if rest.starts_with('#') {
                Some(line.len())
            } else {
                None
            }
        }
        Lang::Sql => {
            if rest.starts_with("--") {
                Some(line.len())
            } else {
                None
            }
        }
        Lang::Rust
        | Lang::JavaScript
        | Lang::TypeScript
        | Lang::C
        | Lang::Cpp
        | Lang::Go
        | Lang::Java
        | Lang::Css
        | Lang::Json => {
            if rest.starts_with("//") {
                Some(line.len())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn try_string(lang: Lang, line: &str, i: usize) -> Option<(usize, Vec<(usize, usize, Kind)>)> {
    let bytes = line.as_bytes();
    // Python triple quotes
    if lang == Lang::Python {
        for q in ["\"\"\"", "'''"] {
            if line[i..].starts_with(q) {
                let start = i;
                let after = i + 3;
                if let Some(rel) = line[after..].find(q) {
                    let end = after + rel + 3;
                    return Some((end, vec![(start, end, Kind::String)]));
                }
                return Some((line.len(), vec![(start, line.len(), Kind::String)]));
            }
        }
    }
    // Rust raw strings r"…" / r#"…"#
    if lang == Lang::Rust && (line[i..].starts_with("r\"") || line[i..].starts_with("r#")) {
        let start = i;
        let mut j = i + 1;
        let mut hashes = 0usize;
        if bytes.get(j) == Some(&b'#') {
            while j < bytes.len() && bytes[j] == b'#' {
                hashes += 1;
                j += 1;
            }
        }
        if bytes.get(j) != Some(&b'"') {
            return None;
        }
        j += 1;
        let closing = format!("\"{}", "#".repeat(hashes));
        if let Some(rel) = line[j..].find(&closing) {
            let end = j + rel + closing.len();
            return Some((end, vec![(start, end, Kind::String)]));
        }
        return Some((line.len(), vec![(start, line.len(), Kind::String)]));
    }

    let quote = bytes[i];
    if quote != b'"' && quote != b'\'' {
        // backticks for JS/TS/shell/markdown already handled
        if matches!(lang, Lang::JavaScript | Lang::TypeScript | Lang::Shell) && quote == b'`' {
            return scan_quoted(line, i, b'`', lang);
        }
        return None;
    }
    // Python: allow f/b/r/u prefixes already consumed as ident — here bare quotes
    // Char literals in Rust/C: 'x' — still string-colored, fine
    if lang == Lang::Rust && quote == b'\'' {
        // lifetime or char: 'a or 'a' or '\n'
        if let Some(&b) = bytes.get(i + 1) {
            if b.is_ascii_alphabetic() || b == b'_' {
                // lifetime 'foo — not a string
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                // if next is not quote, it's a lifetime
                if bytes.get(j) != Some(&b'\'') {
                    return None;
                }
            }
        }
    }
    scan_quoted(line, i, quote, lang)
}

fn scan_quoted(
    line: &str,
    i: usize,
    quote: u8,
    lang: Lang,
) -> Option<(usize, Vec<(usize, usize, Kind)>)> {
    let bytes = line.as_bytes();
    let mut parts = Vec::new();
    let start = i;
    let mut j = i + 1;
    let mut seg = start;
    while j < bytes.len() {
        if bytes[j] == b'\\' && j + 1 < bytes.len() {
            if seg < j {
                parts.push((seg, j, Kind::String));
            }
            // The escaped char may be multi-byte (`"\ä"`); never split it.
            let esc = 1 + line[j + 1..].chars().next().map_or(1, char::len_utf8);
            parts.push((j, j + esc, Kind::Escape));
            j += esc;
            seg = j;
            continue;
        }
        if bytes[j] == quote {
            j += 1;
            if seg < j {
                parts.push((seg, j, Kind::String));
            }
            return Some((j, parts));
        }
        // JS template ${}
        if quote == b'`'
            && matches!(lang, Lang::JavaScript | Lang::TypeScript)
            && bytes[j] == b'$'
            && bytes.get(j + 1) == Some(&b'{')
        {
            if seg < j {
                parts.push((seg, j, Kind::String));
            }
            let interp_start = j;
            j += 2;
            let mut depth = 1i32;
            while j < bytes.len() && depth > 0 {
                if bytes[j] == b'{' {
                    depth += 1;
                } else if bytes[j] == b'}' {
                    depth -= 1;
                }
                j += 1;
            }
            parts.push((interp_start, j, Kind::Keyword));
            seg = j;
            continue;
        }
        j += 1;
    }
    if seg < bytes.len() {
        parts.push((seg, bytes.len(), Kind::String));
    } else if parts.is_empty() {
        parts.push((start, bytes.len(), Kind::String));
    }
    Some((bytes.len(), parts))
}

fn is_ident_start(lang: Lang, b: u8) -> bool {
    b.is_ascii_alphabetic()
        || b == b'_'
        || (matches!(lang, Lang::Css | Lang::Shell) && (b == b'-' || b == b'$'))
}

fn is_ident_continue(lang: Lang, b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || b == b'_'
        || (matches!(lang, Lang::Css | Lang::Html) && b == b'-')
        || (lang == Lang::Shell && (b == b'-' || b == b'$'))
}

fn classify_word(lang: Lang, word: &str) -> Kind {
    let check = if lang == Lang::Sql {
        word.to_ascii_lowercase()
    } else {
        word.to_string()
    };
    let w = check.as_str();
    if is_keyword(lang, w) {
        return Kind::Keyword;
    }
    if is_type(lang, w) {
        return Kind::Type;
    }
    if is_builtin(lang, word) {
        return Kind::Builtin;
    }
    if is_constant(lang, word) {
        return Kind::Constant;
    }
    Kind::Text
}

fn is_constant(lang: Lang, w: &str) -> bool {
    match lang {
        Lang::Python => matches!(w, "True" | "False" | "None" | "Ellipsis" | "NotImplemented"),
        Lang::JavaScript | Lang::TypeScript => {
            matches!(w, "true" | "false" | "null" | "undefined" | "NaN" | "Infinity")
        }
        Lang::Rust | Lang::C | Lang::Cpp | Lang::Go | Lang::Java => {
            matches!(w, "true" | "false" | "null" | "NULL" | "nil" | "None" | "Some" | "Ok" | "Err")
        }
        Lang::Json => matches!(w, "true" | "false" | "null"),
        Lang::Toml | Lang::Yaml => matches!(w, "true" | "false" | "null" | "yes" | "no" | "on" | "off"),
        _ => false,
    }
}

fn is_builtin(lang: Lang, w: &str) -> bool {
    match lang {
        Lang::Python => matches!(
            w,
            "print"
                | "len"
                | "range"
                | "open"
                | "str"
                | "int"
                | "float"
                | "list"
                | "dict"
                | "set"
                | "tuple"
                | "bool"
                | "type"
                | "isinstance"
                | "super"
                | "property"
                | "staticmethod"
                | "classmethod"
                | "enumerate"
                | "zip"
                | "map"
                | "filter"
                | "sum"
                | "min"
                | "max"
                | "abs"
                | "sorted"
                | "reversed"
                | "input"
                | "object"
                | "Exception"
                | "ValueError"
                | "TypeError"
                | "KeyError"
                | "self"
                | "cls"
        ),
        Lang::JavaScript | Lang::TypeScript => matches!(
            w,
            "console"
                | "window"
                | "document"
                | "Array"
                | "Object"
                | "String"
                | "Number"
                | "Boolean"
                | "Promise"
                | "Math"
                | "JSON"
                | "Date"
                | "Error"
                | "Map"
                | "Set"
                | "parseInt"
                | "parseFloat"
                | "isNaN"
                | "setTimeout"
                | "setInterval"
                | "fetch"
        ),
        Lang::Rust => matches!(
            w,
            "Some"
                | "None"
                | "Ok"
                | "Err"
                | "Vec"
                | "String"
                | "Option"
                | "Result"
                | "Box"
                | "Rc"
                | "Arc"
                | "Self"
                | "self"
                | "super"
                | "crate"
        ),
        Lang::Shell => matches!(w, "echo" | "cd" | "export" | "alias" | "source" | "exit" | "read"),
        _ => false,
    }
}

fn is_type(lang: Lang, w: &str) -> bool {
    match lang {
        Lang::Rust => matches!(
            w,
            "i8" | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "f32"
                | "f64"
                | "bool"
                | "char"
                | "str"
                | "Self"
        ),
        Lang::C | Lang::Cpp => matches!(
            w,
            "int"
                | "char"
                | "float"
                | "double"
                | "void"
                | "bool"
                | "short"
                | "long"
                | "unsigned"
                | "signed"
                | "size_t"
                | "uint8_t"
                | "uint16_t"
                | "uint32_t"
                | "uint64_t"
                | "int8_t"
                | "int16_t"
                | "int32_t"
                | "int64_t"
                | "wchar_t"
                | "auto"
        ),
        Lang::Go => matches!(
            w,
            "bool"
                | "byte"
                | "complex64"
                | "complex128"
                | "error"
                | "float32"
                | "float64"
                | "int"
                | "int8"
                | "int16"
                | "int32"
                | "int64"
                | "rune"
                | "string"
                | "uint"
                | "uint8"
                | "uint16"
                | "uint32"
                | "uint64"
                | "uintptr"
        ),
        Lang::Java | Lang::TypeScript => matches!(
            w,
            "string"
                | "number"
                | "boolean"
                | "any"
                | "void"
                | "never"
                | "unknown"
                | "object"
                | "int"
                | "long"
                | "float"
                | "double"
                | "char"
                | "byte"
                | "short"
                | "String"
                | "Integer"
                | "Boolean"
                | "Object"
                | "List"
                | "Map"
        ),
        Lang::Css => matches!(
            w,
            "px" | "em" | "rem" | "vh" | "vw" | "rgb" | "rgba" | "hsl" | "hsla" | "url" | "var"
        ),
        _ => false,
    }
}

fn is_keyword(lang: Lang, w: &str) -> bool {
    match lang {
        Lang::Rust => matches!(
            w,
            "as" | "async"
                | "await"
                | "break"
                | "const"
                | "continue"
                | "crate"
                | "dyn"
                | "else"
                | "enum"
                | "extern"
                | "false"
                | "fn"
                | "for"
                | "if"
                | "impl"
                | "in"
                | "let"
                | "loop"
                | "match"
                | "mod"
                | "move"
                | "mut"
                | "pub"
                | "ref"
                | "return"
                | "self"
                | "Self"
                | "static"
                | "struct"
                | "super"
                | "trait"
                | "true"
                | "type"
                | "unsafe"
                | "use"
                | "where"
                | "while"
                | "yield"
                | "union"
                | "box"
                | "become"
                | "abstract"
                | "final"
                | "override"
                | "macro_rules"
                | "typeof"
                | "sizeof"
                | "alignof"
                | "offsetof"
        ),
        Lang::Python => matches!(
            w,
            "and"
                | "as"
                | "assert"
                | "async"
                | "await"
                | "break"
                | "class"
                | "continue"
                | "def"
                | "del"
                | "elif"
                | "else"
                | "except"
                | "finally"
                | "for"
                | "from"
                | "global"
                | "if"
                | "import"
                | "in"
                | "is"
                | "lambda"
                | "nonlocal"
                | "not"
                | "or"
                | "pass"
                | "raise"
                | "return"
                | "try"
                | "while"
                | "with"
                | "yield"
                | "match"
                | "case"
        ),
        Lang::JavaScript | Lang::TypeScript => matches!(
            w,
            "await"
                | "break"
                | "case"
                | "catch"
                | "class"
                | "const"
                | "continue"
                | "debugger"
                | "default"
                | "delete"
                | "do"
                | "else"
                | "export"
                | "extends"
                | "false"
                | "finally"
                | "for"
                | "function"
                | "if"
                | "import"
                | "in"
                | "instanceof"
                | "new"
                | "null"
                | "return"
                | "super"
                | "switch"
                | "this"
                | "throw"
                | "true"
                | "try"
                | "typeof"
                | "var"
                | "void"
                | "while"
                | "with"
                | "yield"
                | "let"
                | "static"
                | "enum"
                | "implements"
                | "interface"
                | "package"
                | "private"
                | "protected"
                | "public"
                | "abstract"
                | "as"
                | "asserts"
                | "any"
                | "async"
                | "from"
                | "get"
                | "set"
                | "of"
                | "keyof"
                | "readonly"
                | "infer"
                | "is"
                | "type"
                | "namespace"
                | "declare"
                | "module"
        ),
        Lang::C | Lang::Cpp => matches!(
            w,
            "auto"
                | "break"
                | "case"
                | "char"
                | "const"
                | "continue"
                | "default"
                | "do"
                | "double"
                | "else"
                | "enum"
                | "extern"
                | "float"
                | "for"
                | "goto"
                | "if"
                | "inline"
                | "int"
                | "long"
                | "register"
                | "restrict"
                | "return"
                | "short"
                | "signed"
                | "sizeof"
                | "static"
                | "struct"
                | "switch"
                | "typedef"
                | "union"
                | "unsigned"
                | "void"
                | "volatile"
                | "while"
                | "class"
                | "namespace"
                | "template"
                | "typename"
                | "using"
                | "public"
                | "private"
                | "protected"
                | "virtual"
                | "friend"
                | "operator"
                | "new"
                | "delete"
                | "this"
                | "try"
                | "catch"
                | "throw"
                | "true"
                | "false"
                | "nullptr"
                | "constexpr"
                | "consteval"
                | "constinit"
                | "concept"
                | "requires"
                | "co_await"
                | "co_return"
                | "co_yield"
        ),
        Lang::Go => matches!(
            w,
            "break"
                | "case"
                | "chan"
                | "const"
                | "continue"
                | "default"
                | "defer"
                | "else"
                | "fallthrough"
                | "for"
                | "func"
                | "go"
                | "goto"
                | "if"
                | "import"
                | "interface"
                | "map"
                | "package"
                | "range"
                | "return"
                | "select"
                | "struct"
                | "switch"
                | "type"
                | "var"
        ),
        Lang::Java => matches!(
            w,
            "abstract"
                | "assert"
                | "boolean"
                | "break"
                | "byte"
                | "case"
                | "catch"
                | "char"
                | "class"
                | "const"
                | "continue"
                | "default"
                | "do"
                | "double"
                | "else"
                | "enum"
                | "extends"
                | "final"
                | "finally"
                | "float"
                | "for"
                | "goto"
                | "if"
                | "implements"
                | "import"
                | "instanceof"
                | "int"
                | "interface"
                | "long"
                | "native"
                | "new"
                | "package"
                | "private"
                | "protected"
                | "public"
                | "return"
                | "short"
                | "static"
                | "strictfp"
                | "super"
                | "switch"
                | "synchronized"
                | "this"
                | "throw"
                | "throws"
                | "transient"
                | "try"
                | "void"
                | "volatile"
                | "while"
                | "var"
                | "yield"
                | "record"
                | "sealed"
                | "permits"
                | "non-sealed"
        ),
        Lang::Shell => matches!(
            w,
            "if" | "then"
                | "else"
                | "elif"
                | "fi"
                | "for"
                | "while"
                | "do"
                | "done"
                | "case"
                | "esac"
                | "function"
                | "select"
                | "until"
                | "in"
                | "time"
                | "coproc"
                | "return"
                | "exit"
                | "local"
                | "declare"
                | "typeset"
                | "unset"
                | "export"
                | "readonly"
                | "shift"
                | "trap"
                | "true"
                | "false"
        ),
        Lang::Sql => matches!(
            w,
            "select"
                | "from"
                | "where"
                | "insert"
                | "into"
                | "values"
                | "update"
                | "set"
                | "delete"
                | "create"
                | "table"
                | "drop"
                | "alter"
                | "index"
                | "join"
                | "left"
                | "right"
                | "inner"
                | "outer"
                | "on"
                | "as"
                | "and"
                | "or"
                | "not"
                | "null"
                | "primary"
                | "key"
                | "foreign"
                | "references"
                | "group"
                | "by"
                | "order"
                | "having"
                | "limit"
                | "offset"
                | "union"
                | "all"
                | "distinct"
                | "case"
                | "when"
                | "then"
                | "else"
                | "end"
                | "in"
                | "is"
                | "like"
                | "between"
                | "exists"
                | "asc"
                | "desc"
                | "with"
        ),
        Lang::Css => matches!(
            w,
            "important"
                | "media"
                | "keyframes"
                | "from"
                | "to"
                | "import"
                | "charset"
                | "supports"
                | "font-face"
        ),
        Lang::Toml | Lang::Ini => matches!(w, "true" | "false"),
        Lang::Yaml | Lang::Json | Lang::Html | Lang::Markdown | Lang::Plain => false,
    }
}

/// Walk lines before `upto` to recover multi-line highlight state.
pub fn state_before(lang: Lang, lines: &[String], upto: usize) -> HlState {
    let mut state = HlState::default();
    if lang == Lang::Plain || lang == Lang::Markdown {
        return state;
    }
    for line in lines.iter().take(upto) {
        let _ = highlight_line(lang, line, &mut state);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tokenizer walks bytes; every span it cuts must still land on a char
    /// boundary, or opening a file with an umlaut or emoji panics the editor.
    #[test]
    fn non_ascii_lines_never_split_a_char() {
        let langs = [
            Lang::Plain, Lang::Rust, Lang::Python, Lang::JavaScript, Lang::TypeScript,
            Lang::Json, Lang::Toml, Lang::Yaml, Lang::Markdown, Lang::C, Lang::Cpp,
            Lang::Go, Lang::Java, Lang::Shell, Lang::Html, Lang::Css, Lang::Sql, Lang::Ini,
        ];
        let atoms = [
            "ä", "€", "😀", "\"", "'", "`", "/*", "*/", "//", "#", "<!--", "-->", "<", "=",
            ":", "r#\"", "\"#", "\\", "**", "__", "[", "](", ")", "1", ".", "a", " ", "-",
            "$", "{", "}", "'''", "\"\"\"",
        ];
        let mut seed: u64 = 12345;
        for _ in 0..50_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let line: String = (0..(seed % 8) as usize)
                .map(|k| atoms[(seed >> (k * 6)) as usize % atoms.len()])
                .collect();
            let lang = langs[(seed >> 50) as usize % langs.len()];
            let spans = highlight_line(lang, &line, &mut HlState::default());
            let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(joined, line, "{}", lang.label());
        }
    }
}
