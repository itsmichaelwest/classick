//! ffprobe metadata extraction + ffmpeg FLAC→ALAC transcoding.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Deserialize)]
pub struct ProbeOutput {
    #[serde(default)]
    pub streams: Vec<ProbeStream>,
    pub format: ProbeFormat,
}

#[derive(Debug, Deserialize)]
pub struct ProbeFormat {
    /// Comma-separated container list (e.g. `mov,mp4,m4a,3gp,3g2,mj2` for an
    /// MP4-family file). `classify` splits on `,` and checks each component.
    /// Optional + serde-default so Phase 1/Phase 2 fixtures still deserialize.
    #[serde(default)]
    pub format_name: Option<String>,
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
    /// ffprobe codec_name, e.g. `flac`, `mp3`, `aac`, `alac`, `vorbis`, `opus`,
    /// `pcm_s16le`. Required input to `classify`; optional + serde-default so
    /// Phase 1/Phase 2 fixtures still deserialize.
    #[serde(default)]
    pub codec_name: Option<String>,
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

/// Derive the ffprobe binary path from the configured ffmpeg path. If ffmpeg
/// is a full path (e.g. `C:\bin\ffmpeg.exe`), ffprobe is taken from the same
/// directory; otherwise we assume both are on PATH and return the bare
/// `"ffprobe"` name. Centralizing this here means callers don't each invent
/// their own ffprobe-lookup logic, and the `--ffmpeg` override extends to
/// probing automatically (F-16).
pub fn ffprobe_path_for(ffmpeg: &Path) -> PathBuf {
    if ffmpeg.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
        ffmpeg.with_file_name(
            if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" }
        )
    } else {
        PathBuf::from("ffprobe")
    }
}

