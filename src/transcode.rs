//! ffprobe metadata extraction + ffmpeg FLAC→ALAC transcoding.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

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
/// Worse, the SAME field can appear under multiple synonymous keys in one file
/// (e.g. `TRACKTOTAL` and `TOTALTRACKS` both populated by MusicBrainz Picard).
/// Serde's `#[serde(alias = ...)]` rejects this as a duplicate-field error,
/// so we deserialize manually: lowercase every incoming key, dispatch via the
/// canonical-name table below, and use first-write-wins when synonyms collide.
#[derive(Debug, Default)]
pub struct ProbeTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub date: Option<String>,
    pub track: Option<String>,
    pub track_total: Option<String>,
    pub disc: Option<String>,
    pub disc_total: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
}

impl<'de> Deserialize<'de> for ProbeTags {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Visit the JSON object as a free-form map of String -> serde_json::Value
        // so duplicate keys don't blow up. ffprobe values are always strings, but
        // accept Value to be forgiving (e.g. numeric DATE).
        let raw: std::collections::BTreeMap<String, serde_json::Value> =
            std::collections::BTreeMap::deserialize(d)?;
        let mut out = ProbeTags::default();
        for (key, value) in raw {
            let s = match value {
                serde_json::Value::String(s) => s,
                serde_json::Value::Number(n) => n.to_string(),
                _ => continue,  // skip arrays/objects/bools/null
            };
            if s.is_empty() {
                continue;
            }
            // Canonical lowercase mapping. Aliases share a target.
            let slot: Option<&mut Option<String>> = match key.to_ascii_lowercase().as_str() {
                "title" => Some(&mut out.title),
                "artist" => Some(&mut out.artist),
                "album" => Some(&mut out.album),
                "album_artist" | "albumartist" => Some(&mut out.album_artist),
                "date" | "year" => Some(&mut out.date),
                "track" | "tracknumber" => Some(&mut out.track),
                "tracktotal" | "totaltracks" => Some(&mut out.track_total),
                "disc" | "discnumber" => Some(&mut out.disc),
                "disctotal" | "totaldiscs" => Some(&mut out.disc_total),
                "genre" => Some(&mut out.genre),
                "composer" => Some(&mut out.composer),
                _ => None,
            };
            if let Some(slot) = slot {
                if slot.is_none() {
                    *slot = Some(s);
                }
            }
        }
        Ok(out)
    }
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

/// Build the ffmpeg argument vector for FLAC→ALAC with art passthrough.
/// Extracted so we can unit-test the arg construction without spawning ffmpeg.
pub fn ffmpeg_args(src: &Path, dst: &Path) -> Vec<String> {
    vec![
        "-loglevel".into(), "error".into(),
        "-y".into(),  // overwrite output without prompting
        "-i".into(), src.to_string_lossy().into_owned(),
        "-map".into(), "0:a".into(),
        "-map".into(), "0:v?".into(),  // optional video (attached pic) — `?` = don't error if absent
        "-c:a".into(), "alac".into(),
        "-c:v".into(), "copy".into(),
        "-disposition:v".into(), "attached_pic".into(),
        "-f".into(), "ipod".into(),
        dst.to_string_lossy().into_owned(),
    ]
}

