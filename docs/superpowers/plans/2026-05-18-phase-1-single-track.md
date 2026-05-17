# Phase 1: End-to-End Single Track — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Take one user-supplied FLAC, transcode to ALAC, write it to the connected iPod Classic (G:\) via libgpod with metadata + embedded album art, preserve the Phase 0 track that's already there, and physically verify on the device.

**Architecture:** Single-binary CLI taking one positional arg (the source FLAC path). Orchestrator in `main.rs` calls into two domain modules: `transcode` (ffprobe metadata + ffmpeg FLAC→ALAC) and `ipod` (RAII-wrapped libgpod write path). Album art rides in-band through ffmpeg's `-f ipod` muxer — no separate art handling. FirewireGUID is read from `SysInfoExtended` and pushed into libgpod's device struct so `itdb_write` computes the correct hashed signature for the Classic 7G.

**Tech Stack:** Rust stable (x86_64-pc-windows-msvc), libgpod (vendored from Phase 0, MSYS2/MinGW build), bindgen, anyhow, serde + serde_json (for parsing ffprobe output), ffmpeg + ffprobe (user-installed, on PATH).

**Plan scope:** Phase 1 only. Phase 2 (CLI flags, manifest, source walker, diff, deletions, `--dry-run`, TUI, etc.) is deferred to a separate plan written after this gate passes. This plan adds enough structure that Phase 2 can extend it without rewriting — but **does not** pre-build modules Phase 1 doesn't exercise.

**Gate:** track plays on the iPod's native menu with correct metadata and album art on the Now Playing screen; the pre-existing Phase 0 track (Beck "Colors") is preserved; the iPod boots and plays normally after eject. If after one focused day Task 6 produces a DB the iPod rejects, escalate per SPEC §8 row 2 (try FirewireGUID variants, fall back to libgpod artwork API, last resort: re-evaluate Rust port). Restoring the iPod via iTunes is the safety net.

---

## File Structure

```
F:\repos\ipod-sync\
├── Cargo.toml                    (modify: add serde, serde_json)
├── build.rs                      (modify: add g_strdup, g_free to allowlist)
├── src\
│   ├── main.rs                   (replace: spike → Phase 1 orchestrator)
│   ├── ffi.rs                    (unchanged)
│   ├── ipod\
│   │   ├── mod.rs                (new: public API re-exports)
│   │   ├── db.rs                 (new: OwnedDb RAII + Tags + write path)
│   │   └── device.rs             (new: SysInfoExtended → FirewireGUID)
│   └── transcode.rs              (new: ffprobe + ffmpeg invocation)
└── tests\
    └── fixtures\
        └── sample-ffprobe.json   (new: ffprobe output for a known FLAC, for unit tests)
```

**Module responsibilities:**

- `transcode.rs` — knows how to call ffmpeg/ffprobe and parse their output. Does not know anything about libgpod. One file because the responsibilities (probe, transcode, tool-check) are small and change together.
- `ipod::db` — wraps libgpod's DB operations in a Drop-safe Rust API. `OwnedDb` is the central RAII type. Knows about Tags (the data structure libgpod's struct fields are populated from), but knows nothing about ffprobe.
- `ipod::device` — narrow: read FirewireGUID from a SysInfoExtended file, push it into libgpod's device struct. One function, separated because it's its own concern with its own failure modes.
- `main.rs` — orchestrator. Maps ffprobe output → Tags → libgpod calls. The only place that knows about both `transcode` and `ipod`.

**Testing reality:** Unit tests cover what we can exercise without the iPod or ffmpeg subprocess: ffprobe JSON parsing, Tags construction from `ProbeOutput`, FirewireGUID extraction from a fixture. The libgpod write path is hardware-only; Task 6 IS its test.

---

## Task 1: Cargo deps, build.rs allowlist, module skeleton

**Files:**
- Modify: `F:\repos\ipod-sync\Cargo.toml`
- Modify: `F:\repos\ipod-sync\build.rs`
- Create: `F:\repos\ipod-sync\src\ipod\mod.rs`
- Create: `F:\repos\ipod-sync\src\ipod\db.rs` (empty stub)
- Create: `F:\repos\ipod-sync\src\ipod\device.rs` (empty stub)
- Create: `F:\repos\ipod-sync\src\transcode.rs` (empty stub)
- Modify: `F:\repos\ipod-sync\src\main.rs` (declare new modules; keep Phase 0 print)

- [ ] **Step 1: Add deps to `Cargo.toml`**

Edit the `[dependencies]` section to add `serde` (with derive) and `serde_json`:
```toml
[dependencies]
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 2: Broaden bindgen allowlist in `build.rs`**

Find the `.allowlist_function("g_error_.*")` line in `build.rs` and add two more lines below it:
```rust
.allowlist_function("g_strdup")
.allowlist_function("g_free")
```

These are needed by `ipod::db` to set string fields on `Itdb_Track`. `itdb_track_new` zero-initializes the struct, so existing string pointers are NULL; calling `g_free(NULL)` is a documented no-op so the `apply_tags` helper can unconditionally free-before-set.

- [ ] **Step 3: Create the `ipod` module dir + files**

```powershell
New-Item -ItemType Directory -Force -Path F:\repos\ipod-sync\src\ipod | Out-Null
```

Create `F:\repos\ipod-sync\src\ipod\mod.rs`:
```rust
pub mod db;
pub mod device;
```

Create `F:\repos\ipod-sync\src\ipod\db.rs` as an empty stub:
```rust
//! libgpod DB operations wrapped in RAII Rust types.
//!
//! Implemented in Task 5.
```

Create `F:\repos\ipod-sync\src\ipod\device.rs` as an empty stub:
```rust
//! Read FirewireGUID from the iPod's SysInfoExtended and push it into libgpod's
//! device struct so itdb_write computes a valid signed iTunesDB.
//!
//! Implemented in Task 4.
```

- [ ] **Step 4: Create `src/transcode.rs` stub**

```rust
//! ffprobe metadata extraction + ffmpeg FLAC→ALAC transcoding.
//!
//! Implemented across Tasks 2 and 3.
```

- [ ] **Step 5: Modify `src/main.rs` to declare modules without breaking the Phase 0 print**

Replace `src/main.rs` with:
```rust
mod ffi;
mod ipod;
mod transcode;

