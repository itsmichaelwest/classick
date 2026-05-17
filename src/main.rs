use anyhow::{anyhow, Context, Result};
use clap::Parser;
use ipod_sync::cli::Cli;
use ipod_sync::config::{self, Config};
use ipod_sync::ipod::db::{OwnedDb, Tags, TrackHandle};
use ipod_sync::ipod::{device, detect_ipod_mount};
use ipod_sync::manifest::{self, Action, Manifest, ManifestEntry};
use ipod_sync::progress::Progress;
use ipod_sync::source::{self, SourceEntry};
use ipod_sync::transcode::{self, has_embedded_art, ProbeOutput, ProbeTags};
use std::path::Path;

fn main() -> Result<()> {
    unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE")); }

    let cli = Cli::parse();
    let config = config::resolve(cli)?;
    ipod_sync::logging::init(config.verbose);
    run(&config)
}

fn run(config: &Config) -> Result<()> {
    transcode::verify_tools_available()?;

    // 1. Resolve iPod mount.
    let mount = match &config.ipod {
        Some(m) => {
            let p = ensure_trailing_backslash(m);
            if !Path::new(&p).join("iPod_Control").join("iTunes").join("iTunesDB").exists() {
                return Err(anyhow!("explicit --ipod {} does not contain iPod_Control\\iTunes\\iTunesDB", p));
            }
            p
        }
        None => detect_ipod_mount()?,
    };

    // 2. Walk source.
    let sources = source::walk(&config.source)?;

    // 3. Load (or rebuild) manifest.
    let mut manifest = if config.rebuild_manifest {
        let db = OwnedDb::open(Path::new(&mount))?;
        let rebuilt = build_rebuild_manifest(&db);
        // Save eagerly so a crash after this point doesn't lose the rebuild.
        manifest::save_atomic(&config.manifest_path, &rebuilt)?;
        rebuilt
    } else {
        manifest::load_or_default(&config.manifest_path)?
    };

    // 4. Diff.
    let actions = manifest::diff(&manifest, &sources);
    let (add, modify, remove, unchanged) = count_actions(&actions);

    // Start the progress handle now that we know the action counts.
    let progress = Progress::start(config.use_tui)?;
    progress.header(
        config.source.display().to_string(),
        mount.clone(),
        config.manifest_path.display().to_string(),
    );

    if config.rebuild_manifest {
        progress.log(format!(
            "Rebuilt manifest from iPod: {} track(s) recorded as source-unknown",
            manifest.tracks.len()
        ));
    } else {
        progress.log(format!("Loaded {} existing manifest entries", manifest.tracks.len()));
    }
    progress.log(format!("Source: {} FLAC file(s)", sources.len()));

    let effective_remove = if config.no_delete { 0 } else { remove };
    let total_planned = add + modify + effective_remove;
    progress.summary(add, modify, remove, unchanged, total_planned);

    if config.dry_run {
        progress.log("Dry run; nothing was written.");
        progress.finish();
        return Ok(());
    }

    if add == 0 && modify == 0 && (remove == 0 || config.no_delete) {
        progress.log("Nothing to do.");
        progress.finish();
        return Ok(());
    }

    // 5. Apply actions + 6. Commit DB + manifest.
    // Wrapped in a closure so that any mid-sync error can be paired with a
    // recovery hint before bubbling up, and progress.finish() is always called.
    let sync_result: Result<()> = (|| -> Result<()> {
        let db = OwnedDb::open(Path::new(&mount))?;
        let guid = device::read_firewire_guid(Path::new(&mount))?;
        unsafe {
            let device_ptr = (*db.as_ptr()).device;
            device::set_firewire_guid(device_ptr, &guid)?;
        }

        let mut i = 0usize;
        for action in actions {
            match action {
                Action::Unchanged(_) => continue,
                Action::Remove(entry) => {
                    if config.no_delete {
                        continue;
                    }
                    i += 1;
                    progress.track_start(
                        i,
                        total_planned,
                        format!("REMOVE {} (dbid {})", entry.source_path.display(), entry.ipod_dbid),
                    );
                    db.delete_track(entry.ipod_dbid)
                        .with_context(|| format!("delete dbid {}", entry.ipod_dbid))?;
                    manifest.tracks.retain(|e| e.ipod_dbid != entry.ipod_dbid);
                    progress.track_done();
                }
                Action::Modify(src, old) => {
                    i += 1;
                    progress.track_start(
                        i,
                        total_planned,
                        format!("MODIFY {}", src.path.display()),
                    );
                    if !config.no_delete {
                        db.delete_track(old.ipod_dbid)
                            .with_context(|| format!("delete-for-modify dbid {}", old.ipod_dbid))?;
                        manifest.tracks.retain(|e| e.ipod_dbid != old.ipod_dbid);
                    }
                    let handle = add_one(&db, &src)?;
                    manifest.tracks.push(entry_from(&src, &handle));
                    progress.track_done();
                }
                Action::Add(src) => {
                    i += 1;
                    progress.track_start(
                        i,
                        total_planned,
                        format!("ADD {}", src.path.display()),
                    );
                    let handle = add_one(&db, &src)?;
                    manifest.tracks.push(entry_from(&src, &handle));
                    progress.track_done();
                }
            }
        }

        // 6. Commit DB + manifest. NEITHER is persisted unless we got this far.
        progress.log("Writing iPod DB...");
        db.write()?;
        progress.log("Writing manifest...");
        manifest::save_atomic(&config.manifest_path, &manifest)?;

        progress.log("Done. Eject the iPod before unplugging.");
        Ok(())
    })();

    if let Err(e) = &sync_result {
        progress.error(format!("Sync failed: {e}"));
        progress.error("The iPod may now contain orphan track files (added but not".to_string());
        progress.error("in the iTunesDB), and the manifest has NOT been updated.".to_string());
        progress.error("To recover: re-run with --rebuild-manifest, which will read the iPod's".to_string());
        progress.error("current DB and create a fresh manifest. Then run normally.".to_string());
    }
    progress.finish();
    sync_result
}

