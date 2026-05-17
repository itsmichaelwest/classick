use anyhow::{anyhow, Context, Result};
use clap::Parser;
use ipod_sync::cli::Cli;
use ipod_sync::config::{self, Config};
use ipod_sync::ipod::db::{OwnedDb, Tags, TrackHandle};
use ipod_sync::ipod::{device, detect_ipod_mount};
use ipod_sync::manifest::{self, Action, Manifest, ManifestEntry};
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
    println!("Source  : {}", config.source.display());
    println!("Manifest: {}", config.manifest_path.display());

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
    println!("iPod    : {mount}");

    // 2. Walk source.
    println!("Walking source...");
    let sources = source::walk(&config.source)?;
    println!("  {} FLAC file(s)", sources.len());

    // 3. Load (or rebuild) manifest.
    let mut manifest = if config.rebuild_manifest {
        println!("Rebuilding manifest from iPod (--rebuild-manifest)...");
        let db = OwnedDb::open(Path::new(&mount))?;
        let rebuilt = build_rebuild_manifest(&db);
        println!("  {} existing iPod track(s) recorded as source-unknown", rebuilt.tracks.len());
        // Save eagerly so a crash after this point doesn't lose the rebuild.
        manifest::save_atomic(&config.manifest_path, &rebuilt)?;
        rebuilt
    } else {
        let m = manifest::load_or_default(&config.manifest_path)?;
        println!("Loaded {} existing manifest entries", m.tracks.len());
        m
    };

    // 4. Diff.
    let actions = manifest::diff(&manifest, &sources);
    let (add, modify, remove, unchanged) = count_actions(&actions);
    println!();
    println!("Action plan:");
    println!("  Add      : {add}");
    println!("  Modify   : {modify}");
    println!("  Remove   : {remove}{}", if config.no_delete { " (--no-delete; skipped)" } else { "" });
    println!("  Unchanged: {unchanged}");

    if config.dry_run {
        println!("\nDry run; nothing was written.");
        return Ok(());
    }

    if add == 0 && modify == 0 && (remove == 0 || config.no_delete) {
        println!("\nNothing to do.");
        return Ok(());
    }

    // 5. Apply actions.
    let db = OwnedDb::open(Path::new(&mount))?;
    let guid = device::read_firewire_guid(Path::new(&mount))?;
    unsafe {
        let device_ptr = (*db.as_ptr()).device;
        device::set_firewire_guid(device_ptr, &guid)?;
    }

    let total = actions.len();
    let mut i = 0usize;
    for action in actions {
        i += 1;
        match action {
            Action::Unchanged(_) => {}  // no-op
            Action::Remove(entry) => {
                if config.no_delete {
                    continue;
                }
                println!("[{i}/{total}] REMOVE {} (dbid {})", entry.source_path.display(), entry.ipod_dbid);
                db.delete_track(entry.ipod_dbid)
                    .with_context(|| format!("delete dbid {}", entry.ipod_dbid))?;
                manifest.tracks.retain(|e| e.ipod_dbid != entry.ipod_dbid);
            }
            Action::Modify(src, old) => {
                println!("[{i}/{total}] MODIFY {}", src.path.display());
                if !config.no_delete {
                    db.delete_track(old.ipod_dbid)
                        .with_context(|| format!("delete-for-modify dbid {}", old.ipod_dbid))?;
                    manifest.tracks.retain(|e| e.ipod_dbid != old.ipod_dbid);
                }
                let handle = add_one(&db, &src)?;
                manifest.tracks.push(entry_from(&src, &handle));
            }
            Action::Add(src) => {
                println!("[{i}/{total}] ADD {}", src.path.display());
                let handle = add_one(&db, &src)?;
                manifest.tracks.push(entry_from(&src, &handle));
            }
        }
    }

    // 6. Commit DB + manifest. NEITHER is persisted unless we got this far.
    println!("\nWriting iPod DB...");
    db.write()?;
    println!("Writing manifest...");
    manifest::save_atomic(&config.manifest_path, &manifest)?;

    println!("\nDone. Eject the iPod before unplugging.");
    Ok(())
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
