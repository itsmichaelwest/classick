mod ffi;
mod ipod;
mod transcode;

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

use crate::ipod::db::{OwnedDb, Tags};
use crate::ipod::device;
use crate::transcode::{has_embedded_art, probe, transcode_to_alac, ProbeOutput, ProbeTags};

const IPOD_MOUNT: &str = "G:\\";

fn main() -> Result<()> {
    // Tell gdk-pixbuf where to find its loader plugins. libgpod's artwork
    // code (itdb_track_set_thumbnails_from_data -> ithumb-writer.c ->
    // gdk_pixbuf_new_from_*) silently returns NULL if pixbuf can't locate a
    // loader for the input format -- and libgpod swallows that as a no-op
    // without setting GError, so we'd see itdb_track_set_thumbnails_from_data
    // return FALSE with no diagnostic. The build.rs bakes the absolute path
    // to the vendored loaders.cache into the binary; set the env var BEFORE
    // any libgpod call so the first pixbuf init in this process sees it.
    //
    // SAFETY: set_var is `unsafe` in Rust 2024 because it races with other
    // threads reading the environment. We're single-threaded here and this is
    // the first statement in main, so there's nothing to race with.
    unsafe {
        std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE"));
    }

    let source = parse_arg()?;
    println!("Source FLAC : {}", source.display());

    transcode::verify_tools_available()?;
    let probe_data = probe(&source)?;
    println!(
        "Source has embedded art: {}",
        if has_embedded_art(&probe_data) { "yes" } else { "no" }
    );

    let art_bytes: Option<Vec<u8>> = if has_embedded_art(&probe_data) {
        let art_path = transcode::temp_art_path();
        std::fs::create_dir_all(art_path.parent().unwrap())?;
        println!("Extracting cover art to {} ...", art_path.display());
        transcode::extract_cover_art(&source, &art_path)?;
        let bytes = std::fs::read(&art_path)?;
        println!("  art size: {} bytes", bytes.len());
        let _ = std::fs::remove_file(&art_path);
        Some(bytes)
    } else {
        println!("No embedded art; track will have no thumbnail.");
        None
    };

    let tags = tags_from_probe(&probe_data);
    println!("Title       : {}", tags.title.as_deref().unwrap_or("<none>"));
    println!("Artist      : {}", tags.artist.as_deref().unwrap_or("<none>"));
    println!("Album       : {}", tags.album.as_deref().unwrap_or("<none>"));

    let temp = transcode::temp_alac_path();
    std::fs::create_dir_all(temp.parent().unwrap())?;
    println!("Transcoding to {} ...", temp.display());
    transcode_to_alac(&source, &temp)?;

    let ipod_mount = Path::new(IPOD_MOUNT);
    println!("Opening iPod DB at {}", ipod_mount.display());
    let db = OwnedDb::open(ipod_mount)?;
    println!("Existing track count: {}", db.track_count());

    println!("Wiring FirewireGuid for write signing...");
    let guid = device::read_firewire_guid(ipod_mount)?;
    unsafe {
        let device_ptr = (*db.as_ptr()).device;
        device::set_firewire_guid(device_ptr, &guid)?;
    }

    println!("Adding track to DB...");
    db.add_track_with_file(&temp, &tags, art_bytes.as_deref())?;

    println!("Writing DB to iPod (this signs the hashed iTunesDB)...");
    db.write()?;

    println!("Deleting temp file...");
    let _ = std::fs::remove_file(&temp);

    println!("New track count: {}", db.track_count());
    println!("Done. Eject the iPod and verify on device.");
    Ok(())
}

fn parse_arg() -> Result<PathBuf> {
    let mut args = std::env::args();
    let _exe = args.next();
    let path = args
        .next()
        .ok_or_else(|| anyhow!("usage: ipod-sync <source.flac>"))?;
    let p = PathBuf::from(path);
    if !p.exists() {
        return Err(anyhow!("source file not found: {}", p.display()));
    }
    if p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase())
        != Some("flac".into())
    {
        return Err(anyhow!(
            "Phase 1 only accepts .flac sources (got: {})",
            p.display()
        ));
    }
    Ok(p)
}

fn tags_from_probe(p: &ProbeOutput) -> Tags {
    let pt: &ProbeTags = match &p.format.tags {
        Some(t) => t,
        None => return Tags::default(),
    };

    let (track_nr, tracks) = split_pair(pt.track.as_deref());
    let (disc_nr, discs) = split_pair(pt.disc.as_deref());
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

/// "9/12" → (Some(9), Some(12)). "9" → (Some(9), None). None → (None, None).
fn split_pair(s: Option<&str>) -> (Option<i32>, Option<i32>) {
    let Some(s) = s else { return (None, None); };
    let mut parts = s.split('/');
    let a = parts.next().and_then(|x| x.trim().parse().ok());
    let b = parts.next().and_then(|x| x.trim().parse().ok());
    (a, b)
}

/// "2002-09-24" → Some(2002). "2002" → Some(2002). "" / garbage → None.
fn parse_year(s: &str) -> Option<i32> {
    s.split('-').next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_pair_parses_slashed() {
        assert_eq!(split_pair(Some("9/12")), (Some(9), Some(12)));
    }

    #[test]
    fn split_pair_parses_lone_number() {
        assert_eq!(split_pair(Some("3")), (Some(3), None));
    }

    #[test]
    fn split_pair_handles_none_and_garbage() {
        assert_eq!(split_pair(None), (None, None));
        assert_eq!(split_pair(Some("")), (None, None));
        assert_eq!(split_pair(Some("abc")), (None, None));
    }

    #[test]
    fn parse_year_handles_iso_date() {
        assert_eq!(parse_year("2002-09-24"), Some(2002));
    }

    #[test]
    fn parse_year_handles_lone_year() {
        assert_eq!(parse_year("2002"), Some(2002));
    }

    #[test]
    fn parse_year_handles_garbage() {
        assert_eq!(parse_year(""), None);
        assert_eq!(parse_year("not-a-year"), None);
    }
}
