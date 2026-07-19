//! Normalize cover art to a small baseline JPEG and embed MP4 tags + art into
//! an `.m4a`, so a transcoded track is self-describing for Rockbox (which reads
//! tags/art from the file) while Apple firmware keeps reading the iTunesDB +
//! ithmb ArtworkDB. See
//! docs/device-safety.md.

use anyhow::{Context, Result};
use std::io::Cursor;
use std::path::Path;

/// Longest-edge cap for embedded/normalized cover art. Generous enough that
/// Apple's largest thumbnail (~320px for the F1069 cover format) sees no
/// quality loss, small enough to keep files lean and Rockbox decode fast.
pub const MAX_ART_EDGE: u32 = 600;

/// Decode cover-art bytes of any common format, downscale so the longest edge
/// is ≤ `MAX_ART_EDGE`, and re-encode as a baseline JPEG. Used for BOTH the
/// embedded `covr` atom (Rockbox) and libgpod's ithmb thumbnail input (Apple).
pub fn normalize(source_art: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(source_art).context("decoding source cover art")?;
    let (w, h) = (img.width(), img.height());
    let scaled = if w > MAX_ART_EDGE || h > MAX_ART_EDGE {
        img.resize(
            MAX_ART_EDGE,
            MAX_ART_EDGE,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };
    // Encode baseline JPEG (image's JpegEncoder is baseline). RGB8 drops any
    // alpha, which JPEG cannot represent anyway.
    let rgb = scaled.to_rgb8();
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 85);
    enc.encode(
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )
    .context("encoding normalized cover art JPEG")?;
    Ok(out)
}

