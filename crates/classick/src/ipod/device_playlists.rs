//! Classick-managed playlist candidate reconciliation and best-effort host
//! playlist mirroring. Apple playlist mutation authority comes only from the
//! connected device ownership record and is always resolved by exact ID.

mod reconcile;

pub use reconcile::{
    reconcile_candidate, DesiredPlaylist, PlaylistDiagnostic, PlaylistReconcileOutcome,
    ReconcileStats,
};

use std::path::{Path, PathBuf};

/// Best-effort backup of playlist definitions and subscriptions onto the iPod.
/// This runs only after coordinated publication succeeds and never grants
/// authority to mutate an Apple playlist.
pub fn mirror_to_ipod(mount: &Path, playlists_root: &Path, subscriptions_path: &Path) {
    let dest_dir = crate::ipod::layout::playlists_mirror_dir(mount);
    if let Err(error) = std::fs::create_dir_all(&dest_dir) {
        tracing::warn!(
            "playlist mirror: failed to create {}: {error}",
            dest_dir.display()
        );
        return;
    }

    let mut entries = files_in(playlists_root).unwrap_or_else(|error| {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                "playlist mirror: failed to read {}: {error}",
                playlists_root.display()
            );
        }
        Vec::new()
    });
    entries.sort();
    let mut copied = 0;
    for source in entries {
        let Some(filename) = source.file_name() else {
            continue;
        };
        let destination = dest_dir.join(filename);
        match std::fs::copy(&source, &destination) {
            Ok(_) => copied += 1,
            Err(error) => tracing::warn!(
                "playlist mirror: failed to copy {} -> {}: {error}",
                source.display(),
                destination.display()
            ),
        }
    }
    if subscriptions_path.exists() {
        let destination = dest_dir.join("subscriptions.json");
        if let Err(error) = std::fs::copy(subscriptions_path, &destination) {
            tracing::warn!(
                "playlist mirror: failed to copy {} -> {}: {error}",
                subscriptions_path.display(),
                destination.display()
            );
        }
    }
    tracing::debug!(
        "playlist mirror: copied {copied} playlist file(s) to {}",
        dest_dir.display()
    );
}

/// Adopt a device mirror only when both local playlist artifacts are absent.
/// Files are copied independently and never overwrite local state.
pub fn adopt_from_ipod(mount: &Path, playlists_root: &Path, subscriptions_path: &Path) -> usize {
    if local_state_exists(playlists_root, subscriptions_path) {
        return 0;
    }
    let mirror_dir = crate::ipod::layout::playlists_mirror_dir(mount);
    let mut mirror_files = match files_in(&mirror_dir) {
        Ok(files) if !files.is_empty() => files,
        _ => return 0,
    };
    mirror_files.sort();
    if let Err(error) = std::fs::create_dir_all(playlists_root) {
        tracing::warn!(
            "playlist adopt: failed to create {}: {error}",
            playlists_root.display()
        );
        return 0;
    }

    let mut adopted = 0;
    for source in mirror_files {
        let Some(filename) = source.file_name() else {
            continue;
        };
        if filename == "subscriptions.json" {
            copy_subscriptions_if_absent(&source, subscriptions_path);
            continue;
        }
        let destination = playlists_root.join(filename);
        match std::fs::copy(&source, &destination) {
            Ok(_) => adopted += 1,
            Err(error) => tracing::warn!(
                "playlist adopt: failed to copy {} -> {}: {error}",
                source.display(),
                destination.display()
            ),
        }
    }
    if adopted > 0 {
        tracing::warn!("adopted {adopted} playlists from device mirror");
    }
    adopted
}

fn files_in(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    Ok(std::fs::read_dir(root)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect())
}

fn local_state_exists(playlists_root: &Path, subscriptions_path: &Path) -> bool {
    subscriptions_path.exists()
        || files_in(playlists_root)
            .map(|files| !files.is_empty())
            .unwrap_or(false)
}

fn copy_subscriptions_if_absent(source: &Path, destination: &Path) {
    if destination.exists() {
        return;
    }
    if let Some(parent) = destination.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                "playlist adopt: failed to create {}: {error}",
                parent.display()
            );
            return;
        }
    }
    if let Err(error) = std::fs::copy(source, destination) {
        tracing::warn!(
            "playlist adopt: failed to copy {} -> {}: {error}",
            source.display(),
            destination.display()
        );
    }
}
