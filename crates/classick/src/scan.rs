//! `--scan-library` mode: refresh library-index.json from the source tree.
//! Progress rides the existing IPC event vocabulary (summary / track_start /
//! track_done / finish) so the daemon's forwarding and the UI's progress
//! rendering need no new machinery.

use crate::apply_loop::RunOutcome;
use crate::config::Config;
use crate::library_index::{self, UpdateStats};
use crate::progress::Progress;
use crate::source;
use anyhow::Result;
use std::path::Path;

pub fn run(config: &Config, progress: &Progress) -> Result<RunOutcome> {
    let index_path = library_index::default_index_path()?;
    progress.header(
        config.source.display().to_string(),
        String::new(), // no iPod involved in a scan
        index_path.display().to_string(),
    );
    let stats = scan_with(&config.source, &index_path, |current, total, path| {
        progress.track_start(
            current,
            total,
            path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        );
        progress.track_done();
    }, |walked, to_probe| {
        // summary: reuse the wire's existing shape; only total_planned
        // matters to progress consumers ("Scanning… X of N changed files").
        progress.summary(to_probe, 0, 0, 0, walked - to_probe, to_probe);
        progress.log(format!("scan: {walked} file(s) walked, {to_probe} changed/new to read"));
    })?;
    progress.log(format!(
        "scan: probed={} reused={} dropped={} failed={}",
        stats.probed, stats.reused, stats.dropped, stats.failed));
    Ok(RunOutcome::Completed)
}

/// Progress-free entry used by tests: walk + incremental update + save.
pub fn scan_source(source_root: &Path, index_path: &Path) -> Result<UpdateStats> {
    scan_with(source_root, index_path, |_, _, _| {}, |_, _| {})
}

/// Shared core: walk, size the probe set, incremental update, stamp
/// scanned_at, atomic save. `on_plan(walked, to_probe)` fires once before
/// probing starts; `on_progress` fires per probed file.
fn scan_with(
    source_root: &Path,
    index_path: &Path,
    on_progress: impl FnMut(usize, usize, &Path),
    on_plan: impl FnOnce(usize, usize),
) -> Result<UpdateStats> {
    let entries = source::walk(source_root)?;
    let mut index = library_index::load_or_empty(index_path, source_root);
    let to_probe = library_index::stale_entries(&index, &entries).len();
    on_plan(entries.len(), to_probe);
    let stats = library_index::update_index(
        &mut index, &entries, library_index::read_track_tags, on_progress);
    index.scanned_at_unix_secs = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    library_index::save_atomic(index_path, &index)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_writes_index_and_stamps_scanned_at() {
        let base = std::env::temp_dir().join(format!("classick-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src_dir = base.join("music");
        std::fs::create_dir_all(&src_dir).unwrap();
        // Plain non-audio bytes: lofty will fail to parse -> unknown bucket,
        // which is exactly the skip-don't-abort behavior we want covered
        // without an ffmpeg dependency in this test.
        std::fs::write(src_dir.join("a.flac"), b"not really flac").unwrap();

        let index_path = base.join("library-index.json");
        let stats = scan_source(&src_dir, &index_path).unwrap();
        assert_eq!(stats.probed, 1);
        let idx = crate::library_index::load_or_empty(&index_path, &src_dir);
        assert!(idx.scanned_at_unix_secs.is_some(), "completed scan must stamp scanned_at");
        assert_eq!(idx.files.len(), 1);

        // Second scan: stat-only, nothing probed.
        let stats2 = scan_source(&src_dir, &index_path).unwrap();
        assert_eq!(stats2.probed, 0);
        assert_eq!(stats2.reused, 1);
        let _ = std::fs::remove_dir_all(&base);
    }
}