/// Build the ffmpeg argument vector for FLAC→ALAC with art passthrough.
/// Extracted so we can unit-test the arg construction without spawning ffmpeg.
pub fn ffmpeg_args(src: &Path, dst: &Path) -> Vec<String> {
    vec![
        // `-nostdin` disables ffmpeg's interactive stdin reader, which
        // otherwise blocks during finalization when ffmpeg inherits a
        // pipe-stdin (as it does when invoked from the daemon's sync
        // subprocess — that subprocess's stdin is the daemon's cancel
        // pipe and is never closed). Observed in the wild as ffmpeg
        // wedging at ~97% of a track for the entire session.
        "-nostdin".into(),
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
/// `ffmpeg_path` is the configured ffmpeg binary; ffprobe is derived from it
/// via [`ffprobe_path_for`] (F-16).
pub fn probe(src: &Path, ffmpeg_path: &Path) -> Result<ProbeOutput> {
    let ffprobe = ffprobe_path_for(ffmpeg_path);
    let out = Command::new(&ffprobe)
        .args(["-loglevel", "error", "-of", "json", "-show_format", "-show_streams"])
        .arg(src)
        .output()
        .map_err(|e| anyhow!("failed to spawn {} (is it on PATH?): {e}", ffprobe.display()))?;
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
/// `ffmpeg_path` is the configured ffmpeg binary (F-16).
pub fn transcode_to_alac(src: &Path, dst: &Path, ffmpeg_path: &Path) -> Result<()> {
    // Belt + braces with the `-nostdin` flag in ffmpeg_args: explicitly
    // null out stdin so the inherited pipe (from the sync subprocess's
    // stdin → daemon cancel channel) can't possibly be read.
    let status = Command::new(ffmpeg_path)
        .args(ffmpeg_args(src, dst))
        .stdin(Stdio::null())
        .status()
        .map_err(|e| anyhow!("failed to spawn {} (is it on PATH?): {e}", ffmpeg_path.display()))?;
    if !status.success() {
        return Err(anyhow!("ffmpeg transcode failed (exit {:?})", status.code()));
    }
    Ok(())
}

/// Probe for refalac64. Returns Ok(version_string) on success, Err on
/// any failure (not on PATH, exec error, non-zero exit, weird output).
/// Used by preflight only when the user has selected --encoder refalac.
///
/// The version string is best-effort: refalac prints its banner to stderr on
/// `--help` and we grep for any line containing "refalac". If parsing fails
/// we fall back to `"refalac (version unknown)"` — acceptable because the
/// version field is forensic-only (manifest::diff's encoder-mismatch logic
/// only compares the encoder name, not the version).
pub fn verify_refalac_available(refalac_path: &Path) -> Result<String> {
    let output = Command::new(refalac_path)
        .arg("--help")
        .output()
        .with_context(|| {
            format!(
                "failed to invoke {}: is refalac64 installed and on PATH?",
                refalac_path.display()
            )
        })?;
    // refalac prints its version+banner to stderr on --help and exits 0 or 1.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}{stdout}");
    // Look for "refalac" anywhere in the output as a smoke test that we're
    // talking to refalac and not some other binary that happened to share the name.
    if !combined.to_lowercase().contains("refalac") {
        return Err(anyhow!(
            "{} ran but its output doesn't look like refalac. First 200 bytes: {}",
            refalac_path.display(),
            combined.chars().take(200).collect::<String>()
        ));
    }
    // Try to parse a version string out — best-effort, falls back to "unknown"
    // if the regex doesn't match. The version is recorded in ManifestEntry.encoder_version.
    let version = combined
        .lines()
        .find(|l| l.to_lowercase().contains("refalac"))
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| "refalac (version unknown)".to_string());
    Ok(version)
}

/// Verify ffmpeg and ffprobe are reachable. `ffmpeg_path` is the configured
/// ffmpeg binary; ffprobe is derived from it via [`ffprobe_path_for`] (F-16).
/// Call at startup so the user gets a clear error before we try anything else.
pub fn verify_tools_available(ffmpeg_path: &Path) -> Result<()> {
    let ffprobe = ffprobe_path_for(ffmpeg_path);
    for (label, tool) in [("ffmpeg", ffmpeg_path.to_path_buf()), ("ffprobe", ffprobe)] {
        let r = Command::new(&tool).arg("-version").output();
        match r {
            Ok(o) if o.status.success() => {}
            Ok(o) => return Err(anyhow!(
                "{label} ({}) returned exit {:?}: {}",
                tool.display(),
                o.status.code(),
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(_) => return Err(anyhow!(
                "{label} not found at {}. Install ffmpeg (e.g. winget install Gyan.FFmpeg) or pass --ffmpeg <path> and re-run.",
                tool.display()
            )),
        }
    }
    Ok(())
}

/// Per-process temp file under `%TEMP%\<PROJECT_DIR>\` with an optional
/// filename infix and the given extension. Centralizes the
/// `%TEMP%/ipod-sync/ipod-sync-<infix>-<pid>.<ext>` pattern used by the
/// transcode pipeline (alac, wav, art, passthrough).
fn project_temp_path(infix: &str, ext: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(crate::PROJECT_DIR);
    let filename = if infix.is_empty() {
        format!("{}-{}.{ext}", crate::PROJECT_DIR, std::process::id())
    } else {
        format!("{}-{infix}-{}.{ext}", crate::PROJECT_DIR, std::process::id())
    };
    p.push(filename);
    p
}

/// Build the path to the Phase 1 temp file: %TEMP%\ipod-sync\ipod-sync-<pid>.m4a.
pub fn temp_alac_path() -> PathBuf {
    project_temp_path("", "m4a")
}

/// Build the path to the Phase 1 cover-art temp file.
pub fn temp_art_path() -> PathBuf {
    project_temp_path("art", "jpg")
}

// ---------------------------------------------------------------------------
// Phase 3: source classification + passthrough pipeline.
// ---------------------------------------------------------------------------

/// Outcome of [`classify`] — tells `apply_loop::add_one` which pipeline to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAction {
    /// Copy the source byte-for-byte; iPod plays it natively (mp3, aac, alac,
    /// and pcm/wav-aiff when `--passthrough-wav` is set).
    Passthrough,
    /// Decode + re-encode to ALAC via the configured encoder
    /// (flac, vorbis, opus, and pcm/wav-aiff by default).
    Transcode,
}

/// Subset of `Config` that [`classify`] needs. Keeps the function's signature
/// small and avoids a circular dep on `crate::config` from `transcode`.
///
/// `apply_loop` (Task 6) constructs one of these from `&Config` at call-time:
/// `ClassifyConfig { passthrough_wav: config.passthrough_wav }`. Phase 3's
/// Task 3 adds the `passthrough_wav` field to `Config`; until that lands,
/// callers can still construct this struct directly with a literal bool.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyConfig {
    pub passthrough_wav: bool,
}

/// Classify a source file based on its ffprobe output. Returns the action the
/// apply loop should take for this track.
///
/// Decision matrix per the Phase 3 spec § "Source classification":
///
/// | codec_name             | container                | action                                       |
/// |------------------------|--------------------------|----------------------------------------------|
/// | `flac`                 | `flac`                   | Transcode (iPod can't decode FLAC)           |
/// | `mp3`                  | `mp3`                    | Passthrough                                  |
/// | `aac`                  | `m4a` / `mp4` / `aac` / `mov` | Passthrough                             |
/// | `alac`                 | `m4a` / `mp4` / `mov`    | Passthrough                                  |
/// | `vorbis`               | `ogg`                    | Transcode                                    |
/// | `opus`                 | `opus` / `ogg`           | Transcode                                    |
/// | `pcm_*` (any subtype)  | `wav` / `aiff`           | Passthrough iff `passthrough_wav`, else Transcode |
/// | anything else          | *                        | Err (caller surfaces stop-on-first-error)   |
///
/// `format.format_name` is comma-separated for multi-format containers
/// (e.g. `mov,mp4,m4a,3gp,3g2,mj2`). We split on `,` and check each component.
pub fn classify(probe: &ProbeOutput, config: &ClassifyConfig) -> Result<SourceAction> {
    let audio_codec = probe
        .streams
        .iter()
        .find(|s| s.codec_type == "audio")
        .and_then(|s| s.codec_name.as_deref())
        .ok_or_else(|| anyhow!("classify: no audio stream / codec_name in probe"))?;

    let containers: Vec<&str> = probe
        .format
        .format_name
        .as_deref()
        .map(|s| s.split(',').map(|x| x.trim()).collect())
        .unwrap_or_default();
    let in_container = |c: &str| containers.iter().any(|x| *x == c);

    // PCM is an open family (s16le, s24le, s32le, f32le, ...). Treat any
    // `pcm_*` codec as a single bucket for the WAV/AIFF decision.
    let is_pcm = audio_codec.starts_with("pcm_");

    let action = match audio_codec {
        "flac" if in_container("flac") => SourceAction::Transcode,
        "mp3" if in_container("mp3") => SourceAction::Passthrough,
        "aac"
            if in_container("m4a")
                || in_container("mp4")
                || in_container("aac")
                || in_container("mov") =>
        {
            SourceAction::Passthrough
        }
        "alac" if in_container("m4a") || in_container("mp4") || in_container("mov") => {
            SourceAction::Passthrough
        }
        "vorbis" if in_container("ogg") => SourceAction::Transcode,
        "opus" if in_container("opus") || in_container("ogg") => SourceAction::Transcode,
        _ if is_pcm && (in_container("wav") || in_container("aiff")) => {
            if config.passthrough_wav {
                SourceAction::Passthrough
            } else {
                SourceAction::Transcode
            }
        }
        _ => {
            return Err(anyhow!(
                "unsupported source: codec_name={audio_codec}, container={containers:?}.\n\
                 ipod-sync v1 handles: flac, mp3, aac, alac, vorbis, opus, pcm (wav/aiff).\n\
                 AC3, WMA, and other formats are out of scope."
            ));
        }
    };

    Ok(action)
}

/// Copy `src` to `dst` byte-for-byte. The destination's parent dir is created
/// if missing. Used by `apply_loop::add_one` when [`classify`] returns
/// [`SourceAction::Passthrough`].
///
/// Tags are NOT touched here — libgpod handles them via `apply_tags`, separate
/// from the file body. Cover art for passthrough lives inside the source
/// file's own metadata (e.g. ID3 APIC for MP3, MP4 `covr` atom for AAC/ALAC);
/// we still extract it via `extract_cover_art` for libgpod's thumbnail-write
/// path, but the file body itself is a verbatim copy.
pub fn passthrough(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("create parent dir {}: {e}", parent.display()))?;
    }
    std::fs::copy(src, dst)
        .map(|_| ())
        .map_err(|e| anyhow!("passthrough copy {} -> {}: {e}", src.display(), dst.display()))
}

