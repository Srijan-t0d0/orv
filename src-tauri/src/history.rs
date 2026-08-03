//! Durable transcript history.
//!
//! Every finished utterance is appended to history.jsonl in the config dir;
//! the last N entries are loaded on startup so the transcript view survives
//! restarts. Append-only JSONL keeps writes atomic-enough (one line per
//! event) and makes the file greppable/exportable by the user.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Keep the file from growing unbounded: when it exceeds this many lines,
/// rewrite it with the newest half.
const MAX_LINES: usize = 2000;
/// How many entries the UI gets on startup.
const LOAD_LIMIT: usize = 100;

#[derive(Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Unix millis when the transcript completed.
    pub at: u64,
    pub text: String,
    pub total_ms: u64,
    pub api_ms: u64,
    pub audio_secs: f32,
    pub model: String,
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("openroutervoice").join("history.jsonl"))
}

/// Append one entry (best-effort; history must never break dictation).
pub fn append(entry: &HistoryEntry) {
    let Some(path) = path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(line) = serde_json::to_string(entry) else { return };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
    compact_if_needed(&path);
}

/// Load the most recent entries, oldest first (ready to render top-down).
pub fn load_recent() -> Vec<HistoryEntry> {
    let Some(path) = path() else { return Vec::new() };
    let Ok(raw) = fs::read_to_string(&path) else { return Vec::new() };
    let mut entries: Vec<HistoryEntry> = raw
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if entries.len() > LOAD_LIMIT {
        entries.drain(..entries.len() - LOAD_LIMIT);
    }
    entries
}

fn compact_if_needed(path: &PathBuf) {
    let Ok(raw) = fs::read_to_string(path) else { return };
    let lines: Vec<&str> = raw.lines().collect();
    if lines.len() > MAX_LINES {
        let keep = &lines[lines.len() - MAX_LINES / 2..];
        let _ = fs::write(path, format!("{}\n", keep.join("\n")));
    }
}
