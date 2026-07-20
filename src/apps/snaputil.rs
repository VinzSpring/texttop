//! Local file snapshot store + text diff helpers.
//!
//! Snapshots live under `~/.texttop_snap/`:
//!   index.json  — path → list of snapshot metadata
//!   blobs/<id>  — raw file bytes

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, Utc};
use serde_json::{json, Value};

const INDEX_NAME: &str = "index.json";
const BLOBS_DIR: &str = "blobs";

#[derive(Clone, Debug)]
pub struct SnapMeta {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub ts_unix: i64,
    pub size: u64,
    pub hash: String,
    pub note: String,
}

impl SnapMeta {
    pub fn when_local(&self) -> String {
        DateTime::<Utc>::from_timestamp(self.ts_unix, 0)
            .map(|u| u.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| self.ts_unix.to_string())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Add,
    Del,
    Header,
}

#[derive(Clone)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

fn store_root() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(".texttop_snap"))
}

fn ensure_store() -> Result<PathBuf, String> {
    let root = store_root().ok_or_else(|| "no home directory".to_string())?;
    fs::create_dir_all(root.join(BLOBS_DIR)).map_err(|e| e.to_string())?;
    Ok(root)
}

fn canonical_key(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn content_hash(bytes: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    bytes.len().hash(&mut h);
    format!("{:016x}", h.finish())
}

fn load_index(root: &Path) -> Value {
    let p = root.join(INDEX_NAME);
    match fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|_| json!({ "files": {} })),
        Err(_) => json!({ "files": {} }),
    }
}

fn save_index(root: &Path, index: &Value) -> Result<(), String> {
    let p = root.join(INDEX_NAME);
    let s = serde_json::to_string_pretty(index).map_err(|e| e.to_string())?;
    let mut f = fs::File::create(&p).map_err(|e| e.to_string())?;
    f.write_all(s.as_bytes()).map_err(|e| e.to_string())
}

