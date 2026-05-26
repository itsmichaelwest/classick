//! Persistent log of past sync operations. Backed by a small JSON file
//! at `%LOCALAPPDATA%\classick\history.json`. Cap at 50 entries (oldest
//! evicted). Corrupt file is renamed to `.bak-{unix_secs}` and a fresh
//! file is started.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTrigger {
    PlugIn,
    Scheduled,
    Manual,
    Coalesced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOutcome {
    Ok,
    Error,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSummary {
    pub add: usize,
    pub modify: usize,
    pub remove: usize,
    pub unchanged: usize,
    #[serde(default)]
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: String,
    pub duration_secs: u64,
    pub trigger: SyncTrigger,
    pub outcome: SyncOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<SyncSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryFile {
    pub version: u32,
    pub entries: Vec<HistoryEntry>,
}

impl Default for HistoryFile {
    fn default() -> Self { Self { version: 1, entries: Vec::new() } }
}

const MAX_ENTRIES: usize = 50;

pub struct HistoryService {
    path: PathBuf,
}

impl HistoryService {
    pub fn new(path: PathBuf) -> Self { Self { path } }

    pub fn read(&self) -> Vec<HistoryEntry> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => match serde_json::from_str::<HistoryFile>(&text) {
                Ok(f) => f.entries,
                Err(_) => {
                    self.rename_corrupt_file();
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        }
    }

    pub fn append(&self, entry: HistoryEntry) -> Result<()> {
        let mut existing = self.read();
        existing.push(entry);
        let start = existing.len().saturating_sub(MAX_ENTRIES);
        let trimmed: Vec<_> = existing.into_iter().skip(start).collect();
        let file = HistoryFile { version: 1, entries: trimmed };

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&file).context("serialize history")?;
        std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), self.path.display()))?;
        Ok(())
    }

    fn rename_corrupt_file(&self) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let bak = self.path.with_extension(format!("json.bak-{ts}"));
        let _ = std::fs::rename(&self.path, &bak);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("classick-history-test-{}-{}.json",
            name, std::process::id()))
    }

    fn make_entry(ts: &str, outcome: SyncOutcome) -> HistoryEntry {
        HistoryEntry {
            timestamp: ts.to_string(),
            duration_secs: 5,
            trigger: SyncTrigger::Manual,
            outcome,
            error_message: None,
            summary: Some(SyncSummary { add: 1, modify: 0, remove: 0, unchanged: 0, skipped: 0 }),
        }
    }

    #[test]
    fn read_missing_file_returns_empty() {
        let p = tmp_path("read-missing");
        let _ = std::fs::remove_file(&p);
        let svc = HistoryService::new(p);
        assert!(svc.read().is_empty());
    }

    #[test]
    fn append_then_read_round_trips() {
        let p = tmp_path("append-read");
        let _ = std::fs::remove_file(&p);
        let svc = HistoryService::new(p.clone());
        svc.append(make_entry("2026-05-24T10:00:00Z", SyncOutcome::Ok)).unwrap();
        let entries = svc.read();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp, "2026-05-24T10:00:00Z");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn append_caps_at_50_evicting_oldest() {
        let p = tmp_path("cap-50");
        let _ = std::fs::remove_file(&p);
        let svc = HistoryService::new(p.clone());
        for i in 0..55 {
            let ts = format!("2026-05-24T10:{:02}:00Z", i);
            svc.append(make_entry(&ts, SyncOutcome::Ok)).unwrap();
        }
        let entries = svc.read();
        assert_eq!(entries.len(), 50);
        assert_eq!(entries[0].timestamp, "2026-05-24T10:05:00Z");
        assert_eq!(entries[49].timestamp, "2026-05-24T10:54:00Z");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn aborted_outcome_round_trips() {
        let p = tmp_path("aborted");
        let _ = std::fs::remove_file(&p);
        let svc = HistoryService::new(p.clone());
        let entry = HistoryEntry {
            timestamp: "2026-05-25T10:00:00Z".to_string(),
            duration_secs: 7,
            trigger: SyncTrigger::PlugIn,
            outcome: SyncOutcome::Aborted,
            error_message: Some("too_many_failures: 6 of 10 tracks failed".to_string()),
            summary: None,
        };
        svc.append(entry.clone()).unwrap();
        let read_back = svc.read();
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].outcome, SyncOutcome::Aborted);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_renamed_and_fresh_start() {
        let p = tmp_path("corrupt");
        std::fs::write(&p, "this isn't JSON at all { ]").unwrap();
        let svc = HistoryService::new(p.clone());
        let entries = svc.read();
        assert!(entries.is_empty(), "corrupt file should read as empty");
        assert!(!p.exists(), "corrupt original should have been renamed away");
        // Cleanup .bak files
        if let Some(dir) = p.parent() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(p.file_name().unwrap().to_string_lossy().as_ref())
                    && name.contains(".bak-")
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}