/// Transcode + add one source file. Returns the iPod-side handle.
fn add_one(db: &OwnedDb, src: &SourceEntry) -> Result<TrackHandle> {
    let probe = transcode::probe(&src.path)
        .with_context(|| format!("probe {}", src.path.display()))?;
    let tags = tags_from_probe(&probe);

    let temp = transcode::temp_alac_path();
    if let Some(parent) = temp.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    transcode::transcode_to_alac(&src.path, &temp)
        .with_context(|| format!("transcode {}", src.path.display()))?;

    let art = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        transcode::extract_cover_art(&src.path, &art_path)?;
        let bytes = std::fs::read(&art_path)?;
        let _ = std::fs::remove_file(&art_path);
        Some(bytes)
    } else {
        None
    };

    let handle = db.add_track_with_file(&temp, &tags, art.as_deref())
        .with_context(|| format!("add_track_with_file for {}", src.path.display()))?;

    let _ = std::fs::remove_file(&temp);
    Ok(handle)
}

fn tags_from_probe(p: &ProbeOutput) -> Tags {
    let pt: &ProbeTags = match &p.format.tags {
        Some(t) => t,
        None => return Tags::default(),
    };

    let track_nr = pt.track.as_deref().and_then(|s| parse_int_first_field(s));
    let tracks_from_total = pt.track_total.as_deref().and_then(|s| s.trim().parse().ok());
    let tracks_from_slash = pt.track.as_deref().and_then(parse_int_second_field);
    let tracks = tracks_from_total.or(tracks_from_slash);

    let disc_nr = pt.disc.as_deref().and_then(|s| parse_int_first_field(s));
    let discs_from_total = pt.disc_total.as_deref().and_then(|s| s.trim().parse().ok());
    let discs_from_slash = pt.disc.as_deref().and_then(parse_int_second_field);
    let discs = discs_from_total.or(discs_from_slash);

    let year = pt.date.as_deref().and_then(parse_year);

    Tags {
        title: pt.title.clone(),
        artist: pt.artist.clone(),
        album: pt.album.clone(),
        album_artist: pt.album_artist.clone(),
        genre: pt.genre.clone(),
        composer: pt.composer.clone(),
        year,
        track_nr,
        tracks,
        disc_nr,
        discs,
    }
}

/// "9/12" -> Some(9). "9" -> Some(9). "" / garbage -> None.
fn parse_int_first_field(s: &str) -> Option<i32> {
    s.split('/').next()?.trim().parse().ok()
}

/// "9/12" -> Some(12). "9" -> None.
fn parse_int_second_field(s: &str) -> Option<i32> {
    s.split('/').nth(1)?.trim().parse().ok()
}

fn parse_year(s: &str) -> Option<i32> {
    s.split('-').next()?.trim().parse().ok()
}

fn count_actions(actions: &[Action]) -> (usize, usize, usize, usize) {
    let mut add = 0; let mut modify = 0; let mut remove = 0; let mut unchanged = 0;
    for a in actions {
        match a {
            Action::Add(_) => add += 1,
            Action::Modify(_, _) => modify += 1,
            Action::Remove(_) => remove += 1,
            Action::Unchanged(_) => unchanged += 1,
        }
    }
    (add, modify, remove, unchanged)
}

fn entry_from(src: &SourceEntry, handle: &TrackHandle) -> ManifestEntry {
    ManifestEntry {
        source_path: src.path.clone(),
        source_mtime: src.mtime,
        source_size: src.size,
        source_fingerprint: src.fingerprint.clone(),
        ipod_dbid: handle.dbid,
        ipod_relpath: handle.ipod_relpath.clone(),
        source_known: true,
    }
}

fn build_rebuild_manifest(db: &OwnedDb) -> Manifest {
    let handles = db.list_tracks_for_rebuild();
    let tracks = handles.into_iter().map(|h| ManifestEntry {
        source_path: std::path::PathBuf::new(),
        source_mtime: 0,
        source_size: 0,
        source_fingerprint: String::new(),
        ipod_dbid: h.dbid,
        ipod_relpath: h.ipod_relpath,
        source_known: false,
    }).collect();
    Manifest { version: 1, ipod_serial: None, tracks }
}

fn ensure_trailing_backslash(s: &str) -> String {
    if s.ends_with('\\') { s.to_string() } else { format!("{s}\\") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_first_field_handles_slash_and_lone() {
        assert_eq!(parse_int_first_field("9/12"), Some(9));
        assert_eq!(parse_int_first_field("9"), Some(9));
        assert_eq!(parse_int_first_field(""), None);
        assert_eq!(parse_int_first_field("abc"), None);
    }

    #[test]
    fn parse_int_second_field_only_returns_after_slash() {
        assert_eq!(parse_int_second_field("9/12"), Some(12));
        assert_eq!(parse_int_second_field("9"), None);
    }

    #[test]
    fn parse_year_handles_iso_date_and_lone_year() {
        assert_eq!(parse_year("2002-09-24"), Some(2002));
        assert_eq!(parse_year("2002"), Some(2002));
        assert_eq!(parse_year(""), None);
    }
}