fn meta_from_value(path: &Path, v: &Value) -> Option<SnapMeta> {
    Some(SnapMeta {
        id: v.get("id")?.as_str()?.to_string(),
        path: path.to_path_buf(),
        name: v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        ts_unix: v.get("ts")?.as_i64()?,
        size: v.get("size")?.as_u64()?,
        hash: v
            .get("hash")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        note: v
            .get("note")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// Create a snapshot of `path`. Returns the new metadata.
pub fn create_snapshot(path: &Path, note: &str) -> Result<SnapMeta, String> {
    if !path.is_file() {
        return Err("not a file".into());
    }
    let root = ensure_store()?;
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let hash = content_hash(&bytes);
    let ts = now_unix();
    let id = format!("{ts}_{}", &hash[..8.min(hash.len())]);
    let blob = root.join(BLOBS_DIR).join(&id);
    if !blob.exists() {
        fs::write(&blob, &bytes).map_err(|e| e.to_string())?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let key = canonical_key(path);
    let meta = SnapMeta {
        id: id.clone(),
        path: PathBuf::from(&key),
        name: name.clone(),
        ts_unix: ts,
        size: bytes.len() as u64,
        hash: hash.clone(),
        note: note.trim().to_string(),
    };

    let mut index = load_index(&root);
    let files = index
        .as_object_mut()
        .ok_or_else(|| "bad index".to_string())?
        .entry("files")
        .or_insert_with(|| json!({}));
    let arr = files
        .as_object_mut()
        .ok_or_else(|| "bad files map".to_string())?
        .entry(key.clone())
        .or_insert_with(|| json!([]));
    let list = arr.as_array_mut().ok_or_else(|| "bad list".to_string())?;
    // Skip duplicate if identical to latest
    if let Some(last) = list.last() {
        if last.get("hash").and_then(|h| h.as_str()) == Some(hash.as_str()) {
            return Err("unchanged since last snapshot".into());
        }
    }
    list.push(json!({
        "id": id,
        "name": name,
        "ts": ts,
        "size": bytes.len() as u64,
        "hash": hash,
        "note": note.trim(),
    }));
    save_index(&root, &index)?;
    Ok(meta)
}

pub fn create_many(paths: &[PathBuf]) -> (usize, usize, String) {
    let mut ok = 0usize;
    let mut fail = 0usize;
    let mut last_err = String::new();
    for p in paths {
        match create_snapshot(p, "") {
            Ok(_) => ok += 1,
            Err(e) => {
                fail += 1;
                last_err = e;
            }
        }
    }
    let msg = if fail == 0 {
        format!("snapshotted {ok} file(s)")
    } else if ok == 0 {
        format!("snapshot failed: {last_err}")
    } else {
        format!("snapshotted {ok}, {fail} failed ({last_err})")
    };
    (ok, fail, msg)
}

pub fn history(path: &Path) -> Vec<SnapMeta> {
    let Some(root) = store_root() else {
        return Vec::new();
    };
    let index = load_index(&root);
    let key = canonical_key(path);
    let Some(arr) = index
        .get("files")
        .and_then(|f| f.get(&key))
        .and_then(|a| a.as_array())
    else {
        return Vec::new();
    };
    let mut out: Vec<SnapMeta> = arr
        .iter()
        .filter_map(|v| meta_from_value(Path::new(&key), v))
        .collect();
    out.sort_by(|a, b| b.ts_unix.cmp(&a.ts_unix));
    out
}

pub fn count_for(path: &Path) -> usize {
    history(path).len()
}

pub fn read_blob(id: &str) -> Result<Vec<u8>, String> {
    let root = store_root().ok_or_else(|| "no home directory".to_string())?;
    fs::read(root.join(BLOBS_DIR).join(id)).map_err(|e| e.to_string())
}

pub fn read_blob_text(id: &str) -> Result<String, String> {
    let bytes = read_blob(id)?;
    if looks_binary(&bytes) {
        return Err(format!("binary snapshot ({})", crate_fmt_size(bytes.len() as u64)));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn restore(path: &Path, id: &str) -> Result<(), String> {
    let bytes = read_blob(id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Safety copy of current file before overwrite
    if path.is_file() {
        let _ = create_snapshot(path, "auto before restore");
    }
    fs::write(path, bytes).map_err(|e| e.to_string())
}

pub fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let sample = &bytes[..bytes.len().min(8192)];
    if sample.contains(&0) {
        return true;
    }
    let non_text = sample
        .iter()
        .filter(|&&b| b < 9 || (b > 13 && b < 32) || b == 0x7F)
        .count();
    non_text * 10 > sample.len()
}

fn crate_fmt_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

pub fn read_path_text(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    if looks_binary(&bytes) {
        return Err(format!(
            "binary file ({})",
            crate_fmt_size(bytes.len() as u64)
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Unified-style line list for colored rendering.
pub fn diff_texts(a_label: &str, a: &str, b_label: &str, b: &str) -> Vec<DiffLine> {
    let a_lines: Vec<&str> = a.lines().collect();
    let b_lines: Vec<&str> = b.lines().collect();
    let mut out = vec![
        DiffLine {
            kind: DiffKind::Header,
            text: format!("--- {a_label}"),
        },
        DiffLine {
            kind: DiffKind::Header,
            text: format!("+++ {b_label}"),
        },
    ];
    let edits = lcs_diff(&a_lines, &b_lines);
    let mut hunk_buf: Vec<DiffLine> = Vec::new();
    let mut a_line = 1usize;
    let mut b_line = 1usize;
    let mut hunk_a0 = 1usize;
    let mut hunk_b0 = 1usize;
    let mut a_count = 0usize;
    let mut b_count = 0usize;
    let mut in_hunk = false;
    let mut trail_eq = 0usize;
    let mut ctx_pending: Vec<DiffLine> = Vec::new();

    let mut flush = |hunk_buf: &mut Vec<DiffLine>,
                     out: &mut Vec<DiffLine>,
                     hunk_a0: usize,
                     a_count: usize,
                     hunk_b0: usize,
                     b_count: usize| {
        if hunk_buf.is_empty() {
            return;
        }
        out.push(DiffLine {
            kind: DiffKind::Header,
            text: format!("@@ -{hunk_a0},{a_count} +{hunk_b0},{b_count} @@"),
        });
        out.append(hunk_buf);
    };

    for e in edits {
        match e {
            Edit::Equal(s) => {
                let line = DiffLine {
                    kind: DiffKind::Context,
                    text: format!(" {s}"),
                };
                if in_hunk {
                    hunk_buf.push(line);
                    a_count += 1;
                    b_count += 1;
                    trail_eq += 1;
                    if trail_eq >= 3 {
                        flush(
                            &mut hunk_buf,
                            &mut out,
                            hunk_a0,
                            a_count,
                            hunk_b0,
                            b_count,
                        );
                        in_hunk = false;
                        a_count = 0;
                        b_count = 0;
                        trail_eq = 0;
                        ctx_pending.clear();
                    }
                } else {
                    ctx_pending.push(line);
                    if ctx_pending.len() > 3 {
                        ctx_pending.remove(0);
                    }
                }
                a_line += 1;
                b_line += 1;
            }
            Edit::Delete(s) => {
                if !in_hunk {
                    hunk_a0 = a_line.saturating_sub(ctx_pending.len());
                    hunk_b0 = b_line.saturating_sub(ctx_pending.len());
                    a_count = ctx_pending.len();
                    b_count = ctx_pending.len();
                    hunk_buf.extend(ctx_pending.drain(..));
                    in_hunk = true;
                }
                trail_eq = 0;
                hunk_buf.push(DiffLine {
                    kind: DiffKind::Del,
                    text: format!("-{s}"),
                });
                a_count += 1;
                a_line += 1;
            }
            Edit::Insert(s) => {
                if !in_hunk {
                    hunk_a0 = a_line.saturating_sub(ctx_pending.len());
                    hunk_b0 = b_line.saturating_sub(ctx_pending.len());
                    a_count = ctx_pending.len();
                    b_count = ctx_pending.len();
                    hunk_buf.extend(ctx_pending.drain(..));
                    in_hunk = true;
                }
                trail_eq = 0;
                hunk_buf.push(DiffLine {
                    kind: DiffKind::Add,
                    text: format!("+{s}"),
                });
                b_count += 1;
                b_line += 1;
            }
        }
    }
    if in_hunk {
        flush(
            &mut hunk_buf,
            &mut out,
            hunk_a0,
            a_count,
            hunk_b0,
            b_count,
        );
    }
    if out.len() == 2 {
        out.push(DiffLine {
            kind: DiffKind::Context,
            text: " (identical)".into(),
        });
    }
    out
}

pub fn diff_files(a: &Path, b: &Path) -> Result<Vec<DiffLine>, String> {
    let at = read_path_text(a)?;
    let bt = read_path_text(b)?;
    let al = a
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| a.display().to_string());
    let bl = b
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| b.display().to_string());
    Ok(diff_texts(&al, &at, &bl, &bt))
}

enum Edit<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

/// Myers-ish LCS backtrack for small/medium files.
fn lcs_diff<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<Edit<'a>> {
    let n = a.len();
    let m = b.len();
    // Cap for pathological size — fall back to naive replace
    if n * m > 2_000_000 {
        let mut out = Vec::new();
        for s in a {
            out.push(Edit::Delete(s));
        }
        for s in b {
            out.push(Edit::Insert(s));
        }
        return out;
    }
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in 0..n {
        for j in 0..m {
            if a[i] == b[j] {
                dp[i + 1][j + 1] = dp[i][j] + 1;
            } else {
                dp[i + 1][j + 1] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }
    let mut out = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            out.push(Edit::Equal(a[i - 1]));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            out.push(Edit::Insert(b[j - 1]));
            j -= 1;
        } else {
            out.push(Edit::Delete(a[i - 1]));
            i -= 1;
        }
    }
    out.reverse();
    out
}
