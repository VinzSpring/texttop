//! Persistent browser data: history, bookmarks, and privacy settings.
//!
//! Everything lives under `~/.texttop_browser/` as plain JSON, one file per
//! concern, so it can be inspected or deleted by hand. Cookies and cache are
//! *not* here — those belong to the engine's own data store; see
//! [`crate::engine::Cmd::ClearData`].

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

/// Visits kept before the oldest are dropped. Large enough to be useful,
/// small enough that the whole file is cheap to read on every browser open.
const HISTORY_CAP: usize = 2000;

fn store_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".texttop_browser")
}

fn read(name: &str) -> Option<Value> {
    let text = fs::read_to_string(store_dir().join(name)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write(name: &str, v: &Value) {
    let dir = store_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(v) {
        let _ = fs::write(dir.join(name), text);
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A page worth remembering — one row in history or the bookmark list.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub url: String,
    pub title: String,
    pub ts: i64,
}

impl Entry {
    fn to_json(&self) -> Value {
        json!({"url": self.url, "title": self.title, "ts": self.ts})
    }

    fn from_json(v: &Value) -> Option<Entry> {
        let url = v.get("url")?.as_str()?.to_string();
        if url.is_empty() {
            return None;
        }
        Some(Entry {
            url,
            title: v.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            ts: v.get("ts").and_then(|t| t.as_i64()).unwrap_or(0),
        })
    }

    /// Local time, for the list views.
    pub fn when(&self) -> String {
        use chrono::{DateTime, Local, Utc};
        DateTime::<Utc>::from_timestamp(self.ts, 0)
            .map(|u| u.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default()
    }

    /// What to show when a page had no title.
    pub fn label(&self) -> &str {
        if self.title.is_empty() {
            &self.url
        } else {
            &self.title
        }
    }
}

fn load_list(name: &str, key: &str) -> Vec<Entry> {
    read(name)
        .and_then(|v| v.get(key).and_then(|a| a.as_array()).cloned())
        .map(|arr| arr.iter().filter_map(Entry::from_json).collect())
        .unwrap_or_default()
}

fn save_list(name: &str, key: &str, items: &[Entry]) {
    let arr: Vec<Value> = items.iter().map(Entry::to_json).collect();
    write(name, &json!({ key: arr }));
}

/// Newest first.
#[derive(Default)]
pub struct History {
    pub visits: Vec<Entry>,
}

impl History {
    pub fn load() -> Self {
        History {
            visits: load_list("history.json", "visits"),
        }
    }

    fn save(&self) {
        save_list("history.json", "visits", &self.visits);
    }

    /// Record a visit. Reloading or bouncing within a page must not fill the
    /// list with the same URL, so a repeat of the newest entry only refreshes
    /// its title and timestamp.
    pub fn record(&mut self, url: &str, title: &str) {
        if !is_recordable(url) {
            return;
        }
        match self.visits.first_mut() {
            Some(top) if top.url == url => {
                top.ts = now_unix();
                if !title.is_empty() {
                    top.title = title.to_string();
                }
            }
            _ => self.visits.insert(
                0,
                Entry {
                    url: url.to_string(),
                    title: title.to_string(),
                    ts: now_unix(),
                },
            ),
        }
        self.visits.truncate(HISTORY_CAP);
        self.save();
    }

    /// Late title updates: a page's <title> usually arrives after its URL, so
    /// the entry is created bare and filled in on a later tick.
    pub fn retitle(&mut self, url: &str, title: &str) {
        if title.is_empty() {
            return;
        }
        if let Some(top) = self.visits.first_mut() {
            if top.url == url && top.title != title {
                top.title = title.to_string();
                self.save();
            }
        }
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.visits.len() {
            self.visits.remove(i);
            self.save();
        }
    }

    pub fn clear(&mut self) {
        self.visits.clear();
        self.save();
    }
}

#[derive(Default)]
pub struct Bookmarks {
    pub items: Vec<Entry>,
}

impl Bookmarks {
    pub fn load() -> Self {
        Bookmarks {
            items: load_list("bookmarks.json", "items"),
        }
    }

    fn save(&self) {
        save_list("bookmarks.json", "items", &self.items);
    }

    pub fn contains(&self, url: &str) -> bool {
        self.items.iter().any(|b| b.url == url)
    }

    /// Add the page, or drop it if already bookmarked. Returns whether it is
    /// bookmarked afterwards, for the confirmation message.
    pub fn toggle(&mut self, url: &str, title: &str) -> bool {
        if !is_recordable(url) {
            return false;
        }
        match self.items.iter().position(|b| b.url == url) {
            Some(i) => {
                self.items.remove(i);
                self.save();
                false
            }
            None => {
                self.items.insert(
                    0,
                    Entry {
                        url: url.to_string(),
                        title: title.to_string(),
                        ts: now_unix(),
                    },
                );
                self.save();
                true
            }
        }
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.items.len() {
            self.items.remove(i);
            self.save();
        }
    }
}

/// Privacy choices. Each one is enforced by the engine or by this module —
/// nothing here is decorative.
#[derive(Clone, Copy, PartialEq)]
pub struct Settings {
    /// Ephemeral session: no history recording, and the engine gets a
    /// non-persistent data store so cookies die with the window.
    pub private: bool,
    /// Whether the engine stores cookies at all.
    pub accept_cookies: bool,
    /// Wipe cookies and cache when the last browser window closes.
    pub clear_on_exit: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            private: false,
            accept_cookies: true,
            clear_on_exit: false,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let Some(v) = read("settings.json") else {
            return Settings::default();
        };
        let d = Settings::default();
        let b = |k: &str, fallback: bool| v.get(k).and_then(|x| x.as_bool()).unwrap_or(fallback);
        Settings {
            private: b("private", d.private),
            accept_cookies: b("accept_cookies", d.accept_cookies),
            clear_on_exit: b("clear_on_exit", d.clear_on_exit),
        }
    }

    pub fn save(&self) {
        write(
            "settings.json",
            &json!({
                "private": self.private,
                "accept_cookies": self.accept_cookies,
                "clear_on_exit": self.clear_on_exit,
            }),
        );
    }
}

/// Whether a URL is worth remembering. The start page and inline documents
/// are not places you can navigate back to.
pub fn is_recordable(url: &str) -> bool {
    !url.is_empty()
        && !url.starts_with("about:")
        && !url.starts_with("data:")
        && url != "about:blank"
}

/// Host part of a URL, for compact list rows.
pub fn host_of(url: &str) -> &str {
    let rest = url
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.split('@').next_back().unwrap_or(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str) -> Entry {
        Entry {
            url: url.into(),
            title: String::new(),
            ts: 0,
        }
    }

    #[test]
    fn revisiting_the_newest_page_does_not_duplicate_it() {
        let mut h = History::default();
        h.visits.insert(0, entry("https://a.example"));
        h.record("https://a.example", "A");
        assert_eq!(h.visits.len(), 1);
        assert_eq!(h.visits[0].title, "A");
    }

    #[test]
    fn coming_back_to_an_older_page_is_a_new_visit() {
        let mut h = History::default();
        h.visits.insert(0, entry("https://a.example"));
        h.visits.insert(0, entry("https://b.example"));
        h.record("https://a.example", "");
        assert_eq!(h.visits.len(), 3);
        assert_eq!(h.visits[0].url, "https://a.example");
    }

    #[test]
    fn the_start_page_is_never_recorded() {
        let mut h = History::default();
        for url in ["", "about:blank", "data:text/html,<p>hi", "about:srcdoc"] {
            h.record(url, "");
        }
        assert!(h.visits.is_empty());
    }

    #[test]
    fn bookmarking_twice_removes_the_bookmark() {
        let mut b = Bookmarks::default();
        assert!(b.toggle("https://a.example", "A"));
        assert!(b.contains("https://a.example"));
        assert!(!b.toggle("https://a.example", "A"));
        assert!(!b.contains("https://a.example"));
    }

    #[test]
    fn entries_survive_a_json_round_trip() {
        let e = Entry {
            url: "https://a.example/x?y=1".into(),
            title: "Some “quoted” title".into(),
            ts: 1_700_000_000,
        };
        assert_eq!(Entry::from_json(&e.to_json()).unwrap(), e);
    }

    #[test]
    fn an_entry_without_a_url_is_dropped() {
        assert!(Entry::from_json(&json!({"title": "x"})).is_none());
        assert!(Entry::from_json(&json!({"url": ""})).is_none());
    }

    #[test]
    fn host_is_pulled_out_of_any_shape_of_url() {
        for (url, want) in [
            ("https://news.ycombinator.com/item?id=1", "news.ycombinator.com"),
            ("http://example.org", "example.org"),
            ("https://user:pw@example.org/p", "example.org"),
            ("example.org/path", "example.org"),
            ("https://example.org#frag", "example.org"),
        ] {
            assert_eq!(host_of(url), want, "url {url}");
        }
    }
}
