//! Thin wrapper around the system `git` CLI with a short TTL cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitLetter {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Unmerged,
    Ignored,
}

impl GitLetter {
    pub fn as_char(self) -> char {
        match self {
            GitLetter::Modified => 'M',
            GitLetter::Added => 'A',
            GitLetter::Deleted => 'D',
            GitLetter::Renamed => 'R',
            GitLetter::Untracked => '?',
            GitLetter::Unmerged => 'U',
            GitLetter::Ignored => '!',
        }
    }

    pub fn color(self) -> ratatui::style::Color {
        use ratatui::style::Color;
        match self {
            GitLetter::Modified => Color::Rgb(255, 200, 90),
            GitLetter::Added => Color::Rgb(110, 220, 140),
            GitLetter::Deleted => Color::Rgb(255, 110, 110),
            GitLetter::Renamed => Color::Rgb(140, 180, 255),
            GitLetter::Untracked => Color::Rgb(140, 150, 170),
            GitLetter::Unmerged => Color::Rgb(255, 120, 200),
            GitLetter::Ignored => Color::Rgb(90, 95, 110),
        }
    }

    fn from_xy(x: u8, y: u8) -> Self {
        if x == b'U' || y == b'U' || (x == b'D' && y == b'D') || (x == b'A' && y == b'A') {
            return GitLetter::Unmerged;
        }
        if x == b'?' || y == b'?' {
            return GitLetter::Untracked;
        }
        if x == b'!' || y == b'!' {
            return GitLetter::Ignored;
        }
        let c = if y != b' ' { y } else { x };
        match c {
            b'M' => GitLetter::Modified,
            b'A' => GitLetter::Added,
            b'D' => GitLetter::Deleted,
            b'R' | b'C' => GitLetter::Renamed,
            _ => GitLetter::Modified,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ShortStat {
    pub branch: String,
    pub dirty: bool,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub ahead: u32,
    pub behind: u32,
}

impl ShortStat {
    pub fn summary(&self) -> String {
        if self.branch.is_empty() {
            return String::new();
        }
        let mut s = format!("⎇ {}", self.branch);
        if self.dirty {
            s.push('*');
        }
        if self.ahead > 0 {
            s.push_str(&format!(" ↑{}", self.ahead));
        }
        if self.behind > 0 {
            s.push_str(&format!(" ↓{}", self.behind));
        }
        if self.staged > 0 || self.unstaged > 0 || self.untracked > 0 {
            s.push_str(&format!(
                " +{} ~{} ?{}",
                self.staged, self.unstaged, self.untracked
            ));
        }
        s
    }

    pub fn taskbar(&self) -> String {
        if self.branch.is_empty() {
            return String::new();
        }
        let mark = if self.dirty { "*" } else { "" };
        format!(" ⎇ {}{} ", self.branch, mark)
    }
}

#[derive(Clone, Debug)]
pub struct StatusEntry {
    pub path: PathBuf,
    pub letter: GitLetter,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
}

struct CacheEntry<T> {
    at: Instant,
    val: T,
}

struct Cache {
    available: Option<CacheEntry<bool>>,
    roots: HashMap<PathBuf, CacheEntry<Option<PathBuf>>>,
    stats: HashMap<PathBuf, CacheEntry<ShortStat>>,
    letters: HashMap<PathBuf, CacheEntry<HashMap<String, GitLetter>>>,
}

impl Cache {
    fn new() -> Self {
        Cache {
            available: None,
            roots: HashMap::new(),
            stats: HashMap::new(),
            letters: HashMap::new(),
        }
    }
}

fn cache_lock() -> std::sync::MutexGuard<'static, Cache> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(|| Mutex::new(Cache::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn git_cmd() -> Command {
    let mut c = Command::new("git");
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

pub fn run_git(root: &Path, args: &[&str]) -> Option<String> {
    let out = git_cmd().args(args).current_dir(root).output().ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn run_git_status(root: &Path, args: &[&str]) -> bool {
    git_cmd()
        .args(args)
        .current_dir(root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn git_available() -> bool {
    let mut c = cache_lock();
    if let Some(e) = &c.available {
        if e.at.elapsed() < Duration::from_secs(30) {
            return e.val;
        }
    }
    let ok = git_cmd()
        .args(["--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    c.available = Some(CacheEntry {
        at: Instant::now(),
        val: ok,
    });
    ok
}

pub fn find_root(path: &Path) -> Option<PathBuf> {
    let key = path.to_path_buf();
    {
        let c = cache_lock();
        if let Some(e) = c.roots.get(&key) {
            if e.at.elapsed() < CACHE_TTL {
                return e.val.clone();
            }
        }
    }
    if !git_available() {
        return None;
    }
    let start = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let out = git_cmd()
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .ok()?;
    let root = if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(PathBuf::from(s))
        }
    } else {
        None
    };
    cache_lock().roots.insert(
        key,
        CacheEntry {
            at: Instant::now(),
            val: root.clone(),
        },
    );
    root
}

pub fn invalidate(root: &Path) {
    let mut c = cache_lock();
    c.stats.remove(root);
    c.letters.remove(root);
    c.roots.retain(|_, v| v.val.as_deref() != Some(root));
}

pub fn branch(root: &Path) -> String {
    run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "HEAD".into())
}

pub fn short_stat(root: &Path) -> ShortStat {
    {
        let c = cache_lock();
        if let Some(e) = c.stats.get(root) {
            if e.at.elapsed() < CACHE_TTL {
                return e.val.clone();
            }
        }
    }
    let mut st = ShortStat {
        branch: branch(root),
        ..Default::default()
    };
    if let Some(por) = run_git(root, &["status", "--porcelain=v1", "-b"]) {
        for line in por.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                // ## main...origin/main [ahead 1, behind 2]
                let head = rest.split_whitespace().next().unwrap_or("");
                let br = head.split("...").next().unwrap_or(head);
                if !br.is_empty() {
                    st.branch = br.to_string();
                }
                if let Some(i) = rest.find("ahead ") {
                    st.ahead = rest[i + 6..]
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect::<String>()
                        .parse()
                        .unwrap_or(0);
                }
                if let Some(i) = rest.find("behind ") {
                    st.behind = rest[i + 7..]
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect::<String>()
                        .parse()
                        .unwrap_or(0);
                }
                continue;
            }
            if line.len() < 3 {
                continue;
            }
            let x = line.as_bytes()[0];
            let y = line.as_bytes()[1];
            if x == b'?' {
                st.untracked += 1;
                st.dirty = true;
            } else {
                if x != b' ' {
                    st.staged += 1;
                    st.dirty = true;
                }
                if y != b' ' {
                    st.unstaged += 1;
                    st.dirty = true;
                }
            }
        }
    }
    cache_lock().stats.insert(
        root.to_path_buf(),
        CacheEntry {
            at: Instant::now(),
            val: st.clone(),
        },
    );
    st
}

pub fn status_entries(root: &Path) -> Vec<StatusEntry> {
    let Some(por) = run_git(root, &["status", "--porcelain=v1"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in por.lines() {
        if line.len() < 4 {
            continue;
        }
        let x = line.as_bytes()[0];
        let y = line.as_bytes()[1];
        let mut path_str = line[3..].to_string();
        if let Some((_, right)) = path_str.split_once(" -> ") {
            path_str = right.to_string();
        }
        path_str = path_str.trim_matches('"').to_string();
        let letter = GitLetter::from_xy(x, y);
        out.push(StatusEntry {
            path: root.join(&path_str),
            letter,
            staged: x != b' ' && x != b'?',
            unstaged: y != b' ' && y != b'?',
            untracked: x == b'?' || y == b'?',
        });
    }
    out
}

/// Map relative path (as shown in Files) → letter for badge paint.
pub fn letter_map(root: &Path) -> HashMap<String, GitLetter> {
    {
        let c = cache_lock();
        if let Some(e) = c.letters.get(root) {
            if e.at.elapsed() < CACHE_TTL {
                return e.val.clone();
            }
        }
    }
    let mut map = HashMap::new();
    for e in status_entries(root) {
        if let Ok(rel) = e.path.strip_prefix(root) {
            let key = rel.to_string_lossy().replace('\\', "/");
            map.insert(key, e.letter);
            // also mark top-level component for dirs
            if let Some(first) = rel.components().next() {
                let d = first.as_os_str().to_string_lossy().to_string();
                map.entry(d).or_insert(e.letter);
            }
        }
    }
    cache_lock().letters.insert(
        root.to_path_buf(),
        CacheEntry {
            at: Instant::now(),
            val: map.clone(),
        },
    );
    map
}

pub fn letter_for(root: &Path, entry_name: &str, entry_path: &Path) -> Option<GitLetter> {
    let map = letter_map(root);
    if let Ok(rel) = entry_path.strip_prefix(root) {
        let key = rel.to_string_lossy().replace('\\', "/");
        if let Some(l) = map.get(&key) {
            return Some(*l);
        }
    }
    map.get(entry_name).copied()
}

pub fn log_oneline(root: &Path, n: usize) -> Vec<(String, String)> {
    let fmt = "%h\t%s";
    let arg = format!("-{n}");
    let Some(out) = run_git(root, &["log", &arg, &format!("--format={fmt}")]) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|l| {
            let (h, s) = l.split_once('\t')?;
            Some((h.to_string(), s.to_string()))
        })
        .collect()
}

pub fn show_commit(root: &Path, hash: &str) -> String {
    run_git(
        root,
        &["show", "--stat", "--format=fuller", "--no-color", hash],
    )
    .unwrap_or_else(|| "cannot show commit".into())
}

pub fn diff(root: &Path, path: Option<&Path>) -> String {
    let mut args = vec!["diff", "--no-color"];
    let path_s;
    if let Some(p) = path {
        args.push("--");
        path_s = p.to_string_lossy().to_string();
        args.push(&path_s);
    }
    run_git(root, &args).unwrap_or_else(|| "no diff".into())
}

pub fn stage(root: &Path, path: &Path) -> Result<(), String> {
    let rel = path.strip_prefix(root).unwrap_or(path);
    if run_git_status(root, &["add", "--", &rel.to_string_lossy()]) {
        invalidate(root);
        Ok(())
    } else {
        Err("git add failed".into())
    }
}

pub fn unstage(root: &Path, path: &Path) -> Result<(), String> {
    let rel = path.strip_prefix(root).unwrap_or(path);
    if run_git_status(root, &["restore", "--staged", "--", &rel.to_string_lossy()])
        || run_git_status(root, &["reset", "HEAD", "--", &rel.to_string_lossy()])
    {
        invalidate(root);
        Ok(())
    } else {
        Err("git unstage failed".into())
    }
}

pub fn commit(root: &Path, message: &str) -> Result<(), String> {
    if message.trim().is_empty() {
        return Err("empty commit message".into());
    }
    if run_git_status(root, &["commit", "-m", message.trim()]) {
        invalidate(root);
        Ok(())
    } else {
        Err("git commit failed (nothing staged?)".into())
    }
}

pub fn branches(root: &Path) -> Vec<(String, bool)> {
    let Some(out) = run_git(root, &["branch", "--list"]) else {
        return Vec::new();
    };
    out.lines()
        .map(|l| {
            let current = l.starts_with('*');
            let name = l.trim_start_matches('*').trim().to_string();
            (name, current)
        })
        .filter(|(n, _)| !n.is_empty())
        .collect()
}

pub fn checkout(root: &Path, name: &str) -> Result<(), String> {
    if run_git_status(root, &["checkout", name]) {
        invalidate(root);
        Ok(())
    } else {
        Err(format!("checkout {name} failed"))
    }
}

pub fn create_branch(root: &Path, name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("empty branch name".into());
    }
    if run_git_status(root, &["checkout", "-b", name.trim()]) {
        invalidate(root);
        Ok(())
    } else {
        Err("create branch failed".into())
    }
}
