//! Filesystem watcher for the configured source library. On any change under
//! the source root it emits a coalesced tick; the runtime debounces those and
//! triggers the existing (crash-isolated, incremental) scan subprocess. A
//! sibling to `device_watcher` — same "background source → mpsc → runtime
//! select arm" shape.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// Owns the `notify` watcher. Dropping it stops the OS watch. `rewatch`
/// re-points it when the configured source changes.
pub struct LibraryWatcher {
    watcher: Option<RecommendedWatcher>,
    current: Option<PathBuf>,
    tx: UnboundedSender<()>,
}

impl LibraryWatcher {
    /// Start watching `source` (if any). Returns the watcher handle plus the
    /// receiver of change ticks. The `notify` callback runs on the crate's own
    /// thread; it forwards one tick per notify event onto the tokio channel
    /// (the runtime does the time-based debounce / coalescing).
    pub fn spawn(source: Option<PathBuf>) -> (Self, UnboundedReceiver<()>) {
        let (tx, rx) = mpsc::unbounded_channel::<()>();
        let mut me = Self { watcher: None, current: None, tx };
        me.rewatch(source);
        (me, rx)
    }

    /// Re-point the watch at `source` (or stop watching when `None`). Idempotent
    /// when the path is unchanged.
    pub fn rewatch(&mut self, source: Option<PathBuf>) {
        // self.current is the path currently ARMED (successfully watched); None
        // means nothing is armed. So a no-op only when we're already watching the
        // requested path (or both are None).
        if source == self.current {
            return;
        }
        // Tear down any existing watch, and treat ourselves as un-armed until a
        // new watch actually succeeds below — so a failed attempt never latches
        // self.current and a later retry with the same path re-attempts.
        self.watcher = None;
        self.current = None;
        let Some(path) = source else { return }; // disarm request; stays None
        if !path.exists() {
            tracing::warn!("library_watcher: source {} does not exist; not watching", path.display());
            return;
        }
        let tx = self.tx.clone();
        let mut watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            match res {
                // Any event is just a "something changed" nudge — the scan
                // itself diffs mtime/size, so we don't inspect the event kind.
                Ok(_) => { let _ = tx.send(()); }
                Err(e) => tracing::warn!("library_watcher: notify error: {e}"),
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("library_watcher: failed to create watcher: {e}");
                return;
            }
        };
        if let Err(e) = watcher.watch(&path, RecursiveMode::Recursive) {
            tracing::warn!("library_watcher: failed to watch {}: {e}", path.display());
            return;
        }
        tracing::info!("library_watcher: watching {}", path.display());
        self.watcher = Some(watcher);
        self.current = Some(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn touching_a_file_delivers_a_change_tick() {
        let dir = std::env::temp_dir().join(format!("classick-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_watcher, mut rx) = LibraryWatcher::spawn(Some(dir.clone()));

        // Give the OS watch a beat to arm, then create a file.
        tokio::time::sleep(Duration::from_millis(200)).await;
        std::fs::write(dir.join("new.flac"), b"x").unwrap();

        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(matches!(got, Ok(Some(()))), "expected a change tick, got {got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn no_source_yields_no_ticks() {
        let (_watcher, mut rx) = LibraryWatcher::spawn(None);
        let got = tokio::time::timeout(Duration::from_millis(400), rx.recv()).await;
        assert!(got.is_err(), "no watched path → no ticks (timeout expected)");
    }

    #[tokio::test]
    async fn rewatch_after_failed_attempt_still_arms() {
        // Guards the state bug: a failed rewatch (nonexistent path) must not
        // latch the watcher into a permanently-un-armed state. Task 3 calls
        // rewatch on every config save, so a transient bad path can't wedge it.
        let dir = std::env::temp_dir().join(format!("classick-rewatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let nonexistent = dir.join("does-not-exist-subdir");

        let (mut watcher, mut rx) = LibraryWatcher::spawn(None);
        // First: a rewatch that fails (path doesn't exist).
        watcher.rewatch(Some(nonexistent));
        // Then: a rewatch onto the real dir must still arm the watch.
        watcher.rewatch(Some(dir.clone()));

        tokio::time::sleep(Duration::from_millis(200)).await;
        std::fs::write(dir.join("new.flac"), b"x").unwrap();

        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(matches!(got, Ok(Some(()))), "expected a change tick after re-arm, got {got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
