//! ffprobe metadata extraction + ffmpeg FLAC→ALAC transcoding.
//!
//! ffmpeg / ffprobe invocations are implemented in Task 3.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ProbeOutput {
    #[serde(default)]
    pub streams: Vec<ProbeStream>,
    pub format: ProbeFormat,
}

#[derive(Debug, Deserialize)]
pub struct ProbeFormat {
    pub tags: Option<ProbeTags>,
}

/// FLAC tag names are case-insensitive but ffprobe preserves the on-disk casing.
/// Common encoders use uppercase (TITLE, ARTIST, ...) but ffmpeg's Lavf muxer
/// normalizes some (`ALBUMARTIST` → `album_artist`, `TRACK` → `track`,
/// `DISC` → `disc`). We accept all common variants via serde aliases so the
/// parser doesn't fight the encoder.
#[derive(Debug, Default, Deserialize)]
pub struct ProbeTags {
    #[serde(default, alias = "TITLE", alias = "Title")]
    pub title: Option<String>,
    #[serde(default, alias = "ARTIST", alias = "Artist")]
    pub artist: Option<String>,
    #[serde(default, alias = "ALBUM", alias = "Album")]
    pub album: Option<String>,
    #[serde(
        default,
        alias = "ALBUMARTIST",
        alias = "album_artist",
        alias = "AlbumArtist"
    )]
    pub album_artist: Option<String>,
    #[serde(default, alias = "DATE", alias = "Date", alias = "year", alias = "YEAR")]
    pub date: Option<String>,
    #[serde(
        default,
        alias = "TRACK",
        alias = "Track",
        alias = "track",
        alias = "tracknumber",
        alias = "TRACKNUMBER"
    )]
    pub track: Option<String>,
    #[serde(
        default,
        alias = "DISC",
        alias = "Disc",
        alias = "disc",
        alias = "discnumber",
        alias = "DISCNUMBER"
    )]
    pub disc: Option<String>,
    #[serde(default, alias = "GENRE", alias = "Genre")]
    pub genre: Option<String>,
    #[serde(default, alias = "COMPOSER", alias = "Composer")]
    pub composer: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProbeStream {
    pub codec_type: String,
    #[serde(default)]
    pub disposition: Option<ProbeDisposition>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ProbeDisposition {
    #[serde(default)]
    pub attached_pic: Option<i32>,
}

/// True if the probe found a video stream marked as an attached picture
/// (i.e. embedded cover art in the FLAC).
pub fn has_embedded_art(probe: &ProbeOutput) -> bool {
    probe.streams.iter().any(|s| {
        s.codec_type == "video"
            && s.disposition
                .as_ref()
                .and_then(|d| d.attached_pic)
                .unwrap_or(0)
                != 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../tests/fixtures/sample-ffprobe.json");

    #[test]
    fn probe_output_parses_format_tags() {
        let probe: ProbeOutput = serde_json::from_str(SAMPLE).unwrap();
        let tags = probe.format.tags.expect("fixture has format.tags");
        assert_eq!(tags.title.as_deref(), Some("Already Dead"));
        assert_eq!(tags.artist.as_deref(), Some("Beck"));
        assert_eq!(tags.album.as_deref(), Some("Sea Change"));
        assert_eq!(tags.album_artist.as_deref(), Some("Beck"));
        assert_eq!(tags.date.as_deref(), Some("2002-09-24"));
        assert_eq!(tags.track.as_deref(), Some("9/12"));
        assert_eq!(tags.disc.as_deref(), Some("1/1"));
        assert_eq!(tags.genre.as_deref(), Some("Alternative"));
    }

    #[test]
    fn probe_output_detects_embedded_art() {
        let probe: ProbeOutput = serde_json::from_str(SAMPLE).unwrap();
        assert!(has_embedded_art(&probe));
    }

    #[test]
    fn probe_output_handles_missing_tags() {
        let json = r#"{"streams":[{"codec_type":"audio"}],"format":{}}"#;
        let probe: ProbeOutput = serde_json::from_str(json).unwrap();
        assert!(probe.format.tags.is_none());
        assert!(!has_embedded_art(&probe));
    }
}
