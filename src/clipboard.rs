//! System clipboard access for text.
//!
//! Two mechanisms, because neither covers every case:
//!
//! * **OSC 52** — an escape sequence asking the terminal to set its clipboard.
//!   It is the only thing that works over ssh, and most terminals support it
//!   for writing. Reading it back is disabled almost everywhere (a remote host
//!   being able to read your clipboard is a security hole), so it is write-only
//!   here.
//! * **The platform tool** (`pbcopy`/`pbpaste`, `xclip`, `clip.exe`) — works
//!   both directions, but only when texttop runs on the same machine as the
//!   terminal.
//!
//! Copies go out through both. Pastes read the platform tool, falling back to
//! whatever we last copied ourselves.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// The last text copied from inside texttop. Authoritative for our own copies,
/// since the system clipboard is not always readable.
static LOCAL: Mutex<String> = Mutex::new(String::new());

/// Cap on what we push through OSC 52. Terminals impose their own limits and a
/// huge sequence just gets truncated into garbage; the platform tool still
/// carries the whole thing.
const OSC52_MAX: usize = 96 * 1024;

pub fn copy(text: &str) {
    if let Ok(mut g) = LOCAL.lock() {
        *g = text.to_string();
    }
    native_copy(text);
    if text.len() <= OSC52_MAX {
        osc52(text);
    }
}

pub fn paste() -> String {
    if let Some(s) = native_paste() {
        return s;
    }
    LOCAL.lock().map(|g| g.clone()).unwrap_or_default()
}

/// Ask the terminal to set its clipboard: `ESC ] 52 ; c ; <base64> BEL`.
fn osc52(text: &str) {
    let mut out = std::io::stdout();
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for i in 0..4 {
            // Positions past the end of a short chunk are padding, not data.
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 0x3f] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// (program, args) that reads stdin into the clipboard on this platform.
fn copy_cmd() -> (&'static str, &'static [&'static str]) {
    if cfg!(target_os = "macos") {
        ("pbcopy", &[])
    } else if cfg!(windows) {
        ("clip", &[])
    } else {
        ("xclip", &["-selection", "clipboard"])
    }
}

/// (program, args) that prints the clipboard to stdout on this platform.
fn paste_cmd() -> (&'static str, &'static [&'static str]) {
    if cfg!(target_os = "macos") {
        ("pbpaste", &[])
    } else if cfg!(windows) {
        ("powershell", &["-NoProfile", "-Command", "Get-Clipboard"])
    } else {
        ("xclip", &["-selection", "clipboard", "-o"])
    }
}

fn native_copy(text: &str) -> bool {
    let (prog, args) = copy_cmd();
    let Ok(mut child) = Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(stdin) = child.stdin.as_mut() {
        if stdin.write_all(text.as_bytes()).is_err() {
            return false;
        }
    }
    matches!(child.wait(), Ok(s) if s.success())
}

fn native_paste() -> Option<String> {
    let (prog, args) = paste_cmd();
    let out = Command::new(prog)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    // Get-Clipboard appends a newline that was never in the clipboard.
    if cfg!(windows) {
        Some(s.trim_end_matches(['\r', '\n']).to_string())
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_rfc4648_including_padding() {
        for (raw, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
            ("/Users/dan/Desktop", "L1VzZXJzL2Rhbi9EZXNrdG9w"),
        ] {
            assert_eq!(base64(raw.as_bytes()), want, "input {raw:?}");
        }
    }
}