/// Spawn ffprobe on `src` and parse its JSON output into a `ProbeOutput`.
pub fn probe(src: &Path) -> Result<ProbeOutput> {
    let out = Command::new("ffprobe")
        .args(["-loglevel", "error", "-of", "json", "-show_format", "-show_streams"])
        .arg(src)
        .output()
        .map_err(|e| anyhow!("failed to spawn ffprobe (is it on PATH?): {e}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "ffprobe failed (exit {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let parsed: ProbeOutput = serde_json::from_slice(&out.stdout)
        .map_err(|e| anyhow!("ffprobe produced unparseable JSON: {e}"))?;
    Ok(parsed)
}

/// Transcode `src` (FLAC) → `dst` (ALAC in MP4/ipod container, art passed through).
pub fn transcode_to_alac(src: &Path, dst: &Path) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args(ffmpeg_args(src, dst))
        .status()
        .map_err(|e| anyhow!("failed to spawn ffmpeg (is it on PATH?): {e}"))?;
    if !status.success() {
        return Err(anyhow!("ffmpeg transcode failed (exit {:?})", status.code()));
    }
    Ok(())
}

/// Verify ffmpeg and ffprobe are reachable via PATH. Call at startup so the user
/// gets a clear error before we try anything else.
pub fn verify_tools_available() -> Result<()> {
    for tool in &["ffmpeg", "ffprobe"] {
        let r = Command::new(tool).arg("-version").output();
        match r {
            Ok(o) if o.status.success() => {}
            Ok(o) => return Err(anyhow!(
                "{tool} returned exit {:?}: {}",
                o.status.code(),
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(_) => return Err(anyhow!(
                "{tool} not found on PATH. Install ffmpeg (e.g. winget install Gyan.FFmpeg) and re-run."
            )),
        }
    }
    Ok(())
}

/// Build the path to the Phase 1 temp file: %TEMP%\ipod-sync\ipod-sync-<pid>.m4a.
pub fn temp_alac_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push("ipod-sync");
    p.push(format!("ipod-sync-{}.m4a", std::process::id()));
    p
}

/// Build the path to the Phase 1 cover-art temp file.
pub fn temp_art_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push("ipod-sync");
    p.push(format!("ipod-sync-art-{}.jpg", std::process::id()));
    p
}

/// Extract the first video stream (assumed to be the attached_pic / cover art)
/// from `src` to `dst` as a single still image. Assumes the source actually
/// has an attached_pic stream — caller should check via `has_embedded_art`
/// before calling.
pub fn extract_cover_art(src: &Path, dst: &Path) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y"])
        .args(["-i"])
        .arg(src)
        .args(["-an", "-c:v", "copy", "-map", "0:v:0", "-frames:v", "1"])
        .arg(dst)
        .status()
        .map_err(|e| anyhow!("failed to spawn ffmpeg for art extract: {e}"))?;
    if !status.success() {
        return Err(anyhow!(
            "ffmpeg cover-art extract failed (exit {:?})",
            status.code()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const SAMPLE: &str = include_str!("../tests/fixtures/sample-ffprobe.json");

    #[test]
    fn ffmpeg_cmd_args_match_spec() {
        let args = ffmpeg_args(
            Path::new(r"C:\src\song.flac"),
            Path::new(r"C:\tmp\out.m4a"),
        );
        // Order matters for ffmpeg — input flags before -i, output flags after.
        let joined = args.join(" ");
        assert!(joined.contains("-loglevel error"));
        assert!(joined.contains("-y"));
        assert!(joined.contains(r"-i C:\src\song.flac"));
        assert!(joined.contains("-map 0:a"));
        assert!(joined.contains("-map 0:v?"));
        assert!(joined.contains("-c:a alac"));
        assert!(joined.contains("-c:v copy"));
        assert!(joined.contains("-disposition:v attached_pic"));
        assert!(joined.contains("-f ipod"));
        // The output path is the LAST arg.
        assert_eq!(args.last().unwrap(), r"C:\tmp\out.m4a");
    }

    #[test]
    fn probe_output_parses_format_tags() {
        let probe: ProbeOutput = serde_json::from_str(SAMPLE).unwrap();
        let tags = probe.format.tags.expect("fixture has format.tags");
        assert_eq!(tags.title.as_deref(), Some("Already Dead"));
        assert_eq!(tags.artist.as_deref(), Some("Beck"));
        assert_eq!(tags.album.as_deref(), Some("Sea Change"));
        assert_eq!(tags.album_artist.as_deref(), Some("Beck"));
        assert_eq!(tags.date.as_deref(), Some("2002-09-24"));
        assert_eq!(tags.track.as_deref(), Some("9"));
        assert_eq!(tags.track_total.as_deref(), Some("12"));
        assert_eq!(tags.disc.as_deref(), Some("1"));
        assert_eq!(tags.disc_total.as_deref(), Some("1"));
        assert_eq!(tags.genre.as_deref(), Some("Alternative"));
    }

    #[test]
    fn probe_output_detects_embedded_art() {
        let probe: ProbeOutput = serde_json::from_str(SAMPLE).unwrap();
        assert!(has_embedded_art(&probe));
    }

    #[test]
    fn probe_output_handles_duplicate_synonymous_keys() {
        // Real-world Picard-tagged FLAC: TRACKTOTAL and TOTALTRACKS coexist.
        // Serde's derive(Deserialize) with aliases rejected this; our manual
        // impl picks first-write-wins.
        let json = r#"{
            "streams":[{"codec_type":"audio"}],
            "format":{"tags":{
                "TITLE":"X",
                "TRACKTOTAL":"12",
                "TOTALTRACKS":"12",
                "DISCTOTAL":"1",
                "TOTALDISCS":"1",
                "track":"1",
                "disc":"1"
            }}
        }"#;
        let probe: ProbeOutput = serde_json::from_str(json).unwrap();
        let tags = probe.format.tags.expect("has tags");
        assert_eq!(tags.title.as_deref(), Some("X"));
        assert_eq!(tags.track.as_deref(), Some("1"));
        assert_eq!(tags.track_total.as_deref(), Some("12"));
        assert_eq!(tags.disc.as_deref(), Some("1"));
        assert_eq!(tags.disc_total.as_deref(), Some("1"));
    }

    #[test]
    fn probe_output_handles_missing_tags() {
        let json = r#"{"streams":[{"codec_type":"audio"}],"format":{}}"#;
        let probe: ProbeOutput = serde_json::from_str(json).unwrap();
        assert!(probe.format.tags.is_none());
        assert!(!has_embedded_art(&probe));
    }
}