/// Embed MP4 `ilst` metadata tags + an optional `covr` cover-art atom into an
/// existing `.m4a`, overwriting any existing tags/art (idempotent). Pure file
/// I/O — safe on a transcode worker; never touches libgpod.
pub fn embed_track_metadata(
    m4a: &Path,
    tags: &crate::ipod::db::Tags,
    art: Option<&[u8]>,
) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::prelude::*;
    use lofty::tag::{Tag, TagType};

    let mut file = lofty::read_from_path(m4a)
        .with_context(|| format!("reading {} for tagging", m4a.display()))?;
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(TagType::Mp4Ilst));
    }
    // `insert_tag` is a no-op for file types that don't support the tag type
    // (e.g. a WAV/AIFF passthrough file can't hold an MP4 ilst), so the tag may
    // still be absent here. Return Err (never panic) so the caller warn-skips —
    // this fn is called from non-fatal paths (transcode worker + backfill).
    let Some(tag) = file.primary_tag_mut() else {
        anyhow::bail!(
            "{} does not support MP4 tags (not an .m4a?); skipping embed",
            m4a.display()
        );
    };

    if let Some(v) = &tags.title {
        tag.set_title(v.clone());
    }
    if let Some(v) = &tags.artist {
        tag.set_artist(v.clone());
    }
    if let Some(v) = &tags.album {
        tag.set_album(v.clone());
    }
    if let Some(v) = &tags.genre {
        tag.set_genre(v.clone());
    }
    if let Some(v) = &tags.album_artist {
        tag.insert_text(ItemKey::AlbumArtist, v.clone());
    }
    if let Some(v) = &tags.composer {
        tag.insert_text(ItemKey::Composer, v.clone());
    }
    if let Some(v) = tags.year {
        tag.set_year(v as u32);
    }
    if let Some(v) = tags.track_nr {
        tag.set_track(v as u32);
    }
    if let Some(v) = tags.tracks {
        tag.set_track_total(v as u32);
    }
    if let Some(v) = tags.disc_nr {
        tag.set_disk(v as u32);
    }
    if let Some(v) = tags.discs {
        tag.set_disk_total(v as u32);
    }

    if let Some(bytes) = art {
        use lofty::picture::{MimeType, Picture, PictureType};
        // Replace any existing pictures with our normalized JPEG cover.
        while tag.picture_count() > 0 {
            tag.remove_picture(0);
        }
        tag.push_picture(Picture::new_unchecked(
            PictureType::CoverFront,
            Some(MimeType::Jpeg),
            None,
            bytes.to_vec(),
        ));
    }

    file.save_to_path(m4a, WriteOptions::default())
        .with_context(|| format!("writing tags to {}", m4a.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipod::db::Tags;

    fn sample_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, _| image::Rgb([(x % 256) as u8, 100, 150]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn normalize_downscales_large_art_to_baseline_jpeg() {
        let big = sample_png(1200, 1000);
        let out = normalize(&big).unwrap();
        let decoded = image::load_from_memory(&out).unwrap();
        assert!(decoded.width() <= MAX_ART_EDGE && decoded.height() <= MAX_ART_EDGE);
        // JPEG magic (baseline + progressive both start FF D8 FF).
        assert_eq!(&out[..2], &[0xFF, 0xD8]);
        // No progressive-JPEG SOF2 marker (0xFF 0xC2) — must be baseline.
        assert!(
            !out.windows(2).any(|w| w == [0xFF, 0xC2]),
            "must be baseline JPEG"
        );
    }

    #[test]
    fn normalize_keeps_small_art_within_bounds() {
        let small = sample_png(300, 300);
        let out = normalize(&small).unwrap();
        let decoded = image::load_from_memory(&out).unwrap();
        assert!(decoded.width() <= MAX_ART_EDGE && decoded.height() <= MAX_ART_EDGE);
    }

    fn tags_fixture() -> Tags {
        Tags {
            title: Some("Wake Me Up Tomorrow".into()),
            artist: Some("Luttrell".into()),
            album: Some("Intergalactic Plastic EP".into()),
            album_artist: Some("Luttrell".into()),
            genre: Some("Electronic".into()),
            composer: None,
            year: Some(2019),
            track_nr: Some(3),
            tracks: Some(5),
            disc_nr: Some(1),
            discs: Some(1),
            duration_ms: Some(240000),
        }
    }

    #[test]
    fn embed_writes_tags_and_art_readable_by_lofty() {
        use lofty::file::TaggedFileExt;
        use lofty::prelude::*;
        // A minimal real ALAC .m4a fixture must exist for lofty to open it.
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bare.m4a");
        let tmp = std::env::temp_dir().join(format!("classick-embed-{}.m4a", std::process::id()));
        std::fs::copy(fixture, &tmp).unwrap();

        let art = normalize(&sample_png(800, 800)).unwrap();
        embed_track_metadata(&tmp, &tags_fixture(), Some(&art)).unwrap();

        let f = lofty::read_from_path(&tmp).unwrap();
        let tag = f.primary_tag().unwrap();
        assert_eq!(tag.title().as_deref(), Some("Wake Me Up Tomorrow"));
        assert_eq!(tag.album().as_deref(), Some("Intergalactic Plastic EP"));
        assert!(tag.picture_count() >= 1, "covr must be embedded");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn embed_tags_only_when_no_art() {
        use lofty::file::TaggedFileExt;
        use lofty::prelude::*;
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bare.m4a");
        let tmp =
            std::env::temp_dir().join(format!("classick-embed-noart-{}.m4a", std::process::id()));
        std::fs::copy(fixture, &tmp).unwrap();
        embed_track_metadata(&tmp, &tags_fixture(), None).unwrap();
        let f = lofty::read_from_path(&tmp).unwrap();
        assert_eq!(
            f.primary_tag().unwrap().title().as_deref(),
            Some("Wake Me Up Tomorrow")
        );
        let _ = std::fs::remove_file(&tmp);
    }

    /// Regression: a file lofty can read but that doesn't support MP4 tags
    /// (e.g. a WAV/AIFF passthrough) must return Err, NOT panic — the backfill
    /// loop calls this per-track and must warn-skip rather than abort.
    #[test]
    fn embed_into_non_mp4_returns_err_not_panic() {
        // Minimal valid PCM WAV (mono/16-bit/44.1k, 2 samples) lofty can parse.
        let mut wav: Vec<u8> = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes()); // 36 + data(4)
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&88200u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&[0, 0, 0, 0]);

        let tmp =
            std::env::temp_dir().join(format!("classick-embed-wav-{}.wav", std::process::id()));
        std::fs::write(&tmp, &wav).unwrap();
        // Must not panic; must be Err (unsupported tag type).
        let r = embed_track_metadata(&tmp, &tags_fixture(), None);
        assert!(
            r.is_err(),
            "embedding into a non-MP4 file must return Err, not panic"
        );
        let _ = std::fs::remove_file(&tmp);
    }
}
