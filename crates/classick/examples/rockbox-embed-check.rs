//! Diagnostic: replicate transcode_one's rockbox_compat embed path for ONE
//! source file (transcode → normalize art → embed tags/art) and re-read the
//! output's tags. Proves whether the real afconvert→embed pipeline produces a
//! tagged .m4a. Usage: cargo run --example rockbox-embed-check -- <src.flac>
use anyhow::Result;
use classick::artwork;
use classick::tags::tags_from_probe;
use classick::transcode;
use std::path::Path;

fn main() -> Result<()> {
    let src = std::env::args().nth(1).expect("usage: rockbox-embed-check <src>");
    let src = Path::new(&src);
    let ffmpeg = Path::new("ffmpeg"); // ignored on macOS (afconvert/lofty)

    let probe = transcode::probe(src, ffmpeg)?;
    let tags = tags_from_probe(&probe);
    eprintln!(
        "SOURCE tags: title={:?} artist={:?} album={:?} | has_art={}",
        tags.title, tags.artist, tags.album, transcode::has_embedded_art(&probe)
    );

    let dst = transcode::temp_alac_path();
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p).ok();
    }
    transcode::transcode_to_alac(src, &dst, ffmpeg)?;

    let art = if transcode::has_embedded_art(&probe) {
        let ap = transcode::temp_art_path();
        transcode::extract_cover_art(src, &ap, ffmpeg)?;
        let bytes = std::fs::read(&ap)?;
        let _ = std::fs::remove_file(&ap);
        Some(artwork::normalize(&bytes)?)
    } else {
        None
    };

    artwork::embed_track_metadata(&dst, &tags, art.as_deref())?;

    let out = transcode::probe(&dst, ffmpeg)?;
    let out_tags = tags_from_probe(&out);
    eprintln!(
        "OUTPUT tags: title={:?} artist={:?} album={:?}",
        out_tags.title, out_tags.artist, out_tags.album
    );
    println!("output: {}", dst.display());
    let embedded_ok = out_tags.title.is_some() || out_tags.artist.is_some();
    eprintln!("EMBED {} — output .m4a {} tags", if embedded_ok { "WORKS" } else { "FAILED" }, if embedded_ok { "HAS" } else { "has NO" });
    Ok(())
}
