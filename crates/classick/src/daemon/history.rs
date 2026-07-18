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
    /// Tracks whose tags/art were rewritten in place (no audio re-transcode).
    /// These ARE in the source and ARE already on the iPod, so they must be
    /// counted into `library_count` alongside add/modify/unchanged — omitting
    /// them made the daemon's cached total undercount, so "X of Y synced"
    /// could show X > Y after a tag-only sync. `#[serde(default)]` keeps
    /// deserialization of pre-existing `history.json` entries backward
    /// compatible (they predate this field).
    #[serde(default)]
    pub metadata_only: usize,
    /// Whole-album fit-pass deferral rollup (Task 8's `skipped_for_space` on
    /// the subprocess `finish` event) — tracks/bytes across every album that
    /// still didn't fit the device's remaining space after the end-of-run
    /// retry. The `albums` count on the wire event is deliberately NOT
    /// persisted here (per plan). `#[serde(default)]` keeps pre-existing
    /// `history.json` entries (written before this field existed)
    /// deserializing cleanly to zero.
    #[serde(default)]
    pub skipped_for_space_tracks: usize,
    #[serde(default)]
    pub skipped_for_space_bytes: u64,
    /// `artwork.failed_sources` from the subprocess `finish` event.
    /// `#[serde(default)]` for the same backward-compat reason as above.
    #[serde(default)]
    pub artwork_failed_sources: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Empty only for legacy entries written before per-device history.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub serial: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
    pub timestamp: String,
    pub duration_secs: u64,
    pub trigger: SyncTrigger,
    pub outcome: SyncOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<SyncSummary>,
    /// `true` when the sync subprocess's `finish` event carried
    /// `db_restored: true` (Task 4's auto-restore-from-backup path fired
    /// this run). Omitted (not `false`) on the wire when it didn't fire,
    /// mirroring the subprocess wire field's old-client-compat convention;
    /// `#[serde(default)]` lets pre-existing `history.json` entries (written
    /// before this field existed) deserialize to `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub db_restored: bool,
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
        self.write_entries(existing)
    }

    /// Attach pre-registry history to the single configured device created by
    /// the legacy migration. Existing scoped entries are never rewritten.
    pub fn migrate_legacy_entries(&self, serial: &str) -> Result<()> {
        let mut entries = self.read();
        let mut changed = false;
        for entry in &mut entries {
            if entry.serial.is_empty() {
                entry.serial = serial.to_string();
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        self.write_entries(entries)
    }

    pub fn latest_attempt(&self, serial: &str) -> Option<HistoryEntry> {
        let key = crate::daemon::device_registry::canonical_serial_key(serial);
        self.read().into_iter().rev().find(|entry| {
            !entry.serial.is_empty()
                && crate::daemon::device_registry::canonical_serial_key(&entry.serial) == key
        })
    }

    pub fn latest_success(&self, serial: &str) -> Option<HistoryEntry> {
        let key = crate::daemon::device_registry::canonical_serial_key(serial);
        self.read().into_iter().rev().find(|entry| {
            entry.outcome == SyncOutcome::Ok
                && !entry.serial.is_empty()
                && crate::daemon::device_registry::canonical_serial_key(&entry.serial) == key
        })
    }

    fn write_entries(&self, existing: Vec<HistoryEntry>) -> Result<()> {
        let start = existing.len().saturating_sub(MAX_ENTRIES);
        let trimmed: Vec<_> = existing.into_iter().skip(start).collect();
        let file = HistoryFile { version: 1, entries: trimmed };

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&file).context("serialize history")?;
        {
            let file =
                std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
            let mut writer = std::io::BufWriter::new(file);
            use std::io::Write;
            writer
                .write_all(text.as_bytes())
                .with_context(|| format!("write {}", tmp.display()))?;
            let file = writer.into_inner().context("flush history writer")?;
            file.sync_all()
                .with_context(|| format!("fsync {}", tmp.display()))?;
        }
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
            serial: String::new(),
            session_id: None,
            timestamp: ts.to_string(),
            duration_secs: 5,
            trigger: SyncTrigger::Manual,
            outcome,
            error_message: None,
            summary: Some(SyncSummary {
                add: 1, modify: 0, remove: 0, unchanged: 0, skipped: 0, metadata_only: 0,
                skipped_for_space_tracks: 0, skipped_for_space_bytes: 0, artwork_failed_sources: 0,
            }),
            db_restored: false,
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
            serial: String::new(),
            session_id: None,
            timestamp: "2026-05-25T10:00:00Z".to_string(),
            duration_secs: 7,
            trigger: SyncTrigger::PlugIn,
            outcome: SyncOutcome::Aborted,
            error_message: Some("too_many_failures: 6 of 10 tracks failed".to_string()),
            summary: None,
            db_restored: false,
        };
        svc.append(entry.clone()).unwrap();
        let read_back = svc.read();
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].outcome, SyncOutcome::Aborted);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn sync_summary_without_metadata_only_field_defaults_to_zero() {
        // Pre-existing history.json entries were written before `metadata_only`
        // existed. `#[serde(default)]` must let them keep deserializing.
        let json = r#"{"add":1,"modify":2,"remove":0,"unchanged":10,"skipped":0}"#;
        let summary: SyncSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.metadata_only, 0);
    }

    #[test]
    fn sync_summary_new_fields_round_trip() {
        let summary = SyncSummary {
            add: 1, modify: 2, remove: 0, unchanged: 10, skipped: 0, metadata_only: 3,
            skipped_for_space_tracks: 5, skipped_for_space_bytes: 123_456_789,
            artwork_failed_sources: 2,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let round_tripped: SyncSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, summary);
        assert_eq!(round_tripped.skipped_for_space_tracks, 5);
        assert_eq!(round_tripped.skipped_for_space_bytes, 123_456_789);
        assert_eq!(round_tripped.artwork_failed_sources, 2);
    }

    #[test]
    fn sync_summary_without_new_fields_defaults_to_zero() {
        // Pre-existing history.json entries were written before the
        // skipped-for-space / artwork fields existed.
        let json = r#"{"add":1,"modify":2,"remove":0,"unchanged":10,"skipped":0,"metadata_only":0}"#;
        let summary: SyncSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.skipped_for_space_tracks, 0);
        assert_eq!(summary.skipped_for_space_bytes, 0);
        assert_eq!(summary.artwork_failed_sources, 0);
    }

    #[test]
    fn history_entry_db_restored_round_trips() {
        let mut entry = make_entry("2026-07-17T10:00:00Z", SyncOutcome::Ok);
        entry.db_restored = true;
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains(r#""db_restored":true"#));
        let round_tripped: HistoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, entry);
        assert!(round_tripped.db_restored);
    }

    #[test]
    fn history_entry_without_db_restored_field_defaults_to_false() {
        // Pre-existing history.json entries were written before `db_restored`
        // existed on HistoryEntry.
        let json = r#"{"timestamp":"2026-07-17T10:00:00Z","duration_secs":5,"trigger":"manual","outcome":"ok"}"#;
        let entry: HistoryEntry = serde_json::from_str(json).unwrap();
        assert!(!entry.db_restored);
    }

    #[test]
    fn history_entry_db_restored_omitted_from_wire_when_false() {
        // Old-client compat, mirroring the subprocess wire field's convention:
        // absent (not `false`) when it didn't fire.
        let entry = make_entry("2026-07-17T10:00:00Z", SyncOutcome::Ok);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("db_restored"));
    }

    #[test]
    fn migrates_unscoped_legacy_history_to_configured_serial() {
        let p = tmp_path("migrate-legacy-serial");
        let _ = std::fs::remove_file(&p);
        let svc = HistoryService::new(p.clone());
        svc.append(make_entry("2026-07-18T10:00:00Z", SyncOutcome::Ok))
            .unwrap();
        svc.append(make_entry("2026-07-18T10:01:00Z", SyncOutcome::Error))
            .unwrap();

        svc.migrate_legacy_entries(" A ").unwrap();

        let entries = svc.read();
        assert!(entries.iter().all(|entry| entry.serial == " A "));
        assert!(entries.iter().all(|entry| entry.session_id.is_none()));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn latest_success_ignores_newer_failed_or_cancelled_attempts() {
        let p = tmp_path("latest-success");
        let _ = std::fs::remove_file(&p);
        let svc = HistoryService::new(p.clone());
        for (timestamp, outcome) in [
            ("2026-07-18T10:00:00Z", SyncOutcome::Ok),
            ("2026-07-18T10:01:00Z", SyncOutcome::Error),
            ("2026-07-18T10:02:00Z", SyncOutcome::Aborted),
        ] {
            let mut entry = make_entry(timestamp, outcome);
            entry.serial = "A".to_string();
            svc.append(entry).unwrap();
        }

        assert_eq!(
            svc.latest_attempt(" a ").unwrap().timestamp,
            "2026-07-18T10:02:00Z"
        );
        assert_eq!(
            svc.latest_success("A").unwrap().timestamp,
            "2026-07-18T10:00:00Z"
        );
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
