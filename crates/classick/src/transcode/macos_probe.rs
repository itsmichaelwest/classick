//! macOS-native probe backend: maps `lofty`'s tag/codec reading onto the
//! shared `ProbeOutput`/`ProbeTags`/`ProbeStream` seam so `apply_loop` and
//! `classify` don't need to know whether ffprobe or lofty produced the data.

use crate::transcode::{ProbeDisposition, ProbeFormat, ProbeOutput, ProbeStream, ProbeTags};
use anyhow::{Context, Result};
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::prelude::*;
use lofty::probe::Probe;
use std::fs::File;
use std::path::Path;

/// Read `path` with lofty and build a `ProbeOutput` equivalent to what
/// ffprobe would have produced: an audio `ProbeStream` with a codec name,
/// an optional synthetic video `ProbeStream` (attached_pic) if the file has
/// embedded art, and `ProbeTags` mapped from the primary tag.
pub fn probe_output_from_lofty(path: &Path) -> Result<ProbeOutput> {
    let tagged = Probe::open(path)
        .with_context(|| format!("lofty open {}", path.display()))?
        .read()
        .with_context(|| format!("lofty read {}", path.display()))?;

    let codec_name = codec_name_for(&tagged, path);
    let format_name = Some(format!("{:?}", tagged.file_type()).to_lowercase());

    let mut tags = ProbeTags::default();
    if let Some(t) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        tags.title = t.get_string(&ItemKey::TrackTitle).map(str::to_owned);
        tags.artist = t.get_string(&ItemKey::TrackArtist).map(str::to_owned);
        tags.album = t.get_string(&ItemKey::AlbumTitle).map(str::to_owned);
        tags.album_artist = t.get_string(&ItemKey::AlbumArtist).map(str::to_owned);
        tags.date = t
            .get_string(&ItemKey::Year)
            .or_else(|| t.get_string(&ItemKey::RecordingDate))
            .map(str::to_owned);
        tags.track = t.get_string(&ItemKey::TrackNumber).map(str::to_owned);
        tags.track_total = t.get_string(&ItemKey::TrackTotal).map(str::to_owned);
        tags.disc = t.get_string(&ItemKey::DiscNumber).map(str::to_owned);
        tags.disc_total = t.get_string(&ItemKey::DiscTotal).map(str::to_owned);
        tags.genre = t.get_string(&ItemKey::Genre).map(str::to_owned);
        tags.composer = t.get_string(&ItemKey::Composer).map(str::to_owned);
    }

    let mut streams = vec![ProbeStream {
        codec_type: "audio".into(),
        codec_name,
        disposition: None,
    }];
    let has_pic = tagged.tags().iter().any(|t| !t.pictures().is_empty());
    if has_pic {
        streams.push(ProbeStream {
            codec_type: "video".into(),
            codec_name: Some("mjpeg".into()),
            disposition: Some(ProbeDisposition {
                attached_pic: Some(1),
            }),
        });
    }

    // Duration → iTunesDB tracklen (ms). Without this the iPod shows -0:00 and
    // may skip, because classick doesn't otherwise set the track length and
    // libgpod can't backfill it from afconvert's ALAC container.
    let duration_ms = u32::try_from(tagged.properties().duration().as_millis()).ok();

    Ok(ProbeOutput {
        streams,
        format: ProbeFormat {
            format_name,
            tags: Some(tags),
        },
        duration_ms,
    })
}

/// Map lofty's `FileType` (+ MP4 codec sub-detection) to the ffprobe-style
/// `codec_name` string that `classify` matches on.
fn codec_name_for(tagged: &TaggedFile, path: &Path) -> Option<String> {
    Some(
        match tagged.file_type() {
            FileType::Flac => "flac",
            FileType::Mpeg => "mp3",
            FileType::Vorbis => "vorbis",
            FileType::Opus => "opus",
            FileType::Wav => "pcm_s16le",
            FileType::Aiff => "pcm_s16le",
            FileType::Mp4 => return mp4_codec(path),
            _ => return None,
        }
        .to_owned(),
    )
}

/// MP4 containers can hold either ALAC or AAC audio (occasionally MP3/FLAC-
/// in-MP4). The generic `TaggedFile` doesn't expose the codec, so re-open
/// the file as a concrete `Mp4File` to read its `Mp4Properties::codec()`.
fn mp4_codec(path: &Path) -> Option<String> {
    use lofty::config::ParseOptions;
    use lofty::mp4::{Mp4Codec, Mp4File};

    let mut file = File::open(path).ok()?;
    let mp4 = Mp4File::read_from(&mut file, ParseOptions::new()).ok()?;
    Some(
        match mp4.properties().codec() {
            Mp4Codec::ALAC => "alac",
            Mp4Codec::AAC => "aac",
            Mp4Codec::MP3 => "mp3",
            Mp4Codec::FLAC => "flac",
            Mp4Codec::Unknown | _ => return None,
        }
        .to_owned(),
    )
}

/// Write the first embedded picture found across `src`'s tags to `dst`.
/// Errors if no picture is present (callers should have already checked
/// `has_embedded_art` on the corresponding `ProbeOutput`).
pub fn extract_cover_art_via_lofty(src: &Path, dst: &Path) -> Result<()> {
    let tagged = Probe::open(src)
        .with_context(|| format!("lofty open {}", src.display()))?
        .read()
        .with_context(|| format!("lofty read {}", src.display()))?;

    let picture = tagged
        .tags()
        .iter()
        .find_map(|t| t.pictures().first())
        .ok_or_else(|| anyhow::anyhow!("{}: no embedded picture found", src.display()))?;

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir {}", parent.display()))?;
    }
    std::fs::write(dst, picture.data())
        .with_context(|| format!("write cover art to {}", dst.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcode::has_embedded_art;

    #[test]
    fn maps_flac_tags_codec_and_art() {
        let p = probe_output_from_lofty(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/tagged.flac"
        )))
        .unwrap();
        let tags = p.format.tags.as_ref().unwrap();
        assert!(tags.title.is_some());
        assert!(tags.artist.is_some());
        assert!(tags.album.is_some());
        assert!(p
            .streams
            .iter()
            .any(|s| s.codec_type == "audio" && s.codec_name.as_deref() == Some("flac")));
        assert!(has_embedded_art(&p));
    }
}