use anyhow::Result;

fn main() -> Result<()> {
    println!("ipod-sync — Phase 1 in progress (scaffold)");
    Ok(())
}
```

The Phase 0 spike code (the libgpod DB walk) is removed. Phase 0 is tagged at `phase-0-complete`; the spike is recoverable via `git show phase-0-complete:src/main.rs` if ever needed.

- [ ] **Step 6: Build and verify**

```powershell
cd F:\repos\ipod-sync
cargo build 2>&1 | Select-Object -Last 3
.\target\debug\ipod-sync.exe
```

Expected: builds cleanly (you should see Cargo download serde + serde_json on first run). Run prints:
```
ipod-sync — Phase 1 in progress (scaffold)
```

If the build fails because `g_strdup` or `g_free` aren't found by bindgen, double-check `build.rs` includes the new lines AND that the GLib include paths are still present (they were added in Phase 0 Task 5).

- [ ] **Step 7: Commit**

```powershell
git -C F:\repos\ipod-sync add Cargo.toml Cargo.lock build.rs src\main.rs src\ipod\ src\transcode.rs
git -C F:\repos\ipod-sync commit -m "feat: Phase 1 module skeleton + serde deps"
```

---

## Task 2: transcode — ffprobe metadata extraction

**Files:**
- Modify: `F:\repos\ipod-sync\src\transcode.rs`
- Create: `F:\repos\ipod-sync\tests\fixtures\sample-ffprobe.json`
- Create: `F:\repos\ipod-sync\src\transcode_tests.rs` (test module included from transcode.rs)

We TDD this because the JSON parsing is the one piece of `transcode.rs` that's pure logic and doesn't require subprocesses.

- [ ] **Step 1: Capture a real ffprobe sample as a test fixture**

Pick any FLAC on the machine (or use the source the user will provide for Task 6 — easier). Run:
```powershell
New-Item -ItemType Directory -Force -Path F:\repos\ipod-sync\tests\fixtures | Out-Null
ffprobe -loglevel error -of json -show_format -show_streams "<path-to-some.flac>" > F:\repos\ipod-sync\tests\fixtures\sample-ffprobe.json
Get-Content F:\repos\ipod-sync\tests\fixtures\sample-ffprobe.json | Select-Object -First 50
```

If you don't have a FLAC handy yet, hand-write a representative one — the fields we care about are documented below. But a real one is preferred because tag-casing varies across FLAC encoders (`ARTIST` vs `artist`, `ALBUMARTIST` vs `album_artist`).

The fixture must contain at minimum (real values fine, this is just a shape example):
```json
{
  "streams": [
    { "codec_type": "audio", "codec_name": "flac" },
    { "codec_type": "video", "codec_name": "mjpeg", "disposition": { "attached_pic": 1 } }
  ],
  "format": {
    "filename": "...",
    "duration": "243.546667",
    "tags": {
      "TITLE": "Already Dead",
      "ARTIST": "Beck",
      "ALBUM": "Sea Change",
      "ALBUMARTIST": "Beck",
      "DATE": "2002-09-24",
      "TRACK": "9/12",
      "DISC": "1/1",
      "GENRE": "Alternative"
    }
  }
}
```

The fixture is commited. We never re-run ffprobe in tests.

- [ ] **Step 2: Write the failing test in `src/transcode.rs`**

Append to `src/transcode.rs`:
```rust
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
```

(Substitute the actual values from your fixture if they differ — but use real values from a real file, not placeholders.)

- [ ] **Step 3: Run the test to verify it fails**

```powershell
cargo test transcode::tests 2>&1 | Select-Object -Last 10
```
Expected: FAIL — `ProbeOutput`, `has_embedded_art`, etc. are undefined.

- [ ] **Step 4: Implement the types + parser to make the test pass**

Replace `src/transcode.rs` with:
```rust
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
/// Common encoders use uppercase (TITLE, ARTIST, ...). We accept both via serde
/// aliases so the parser doesn't fight the encoder.
#[derive(Debug, Default, Deserialize)]
pub struct ProbeTags {
    #[serde(default, alias = "TITLE", alias = "Title")]
    pub title: Option<String>,
    #[serde(default, alias = "ARTIST", alias = "Artist")]
    pub artist: Option<String>,
    #[serde(default, alias = "ALBUM", alias = "Album")]
    pub album: Option<String>,
    #[serde(default, alias = "ALBUMARTIST", alias = "album_artist", alias = "AlbumArtist")]
    pub album_artist: Option<String>,
    #[serde(default, alias = "DATE", alias = "Date", alias = "year", alias = "YEAR")]
    pub date: Option<String>,
    #[serde(default, alias = "TRACK", alias = "Track", alias = "tracknumber", alias = "TRACKNUMBER")]
    pub track: Option<String>,
    #[serde(default, alias = "DISC", alias = "Disc", alias = "discnumber", alias = "DISCNUMBER")]
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
            && s.disposition.as_ref()
                .and_then(|d| d.attached_pic)
                .unwrap_or(0) != 0
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
```

(Substitute test assertion values to match your actual fixture if it has different tag values.)

- [ ] **Step 5: Run the tests to verify they pass**

```powershell
cargo test transcode 2>&1 | Select-Object -Last 15
```
Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\transcode.rs tests\fixtures\sample-ffprobe.json
git -C F:\repos\ipod-sync commit -m "feat(transcode): ffprobe JSON parsing with tag-case aliases"
```

---

## Task 3: transcode — ffmpeg invocation and tool verification

**Files:**
- Modify: `F:\repos\ipod-sync\src\transcode.rs` (append functions + test)

The ffmpeg subprocess call has two parts: building the arg vector (testable in isolation) and spawning the process (manual smoke test). We TDD the arg vector.

- [ ] **Step 1: Write the failing test for ffmpeg command construction**

Append to the `tests` module in `src/transcode.rs`:
```rust
    use std::path::Path;

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
```

- [ ] **Step 2: Run the test to verify it fails**

```powershell
cargo test ffmpeg_cmd_args_match_spec 2>&1 | Select-Object -Last 10
```
Expected: FAIL — `ffmpeg_args` undefined.

- [ ] **Step 3: Implement `ffmpeg_args`, `transcode_to_alac`, `probe`, `verify_tools_available`**

Append to `src/transcode.rs` (between the type definitions and the `#[cfg(test)]` module):
```rust
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

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
```

- [ ] **Step 4: Run the test to verify it passes**

```powershell
cargo test transcode 2>&1 | Select-Object -Last 15
```
Expected: 4 tests pass total.

- [ ] **Step 5: Manual ffmpeg smoke test**

This is hardware-adjacent: we want to verify the ffmpeg command actually produces a valid m4a from a real FLAC before Task 6 depends on it. Pick any FLAC you can lay hands on — same one you used for the fixture is fine.

```powershell
New-Item -ItemType Directory -Force -Path $env:TEMP\ipod-sync | Out-Null
ffmpeg -loglevel error -y -i "<your.flac>" -map 0:a -map 0:v? -c:a alac -c:v copy -disposition:v attached_pic -f ipod "$env:TEMP\ipod-sync\smoke.m4a"
Get-Item "$env:TEMP\ipod-sync\smoke.m4a" | Select-Object Length
ffprobe -loglevel error -of json -show_streams "$env:TEMP\ipod-sync\smoke.m4a" | Select-Object -First 30
```

Expected: m4a file is created, non-zero size, ffprobe shows an `alac` audio stream and (if source had art) an `mjpeg` video stream with `attached_pic: 1`. If ffmpeg errors with "Encoder 'alac' not found" the user's ffmpeg is a stripped build — install a full one (`winget install Gyan.FFmpeg`).

Delete the smoke file:
```powershell
Remove-Item "$env:TEMP\ipod-sync\smoke.m4a"
```

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\transcode.rs
git -C F:\repos\ipod-sync commit -m "feat(transcode): ffmpeg invocation, ffprobe spawn, tool check, temp path helper"
```

---

## Task 4: ipod::device — read SysInfo, push FirewireGUID

**Files:**
- Modify: `F:\repos\ipod-sync\src\ipod\device.rs`
- Create: `F:\repos\ipod-sync\tests\fixtures\sample-sysinfo.txt`

**Revised 2026-05-17:** the target iPod (drive-modded 160 GB MB029 unit) has the **older flat-text `SysInfo`** format at `<mount>\iPod_Control\Device\SysInfo`, NOT the XML-plist `SysInfoExtended` that SPEC §4.6 anticipated. Real content:
```
FirewireGuid: 0x000A27002138B0A8
ModelNumStr: MB029
```

Parsing this is trivial — line-oriented, `key: value`. We code to this format only. A `SysInfoExtended` fallback would be needed for newer iPods (e.g. anyone else's later-restored Classic) but is deferred to Phase 2 or whenever the need arises. Document the limitation in LEARNINGS.md.

**Critical libgpod context (unchanged):** the libgpod we built has libplist stripped (Phase 0 Task 3 patch). That removed libgpod's own plist parser. We push FirewireGuid via `itdb_device_set_sysinfo(device, "FirewireGuid", value)` — a per-field setter that doesn't need libplist or XML parsing.

- [ ] **Step 1: Capture the iPod's real SysInfo as a test fixture**

With the iPod plugged in at `G:\`:
```powershell
Test-Path G:\iPod_Control\Device\SysInfo
Copy-Item G:\iPod_Control\Device\SysInfo F:\repos\ipod-sync\tests\fixtures\sample-sysinfo.txt
Get-Content F:\repos\ipod-sync\tests\fixtures\sample-sysinfo.txt
```

Expected: a few lines, ~50-150 bytes, of the form:
```
FirewireGuid: 0x000A27002138B0A8
ModelNumStr: MB029
```

The FirewireGuid value is hardware-bound (like a MAC address) but not secret — fine to commit. If `Test-Path` returns False, the iPod isn't mounted as MSC — investigate before continuing.

- [ ] **Step 2: Write the failing test in `src/ipod/device.rs`**

Replace `src/ipod/device.rs` with:
```rust
//! Read FirewireGUID from the iPod's SysInfo and push it into libgpod's
//! device struct so itdb_write computes a valid signed iTunesDB.

use anyhow::{anyhow, Result};
use std::path::Path;

/// Extract the value of the `FirewireGuid:` line from a SysInfo body.
/// Returns just the hex value (typically `0x...`).
pub fn extract_firewire_guid(sysinfo: &str) -> Result<String> {
    unimplemented!("Task 4 step 4")
}

/// Resolve `<mount>\iPod_Control\Device\SysInfo`, read it, extract FirewireGuid.
pub fn read_firewire_guid(ipod_mount: &Path) -> Result<String> {
    unimplemented!("Task 4 step 4")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../../tests/fixtures/sample-sysinfo.txt");

    #[test]
    fn extracts_firewire_guid_from_real_sample() {
        let guid = extract_firewire_guid(SAMPLE).expect("extract");
        // Classic uses a 16-hex-digit ID with 0x prefix.
        assert!(guid.starts_with("0x"), "expected hex prefix, got: {guid}");
        assert_eq!(guid.len(), 18, "expected 0x + 16 hex chars, got len {}: {guid}", guid.len());
        assert!(guid[2..].chars().all(|c| c.is_ascii_hexdigit()),
            "expected hex digits, got: {guid}");
    }

    #[test]
    fn errors_on_missing_key() {
        let sysinfo = "ModelNumStr: MB029\nOther: value\n";
        assert!(extract_firewire_guid(sysinfo).is_err());
    }

    #[test]
    fn errors_on_missing_value() {
        let sysinfo = "FirewireGuid:\nModelNumStr: MB029\n";
        assert!(extract_firewire_guid(sysinfo).is_err());
    }

    #[test]
    fn ignores_lines_starting_with_firewire_guid_prefix_but_not_exact_key() {
        let sysinfo = "FirewireGuidSomething: 0xDEADBEEF\nFirewireGuid: 0x000A27002138B0A8\n";
        assert_eq!(
            extract_firewire_guid(sysinfo).unwrap(),
            "0x000A27002138B0A8"
        );
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

```powershell
cargo test ipod::device 2>&1 | Select-Object -Last 10
```
Expected: FAIL — panic on `unimplemented!`.

- [ ] **Step 4: Implement the two functions**

Replace the two `unimplemented!()` bodies:
```rust
pub fn extract_firewire_guid(sysinfo: &str) -> Result<String> {
    // SysInfo is line-oriented `Key: value`. We want the value for the exact
    // key `FirewireGuid` (case-sensitive — matches how iTunes writes it).
    for line in sysinfo.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        if key.trim() != "FirewireGuid" { continue; }
        let value = value.trim();
        if value.is_empty() {
            return Err(anyhow!("FirewireGuid line has no value: {line:?}"));
        }
        return Ok(value.to_string());
    }
    Err(anyhow!("FirewireGuid key not found in SysInfo"))
}

pub fn read_firewire_guid(ipod_mount: &Path) -> Result<String> {
    let path = ipod_mount
        .join("iPod_Control")
        .join("Device")
        .join("SysInfo");
    let body = std::fs::read_to_string(&path)
        .map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
    extract_firewire_guid(&body)
}
```

- [ ] **Step 5: Run tests to verify they pass**

```powershell
cargo test ipod::device 2>&1 | Select-Object -Last 10
```
Expected: 3 tests pass.

- [ ] **Step 6: Add the libgpod-side helper that pushes the GUID into the device struct**

This is the FFI half. It can't be unit-tested without the device — exercise it in Task 6. Append to `src/ipod/device.rs`:
```rust
use crate::ffi;
use std::ffi::CString;

/// Push the FirewireGuid into libgpod's `Itdb_Device` struct via the per-field
/// setter (we can't use `itdb_device_read_sysinfo_xml` because libplist is
/// stripped from our libgpod build).
///
/// # Safety
/// `device` must be a valid `*mut Itdb_Device` obtained from libgpod
/// (e.g. via `(*db.as_ptr()).device` after a successful `itdb_parse_file`).
pub unsafe fn set_firewire_guid(
    device: *mut ffi::Itdb_Device,
    guid: &str,
) -> anyhow::Result<()> {
    if device.is_null() {
        return Err(anyhow!("Itdb_Device pointer is NULL"));
    }
    let key = CString::new("FirewireGuid").unwrap();
    let value = CString::new(guid)
        .map_err(|_| anyhow!("FirewireGuid contains interior NUL byte"))?;
    ffi::itdb_device_set_sysinfo(device, key.as_ptr(), value.as_ptr());
    Ok(())
}
```

The bindgen allowlist `itdb_.*` already covers `itdb_device_set_sysinfo` — verify by searching the generated bindings:
```powershell
$bindings = Get-ChildItem F:\repos\ipod-sync\target\debug\build\ipod-sync-*\out\libgpod_bindings.rs | Select-Object -First 1
Select-String -Path $bindings.FullName -Pattern "itdb_device_set_sysinfo"
```
Expected: matches found. If not (libgpod version doesn't export this name), search for alternatives:
```powershell
Select-String -Path $bindings.FullName -Pattern "itdb_device_set" | Select-Object -First 5
```
and adjust the FFI call. Document the actual symbol used in LEARNINGS.md.

- [ ] **Step 7: Build to confirm the FFI compiles**

```powershell
cargo build 2>&1 | Select-Object -Last 3
```
Expected: clean build.

- [ ] **Step 8: Commit**

```powershell
git -C F:\repos\ipod-sync add src\ipod\device.rs tests\fixtures\sample-sysinfoextended.xml
git -C F:\repos\ipod-sync commit -m "feat(ipod::device): extract FirewireGuid from SysInfoExtended and push to libgpod"
```

---

## Task 5: ipod::db — OwnedDb RAII, Tags, write path

**Files:**
- Modify: `F:\repos\ipod-sync\src\ipod\db.rs`

This is the most complex task — the libgpod write path with all the FFI safety considerations. We unit-test the small pure-logic helpers; the FFI itself is exercised in Task 6.

- [ ] **Step 1: Write the type/helper definitions**

Replace `src/ipod/db.rs` with:
```rust
//! libgpod DB operations wrapped in RAII Rust types.

use crate::ffi;
use anyhow::{anyhow, Result};
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;

/// Owns an `Itdb_iTunesDB *` and frees it on Drop. Holds the database in memory
/// (libgpod's parse loads the whole thing). All write operations are methods.
pub struct OwnedDb(*mut ffi::Itdb_iTunesDB);

/// The metadata fields we copy into `Itdb_Track`. Parsed from ffprobe by main.rs.
#[derive(Debug, Default)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub year: Option<i32>,
    pub track_nr: Option<i32>,
    pub tracks: Option<i32>,
    pub disc_nr: Option<i32>,
    pub discs: Option<i32>,
}

impl OwnedDb {
    /// Parse the iTunesDB at `<ipod_mount>\iPod_Control\iTunes\iTunesDB`.
    pub fn open(ipod_mount: &Path) -> Result<Self> {
        let db_path = ipod_mount
            .join("iPod_Control")
            .join("iTunes")
            .join("iTunesDB");
        let path_c = path_to_cstring(&db_path)?;
        unsafe {
            let mut err: *mut ffi::GError = ptr::null_mut();
            let db = ffi::itdb_parse_file(path_c.as_ptr(), &mut err);
            if db.is_null() {
                return Err(gerror_to_anyhow("itdb_parse_file", err));
            }
            Ok(OwnedDb(db))
        }
    }

    pub fn as_ptr(&self) -> *mut ffi::Itdb_iTunesDB { self.0 }

    /// Number of tracks currently in the DB.
    pub fn track_count(&self) -> usize {
        unsafe { ffi::itdb_tracks_number(self.0) as usize }
    }

    /// Persist DB to the iPod. After this returns Ok, the iPod's stored DB on
    /// disk reflects the in-memory state (track adds, file copies, etc.).
    pub fn write(&self) -> Result<()> {
        unsafe {
            let mut err: *mut ffi::GError = ptr::null_mut();
            if ffi::itdb_write(self.0, &mut err) == 0 {
                return Err(gerror_to_anyhow("itdb_write", err));
            }
        }
        Ok(())
    }

    /// Copy `source_alac` onto the iPod, attach metadata `tags`, add to the
    /// master playlist. Does NOT call `itdb_write` — call `write()` separately
    /// so the caller controls when the DB is flushed.
    ///
    /// On failure mid-way (file copied but playlist add fails), the file is
    /// left on the iPod orphaned — Phase 2's `--rebuild-manifest` recovers
    /// from this kind of state. Phase 1 just surfaces the error.
    pub fn add_track_with_file(&self, source_alac: &Path, tags: &Tags) -> Result<()> {
        let alac_c = path_to_cstring(source_alac)?;
        unsafe {
            let track = ffi::itdb_track_new();
            if track.is_null() {
                return Err(anyhow!("itdb_track_new returned NULL"));
            }
            apply_tags(track, tags);

            let mut err: *mut ffi::GError = ptr::null_mut();
            if ffi::itdb_cp_track_to_ipod(track, alac_c.as_ptr(), &mut err) == 0 {
                // The track was not added to the DB; we own it and must free.
                ffi::itdb_track_free(track);
                return Err(gerror_to_anyhow("itdb_cp_track_to_ipod", err));
            }
            // cp_track adds the track to db.tracks; we still need to add it to
            // the master playlist for it to show in the iPod's Songs menu.
            let master = ffi::itdb_playlist_mpl(self.0);
            if master.is_null() {
                return Err(anyhow!("master playlist missing on this iPod (corrupt DB?)"));
            }
            ffi::itdb_playlist_add_track(master, track, -1);
        }
        Ok(())
    }
}

impl Drop for OwnedDb {
    fn drop(&mut self) {
        unsafe { ffi::itdb_free(self.0); }
    }
}

/// Copy each set field from `tags` into the corresponding `Itdb_Track` slot.
/// Strings are duplicated via `g_strdup` so libgpod owns them and frees with
/// the track. Numeric fields are written directly. Unset Optional fields leave
/// the libgpod default (typically 0 or NULL).
///
/// # Safety
/// `track` must be a freshly-allocated `Itdb_Track *` from `itdb_track_new`.
unsafe fn apply_tags(track: *mut ffi::Itdb_Track, tags: &Tags) {
    set_str(&mut (*track).title, tags.title.as_deref());
    set_str(&mut (*track).artist, tags.artist.as_deref());
    set_str(&mut (*track).album, tags.album.as_deref());
    set_str(&mut (*track).albumartist, tags.album_artist.as_deref());
    set_str(&mut (*track).genre, tags.genre.as_deref());
    set_str(&mut (*track).composer, tags.composer.as_deref());
    if let Some(y) = tags.year { (*track).year = y; }
    if let Some(n) = tags.track_nr { (*track).track_nr = n; }
    if let Some(t) = tags.tracks { (*track).tracks = t; }
    if let Some(n) = tags.disc_nr { (*track).cd_nr = n; }
    if let Some(t) = tags.discs { (*track).cds = t; }
}

/// Replace `*slot` with a g_strdup of `value`, freeing whatever was there.
/// `g_free(NULL)` is a documented no-op.
unsafe fn set_str(slot: *mut *mut std::os::raw::c_char, value: Option<&str>) {
    ffi::g_free(*slot as *mut std::os::raw::c_void);
    *slot = match value {
        Some(s) => {
            // FLAC tags should not contain interior NULs but defend against it:
            // CString::new fails on NUL, in which case we skip the tag rather
            // than silently truncate or panic.
            match CString::new(s) {
                Ok(c) => ffi::g_strdup(c.as_ptr()),
                Err(_) => ptr::null_mut(),
            }
        }
        None => ptr::null_mut(),
    };
}

fn path_to_cstring(p: &Path) -> Result<CString> {
    let s = p.to_str()
        .ok_or_else(|| anyhow!("path contains non-UTF-8: {}", p.display()))?;
    CString::new(s)
        .map_err(|_| anyhow!("path contains interior NUL byte: {}", p.display()))
}

unsafe fn gerror_to_anyhow(api: &str, err: *mut ffi::GError) -> anyhow::Error {
    if err.is_null() {
        return anyhow!("{api} failed (no error detail)");
    }
    let msg = CStr::from_ptr((*err).message).to_string_lossy().into_owned();
    ffi::g_error_free(err);
    anyhow!("{api} failed: {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn path_to_cstring_accepts_ascii() {
        let p = PathBuf::from(r"C:\foo\bar.m4a");
        let c = path_to_cstring(&p).expect("ascii path converts");
        assert_eq!(c.to_str().unwrap(), r"C:\foo\bar.m4a");
    }

    #[test]
    fn path_to_cstring_accepts_unc() {
        let p = PathBuf::from(r"\\server\share\file.flac");
        let c = path_to_cstring(&p).expect("UNC path converts");
        assert_eq!(c.to_str().unwrap(), r"\\server\share\file.flac");
    }

    #[test]
    fn tags_default_is_all_none() {
        let t = Tags::default();
        assert!(t.title.is_none());
        assert!(t.artist.is_none());
        assert!(t.year.is_none());
    }
}
```

- [ ] **Step 2: Verify the bindgen output exposes the symbols we need**

Before building, sanity-check the generated bindings contain everything the new code references:
```powershell
$bindings = Get-ChildItem F:\repos\ipod-sync\target\debug\build\ipod-sync-*\out\libgpod_bindings.rs | Select-Object -First 1
$needed = "itdb_track_new", "itdb_track_free", "itdb_cp_track_to_ipod", "itdb_playlist_mpl", "itdb_playlist_add_track", "itdb_tracks_number", "g_strdup", "g_free"
foreach ($name in $needed) {
    $hit = Select-String -Path $bindings.FullName -Pattern "\b$name\b" -Quiet
    "$name : $(if ($hit) { 'OK' } else { 'MISSING' })"
}
```
Expected: every entry says OK. If any are MISSING, the symbol either has a different name in the libgpod version we built (search the bindings for similar prefixes and adjust) or isn't exported (in which case escalate — patching libgpod again is beyond Phase 1 scope without controller approval).

Particularly verify the `Itdb_Track` struct field names referenced by `apply_tags`: `title`, `artist`, `album`, `albumartist`, `genre`, `composer`, `year`, `track_nr`, `tracks`, `cd_nr`, `cds`. Search:
```powershell
Select-String -Path $bindings.FullName -Pattern "pub struct Itdb_Track\b" -Context 0,60 | Out-String -Width 200
```
Cross-reference each field. If any differ (e.g. `disc_nr` instead of `cd_nr`), update `apply_tags` to use the actual names. Document substitutions in LEARNINGS.md.

- [ ] **Step 3: Build and test**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test ipod::db 2>&1 | Select-Object -Last 10
```
Expected: clean build, 3 tests pass.

- [ ] **Step 4: Commit**

```powershell
git -C F:\repos\ipod-sync add src\ipod\db.rs
git -C F:\repos\ipod-sync commit -m "feat(ipod::db): OwnedDb RAII + Tags + add_track_with_file write path"
```

---

## Task 6: main.rs orchestrator + hardware run

**Files:**
- Modify: `F:\repos\ipod-sync\src\main.rs`

This task wires everything together and runs against the real iPod. The "test" for this task is the iPod itself — output a new track that plays.

- [ ] **Step 1: Replace `src/main.rs` with the orchestrator**

```rust
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
    let source = parse_arg()?;
    println!("Source FLAC : {}", source.display());

    transcode::verify_tools_available()?;
    let probe_data = probe(&source)?;
    println!(
        "Source has embedded art: {}",
        if has_embedded_art(&probe_data) { "yes" } else { "no" }
    );

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
    db.add_track_with_file(&temp, &tags)?;

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
```

- [ ] **Step 2: Build and run unit tests**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test 2>&1 | Select-Object -Last 15
```
Expected: clean build, all tests pass (we now have ~13 tests across transcode, ipod::db, ipod::device, main).

- [ ] **Step 3: Confirm the iPod is plugged in and check state**

```powershell
Test-Path G:\iPod_Control\iTunes\iTunesDB
Get-Item G:\iPod_Control\iTunes\iTunesDB | Select-Object Length, LastWriteTime
```
Expected: True, with a non-zero Length and a recent LastWriteTime (from Phase 0 — or from this session if you've touched it). Record the LastWriteTime — it should change after we write.

If the iPod is not at G:, **stop and report**. The plan hardcodes G:\; running against a wrong drive could corrupt unintended data.

- [ ] **Step 4: Run the spike on a real FLAC the user supplies**

The user will paste a FLAC path. Either pass it as a CLI argument:
```powershell
cargo run -- "<path-to-source.flac>"
```

If the user wants to skip the recompile overhead, build once then call directly:
```powershell
cargo build 2>&1 | Select-Object -Last 2
.\target\debug\ipod-sync.exe "<path-to-source.flac>"
```

Capture the full output. Expected (rough shape):
```
Source FLAC : C:\...\song.flac
Source has embedded art: yes
Title       : <title>
Artist      : <artist>
Album       : <album>
Transcoding to C:\Users\...\AppData\Local\Temp\ipod-sync\ipod-sync-<pid>.m4a ...
Opening iPod DB at G:\
Existing track count: 1
Wiring FirewireGuid for write signing...
Adding track to DB...
Writing DB to iPod (this signs the hashed iTunesDB)...
Deleting temp file...
New track count: 2
Done. Eject the iPod and verify on device.
```

Note: `New track count: 2` because Beck's "Colors" from Phase 0 is preserved.

If `itdb_write` fails, capture the GError message verbatim. Common failure modes:
- `Hash error` / `Signature error` → the FirewireGuid wiring didn't work. Try without it (comment out the `set_firewire_guid` block and re-run; if THAT works, we've discovered FirewireGuid setup was the problem). Document the outcome.
- `Permission denied` → AV is locking files on G:; add `G:\iPod_Control` to Defender exclusions and retry.
- Crash / access violation → likely struct layout mismatch (different libgpod build than headers); escalate.

- [ ] **Step 5: Inspect the iPod's iTunesDB after the run**

```powershell
Get-Item G:\iPod_Control\iTunes\iTunesDB | Select-Object Length, LastWriteTime
Get-ChildItem G:\iPod_Control\Music -Recurse -Filter *.m4a | Select-Object Name, Length, FullName
```
Expected: `iTunesDB`'s LastWriteTime is now within the last minute. A new `.m4a` file appears under `G:\iPod_Control\Music\Fnn\` (libgpod picks the folder + 4-char filename — both are correct).

- [ ] **Step 6: Verify the iTunesDB still parses (sanity check before ejecting)**

Run the old Phase 0 spike code by temporarily checking it out... actually easier: re-run our Phase 1 binary with no args (it errors on missing arg) — no, that doesn't help.

Instead, re-run Phase 1 to read the DB (it'll fail at "create new track" or similar, but the parse + GUID + write will work). Or write a tiny sanity check by running the original Phase 0 binary if you have it.

Simplest: just rebuild + run with the source again. If `itdb_parse_file` succeeds and the existing-count is now 2, the DB is well-formed.

Actually the SIMPLEST sanity check: run a second time with a DIFFERENT FLAC (if you have one). If that succeeds and gets to count: 3, the DB round-trip works.

If you only have one test FLAC, skip this and proceed to Step 7 — the device boot is the real test.

- [ ] **Step 7: Commit (BEFORE eject so the commit reflects the state that produced the working device)**

```powershell
git -C F:\repos\ipod-sync add src\main.rs
git -C F:\repos\ipod-sync commit -m "feat: Phase 1 orchestrator — transcode FLAC and write to iPod"
```

- [ ] **Step 8: Hand off to user for physical verification**

The implementer cannot perform Steps 9-11 — these are physical actions by the user. STOP HERE and report DONE_WITH_CONCERNS, listing the verbatim cargo run output and asking the user to eject + verify on device. The controller will pick up Task 7.

The remaining steps (eject + verify) are owned by Task 7's gate review.

---

## Task 7: Phase 1 gate review (hardware-dependent)

**Files:**
- Modify: `F:\repos\ipod-sync\LEARNINGS.md`

This task is the controller + user pair. The implementer's job ended at Task 6 Step 7.

- [ ] **Step 1: Acceptance checklist (user performs)**

The user ejects the iPod cleanly (right-click G: → Eject; wait for "Safe to remove"), unplugs, and physically verifies:

1. iPod boots to its normal menu (does NOT display the "connect to iTunes" / sad iPod screen).
2. Music → Songs lists both tracks: Beck "Colors" (from Phase 0) AND the new Phase 1 track.
3. The new Phase 1 track plays. Audio is correct.
4. The new Phase 1 track shows correct title/artist/album on the Now Playing screen.
5. The new Phase 1 track shows the album art on the Now Playing screen.

ALL FIVE must pass for the gate.

If 1 fails (iPod doesn't boot): SPEC §8 row 2 risk materialized. Restore via iTunes 12.6.5.3, then iterate on the FirewireGuid path (try without; try `itdb_device_read_sysinfo_xml` if it exists; investigate `_RID_` / SystemInfo other fields). Phase 1 gate FAIL — re-plan.

If 1 passes but 2/3 fails (track missing or won't play): write succeeded but the file or DB entry is malformed. Check `G:\iPod_Control\Music\` for the file. May be a metadata issue with `itdb_cp_track_to_ipod` — investigate.

If 1-3 pass but 4 fails (wrong metadata): the `apply_tags` field mapping is wrong. Check which fields show vs are blank. Document and re-iterate Task 6.

If 1-4 pass but 5 fails (no art on Now Playing): SPEC §8 row 3 — ffmpeg in-band art rejected by iPod. Fall back to Plan B: use libgpod's `itdb_track_set_thumbnails_from_data` after `itdb_cp_track_to_ipod`. Extract the cover via `ffmpeg -map 0:v:0 -c:v copy -f image2 <tempart.jpg>` and load. Add as Task 6b and re-iterate.

- [ ] **Step 2: Record the gate result in LEARNINGS.md**

Append to `F:\repos\ipod-sync\LEARNINGS.md` (controller performs once user reports):
```markdown
## Phase 1 gate (YYYY-MM-DD) — PASS / FAIL

- **Result:** PASS / FAIL (<reason if fail>)
- **Test track:** <artist - album - title> (source path <path>)
- **iPod state before:** 1 track (Beck "Colors" from Phase 0).
- **iPod state after:** <N> tracks: <list>.
- **iTunesDB write:** PASS — itdb_write returned success; LastWriteTime updated.
- **FirewireGuid wiring:** required / not required (see note).
- **Album art (Plan A in-band):** displayed / not displayed.
- **Album art (Plan B fallback used?):** yes / no.
- **iPod post-eject boot:** boots normally / sad iPod / connect-to-iTunes screen.
- **Playback verified on device:** yes / no.
- **Issues to address in Phase 2:** <list, or "none">.
```

- [ ] **Step 3: Commit and tag**

```powershell
git -C F:\repos\ipod-sync add LEARNINGS.md
git -C F:\repos\ipod-sync commit -m "docs: Phase 1 gate result"
git -C F:\repos\ipod-sync tag -a phase-1-complete -m "Single-track end-to-end write verified on Classic 7G"
```

- [ ] **Step 4: Hand off to Phase 2 planning**

If gate passed: write the Phase 2 plan (`docs/superpowers/plans/YYYY-MM-DD-phase-2-full-tool.md`) per SPEC §4: source walker, manifest, diff, deletions, CLI flags, mount auto-detect, TUI, progress, full 1,400-track run.

If gate failed: do not proceed to Phase 2. Address the failure mode first (likely a same-day iteration on Task 6 or a Task 6b fallback). Re-test until gate passes.

---

## Self-review

**Spec coverage (Phase 1 scope per SPEC §12.1):**
- "hardcoded source FLAC" — Task 6 main.rs takes it as CLI arg (slight scope expansion: easier to iterate without recompile, no real cost — confirmed acceptable in brainstorming).
- "ffmpeg" — Task 3.
- "temp ALAC" — Task 3 (`temp_alac_path`), Task 6 (create + cleanup).
- "itdb_cp_track_to_ipod" — Task 5 (`add_track_with_file`).
- "metadata populated" — Task 5 (`apply_tags`), Task 6 (`tags_from_probe`).
- "art attached" — Task 3 (`-c:v copy -disposition:v attached_pic -f ipod` in ffmpeg_args).
- "itdb_write" — Task 5 (`OwnedDb::write`), Task 6 (orchestrator calls it).
- "eject + physically verify" — Task 7.

**SPEC §12 design decisions cross-checked:**
- §12 #1 (temp file not pipe) — `temp_alac_path()` in Task 3, deleted in Task 6 Step orchestrator. ✓
- §12 #2 (ffprobe for metadata) — Task 2 + 3. ✓
- §12 #3 (in-band art first, libgpod-thumbnails fallback) — Plan A in Task 3 ffmpeg args; Plan B documented in Task 7 fallback path. ✓
- §12 #5 (ffmpeg primary, refalac fallback) — ffmpeg in Task 3; refalac out of scope here, documented as Phase-X. ✓
- §12 #6 (ratatui Phase 2, not Phase 1) — Phase 1 prints plain lines, no UI. ✓
- §12.7 (libgpod via MSYS2/MinGW) — Phase 0 vendored; Phase 1 just uses what's there. ✓

**SPEC §8 risks addressed:**
- Row 1 (libgpod buildable on Windows) — closed by Phase 0.
- Row 2 (hashed signature) — Task 4 wires FirewireGuid; Task 6 exercises the write; Task 7 gate confirms.
- Row 3 (ffmpeg cover art rejected by iPod) — Task 7 has the Plan B fallback ready if Plan A fails.
- Row 4 (SMB latency) — not in Phase 1 scope (single track).
- Row 5 (concurrent device access) — partially addressed: an `itdb_write` failure aborts and leaves an orphan file on the iPod (documented). Phase 2's `--rebuild-manifest` recovers.

**Placeholder scan:** Every `unimplemented!()` is in a "Step 2: write failing test" position with its implementation given in the next step (the TDD pattern). Every step that shows test assertions or struct shapes uses concrete values from real fixtures (the user's actual SysInfoExtended, a real ffprobe output). No "TBD" or "add error handling later" anywhere.

**Type consistency check:**
- `OwnedDb`, `Tags` (`ipod::db`) — used identically across Task 5 + Task 6.
- `ProbeOutput`, `ProbeTags`, `has_embedded_art`, `probe`, `transcode_to_alac`, `verify_tools_available`, `temp_alac_path` (`transcode`) — defined in Tasks 2-3, consumed in Task 6.
- `read_firewire_guid`, `set_firewire_guid`, `extract_firewire_guid` (`ipod::device`) — defined in Task 4, consumed in Task 6.
- Field name `cd_nr` (used by libgpod for "disc number") is consistent between `apply_tags` (Task 5) and the assumption — Task 5 Step 2's bindgen verification confirms or substitutes the actual name.

**Scope check:** Tasks are sequential and one-track-focused. No Phase 2 features (manifest, walker, diff, CLI flags, deletions, TUI) creep in. The CLI-arg-instead-of-hardcoded is a minor expansion but doesn't add a Phase 2 dep — it's just `std::env::args` not clap.