/// Path for the refalac 2-step pipeline's WAV intermediate.
/// `%TEMP%\ipod-sync\ipod-sync-<pid>.wav`.
pub fn temp_wav_path() -> PathBuf {
    project_temp_path("", "wav")
}

/// Path for a passthrough copy. Same extension as the source so libgpod's
/// internal type-sniffing works without extra hints. Falls back to `.bin`
/// only if the source has no extension at all (shouldn't happen for files
/// the walker accepted, but defensive).
pub fn temp_passthrough_path(src: &Path) -> PathBuf {
    let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("bin");
    project_temp_path("", ext)
}

/// 2-step pipeline: source → ffmpeg-decode → temp.wav → refalac → temp.m4a.
///
/// Refalac only reads WAV/AIFF natively; uniform handling of all source
/// formats goes through the WAV intermediate. Future work: pipe directly
/// (ffmpeg stdout → refalac stdin) to eliminate the WAV temp.
///
/// Returns `Ok(())` on success; both temps cleaned up before return.
/// Caller is responsible for tags + cover-art via libgpod (refalac doesn't
/// carry over WAV-side tags).
pub fn transcode_via_refalac(
    src: &Path,
    dst_m4a: &Path,
    refalac_path: &Path,
    ffmpeg_path: &Path,
    art_jpg: Option<&Path>,
) -> Result<()> {
    let temp_wav = temp_wav_path();
    if let Some(parent) = temp_wav.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    if let Some(parent) = dst_m4a.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Step 1: ffmpeg decode source → temp.wav. -vn drops any attached_pic
    // video stream so refalac sees a pure-audio WAV. `-nostdin` + stdin
    // null prevents ffmpeg from blocking on inherited pipe-stdin.
    let mut ffmpeg = Command::new(ffmpeg_path);
    ffmpeg.args(["-nostdin", "-hide_banner", "-loglevel", "warning", "-y", "-i"]);
    ffmpeg.arg(src);
    ffmpeg.args(["-vn", "-acodec", "pcm_s16le"]);
    ffmpeg.arg(&temp_wav);
    ffmpeg.stdin(Stdio::null());
    let ffmpeg_out = ffmpeg
        .output()
        .with_context(|| format!("ffmpeg decode of {} to WAV", src.display()))?;
    if !ffmpeg_out.status.success() {
        let _ = std::fs::remove_file(&temp_wav);
        return Err(anyhow!(
            "ffmpeg decode {} -> WAV failed: {}",
            src.display(),
            String::from_utf8_lossy(&ffmpeg_out.stderr)
        ));
    }

    // Step 2: refalac WAV → ALAC m4a (with optional artwork).
    let mut refalac = Command::new(refalac_path);
    refalac.args(["--silent", "-o"]);
    refalac.arg(dst_m4a);
    if let Some(art) = art_jpg {
        refalac.arg("--artwork");
        refalac.arg(art);
    }
    refalac.arg(&temp_wav);
    let refalac_out = refalac
        .output()
        .with_context(|| format!("refalac encode of {} to ALAC", temp_wav.display()));
    let _ = std::fs::remove_file(&temp_wav);
    let refalac_out = refalac_out?;
    if !refalac_out.status.success() {
        let _ = std::fs::remove_file(dst_m4a);
        return Err(anyhow!(
            "refalac encode -> {} failed: {}",
            dst_m4a.display(),
            String::from_utf8_lossy(&refalac_out.stderr)
        ));
    }
    Ok(())
}

/// Extract the first video stream (assumed to be the attached_pic / cover art)
/// from `src` to `dst` as a single still image. Assumes the source actually
/// has an attached_pic stream — caller should check via `has_embedded_art`
/// before calling. `ffmpeg_path` is the configured ffmpeg binary (F-16).
pub fn extract_cover_art(src: &Path, dst: &Path, ffmpeg_path: &Path) -> Result<()> {
    let status = Command::new(ffmpeg_path)
        .args(["-nostdin", "-loglevel", "error", "-y"])
        .args(["-i"])
        .arg(src)
        .args(["-an", "-c:v", "copy", "-map", "0:v:0", "-frames:v", "1"])
        .arg(dst)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| anyhow!("failed to spawn {} for art extract: {e}", ffmpeg_path.display()))?;
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

    // -----------------------------------------------------------------------
    // Phase 3: classify decision matrix.
    // -----------------------------------------------------------------------

    const FX_MP3: &str = include_str!("../tests/fixtures/sample-ffprobe-mp3.json");
    const FX_AAC: &str = include_str!("../tests/fixtures/sample-ffprobe-aac.json");
    const FX_ALAC: &str = include_str!("../tests/fixtures/sample-ffprobe-alac.json");
    const FX_VORBIS: &str = include_str!("../tests/fixtures/sample-ffprobe-vorbis.json");
    const FX_OPUS: &str = include_str!("../tests/fixtures/sample-ffprobe-opus.json");
    const FX_WAV: &str = include_str!("../tests/fixtures/sample-ffprobe-wav.json");
    const FX_UNKNOWN: &str = include_str!("../tests/fixtures/sample-ffprobe-unknown.json");
    // Re-uses the existing SAMPLE constant for FLAC (codec_name=flac, format_name=flac).

    fn cc(passthrough_wav: bool) -> ClassifyConfig {
        ClassifyConfig { passthrough_wav }
    }

    fn parse(s: &str) -> ProbeOutput {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn classify_flac_is_transcode() {
        assert_eq!(
            classify(&parse(SAMPLE), &cc(false)).unwrap(),
            SourceAction::Transcode
        );
    }

    #[test]
    fn classify_mp3_is_passthrough() {
        assert_eq!(
            classify(&parse(FX_MP3), &cc(false)).unwrap(),
            SourceAction::Passthrough
        );
    }

    #[test]
    fn classify_aac_in_m4a_container_is_passthrough() {
        assert_eq!(
            classify(&parse(FX_AAC), &cc(false)).unwrap(),
            SourceAction::Passthrough
        );
    }

    #[test]
    fn classify_alac_in_m4a_container_is_passthrough() {
        assert_eq!(
            classify(&parse(FX_ALAC), &cc(false)).unwrap(),
            SourceAction::Passthrough
        );
    }

    #[test]
    fn classify_vorbis_is_transcode() {
        assert_eq!(
            classify(&parse(FX_VORBIS), &cc(false)).unwrap(),
            SourceAction::Transcode
        );
    }

    #[test]
    fn classify_opus_is_transcode() {
        assert_eq!(
            classify(&parse(FX_OPUS), &cc(false)).unwrap(),
            SourceAction::Transcode
        );
    }

    #[test]
    fn classify_wav_default_is_transcode() {
        assert_eq!(
            classify(&parse(FX_WAV), &cc(false)).unwrap(),
            SourceAction::Transcode
        );
    }

    #[test]
    fn classify_wav_with_passthrough_wav_is_passthrough() {
        assert_eq!(
            classify(&parse(FX_WAV), &cc(true)).unwrap(),
            SourceAction::Passthrough
        );
    }

    #[test]
    fn classify_unknown_codec_errors() {
        let err = classify(&parse(FX_UNKNOWN), &cc(false)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported source"), "got: {msg}");
        assert!(
            msg.contains("ac3"),
            "msg should name the offending codec_name: {msg}"
        );
    }

    #[test]
    fn classify_no_audio_stream_errors() {
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"png"}],"format":{"format_name":"png_pipe"}}"#;
        let err = classify(&parse(json), &cc(false)).unwrap_err();
        assert!(err.to_string().contains("no audio stream"));
    }

    // -----------------------------------------------------------------------
    // Phase 3: passthrough + temp path helpers.
    // -----------------------------------------------------------------------

    #[test]
    fn passthrough_copies_bytes_verbatim() {
        let src_dir =
            std::env::temp_dir().join(format!("ipod-sync-pt-test-{}", std::process::id()));
        std::fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("in.mp3");
        let dst = src_dir.join("subdir").join("out.mp3");
        let bytes: Vec<u8> = (0u8..=255).chain(0..=128).collect(); // arbitrary
        std::fs::write(&src, &bytes).unwrap();

        passthrough(&src, &dst).unwrap();
        let copied = std::fs::read(&dst).unwrap();
        assert_eq!(bytes, copied);

        std::fs::remove_dir_all(&src_dir).ok();
    }

    #[test]
    fn temp_passthrough_path_preserves_extension() {
        assert_eq!(
            temp_passthrough_path(Path::new(r"C:\a.mp3"))
                .extension()
                .and_then(|s| s.to_str()),
            Some("mp3")
        );
        assert_eq!(
            temp_passthrough_path(Path::new(r"C:\a.m4a"))
                .extension()
                .and_then(|s| s.to_str()),
            Some("m4a")
        );
        assert_eq!(
            temp_passthrough_path(Path::new(r"C:\noext"))
                .extension()
                .and_then(|s| s.to_str()),
            Some("bin")
        );
    }
}
