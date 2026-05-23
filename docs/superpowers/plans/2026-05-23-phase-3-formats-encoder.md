# Phase 3: Formats + Encoder Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Tasks marked `(parallel-safe with Task N)` can be dispatched concurrently to independent implementer subagents.

**Goal:** Stop transcoding every source file through ffmpeg→ALAC. (a) iPod-native source codecs (MP3, AAC/M4A, ALAC) get **passthrough**: byte-for-byte copy via `std::fs::copy`. (b) Non-native sources (FLAC, OGG, Opus, WAV-when-`--passthrough-wav`-not-set) get **transcoded to ALAC** via the user-selected encoder (`ffmpeg` default, or `refalac` opt-in via `--encoder refalac`). Manifest records `encoder`, `encoder_version`, and `source_format` per entry so future runs can detect encoder-mismatch (and re-encode if asked) without disrupting Phase 2 manifest entries (which deserialize with safe defaults).

**Architecture:** A new `transcode::classify(&probe, &config)` function returns `SourceAction::Passthrough` or `SourceAction::Transcode` based on a codec_name × container decision matrix. `apply_loop::add_one` branches on the result: Passthrough → `transcode::passthrough` (fs::copy + libgpod tag/art write); Transcode + `encoder=ffmpeg` → existing `transcode::transcode_to_alac`; Transcode + `encoder=refalac` → new `transcode::transcode_via_refalac` (2-step: ffmpeg-decode-to-WAV, then refalac-encode-to-M4A). Encoder selection resolves once at preflight into a `ResolvedEncoder { name, version }` struct that gets propagated into every new ManifestEntry. The diff gains one new branch: encoder-mismatch on an otherwise-unchanged entry → `Action::Modify`, gated by `--force-reencode` and carve-outs for `"unknown"` (Phase 2 manifests) and `"passthrough"` (no encoder to mismatch).

**Tech Stack:** Rust stable (x86_64-pc-windows-msvc), existing ffmpeg + ffprobe + libgpod runtime DLLs. New runtime dependency: vendored `refalac64.exe` + `libFLAC.dll` under `vendor/refalac/`, copied to `target/<profile>/` by `build.rs`. No new Rust crate dependencies.

**Plan scope:** Phase 3 only. Does not touch Phase 3.x (metadata-only smart-update — already shipped) or Phase 3.z (TUI-first error UX — already shipped; Phase 3 reuses its `await_prompt` helper for the new refalac preflight). Does not touch Phase 4 (multi-iPod), Phase 5 (daemon), or Phase 6 (GUI).

**Gate:** end-to-end exercise on the user's mixed library against a real iPod 7G. Spec's 7 acceptance criteria executed manually (mixed-source dry-run preview, passthrough byte-for-byte verify, refalac transcode round-trip, encoder-mismatch triggers Modify, `--force-reencode` works, Phase 2 manifest upgrades cleanly, iPod-level acceptance across 5 tracks per source-codec category).

---

## File Structure

```
F:\repos\ipod-sync\
├── src\
│   ├── transcode.rs                  (modify: + SourceAction, classify, passthrough,
│   │                                            transcode_via_refalac, codec_name on ProbeStream,
│   │                                            ResolvedEncoder, resolve_encoder, encoder_version probes,
│   │                                            temp_wav_path, verify_refalac)
│   ├── cli.rs                        (modify: + --encoder, --refalac-path, --passthrough-wav,
│   │                                            --force-reencode; EncoderChoice enum w/ clap::ValueEnum)
│   ├── config.rs                     (modify: + encoder, refalac_path, passthrough_wav,
│   │                                            force_reencode fields; resolve them)
│   ├── config_file.rs                (modify: + persisted encoder / passthrough_wav / refalac_path)
│   ├── manifest.rs                   (modify: + encoder, encoder_version, source_format on
│   │                                            ManifestEntry; is_encoder_mismatch + force-reencode
│   │                                            branch in diff)
│   ├── preflight.rs                  (modify: + verify_refalac gate, called when encoder=refalac)
│   ├── apply_loop.rs                 (modify: add_one branches on classify; transcode/passthrough
│   │                                            paths; new fields recorded in entry_from)
│   └── lib.rs                        (no change — modules already re-exported)
├── build.rs                          (modify: copy vendor/refalac/*.exe + *.dll to target/)
├── vendor\
│   └── refalac\                      (new dir: refalac64.exe + libFLAC.dll, vendored)
│       ├── refalac64.exe             (vendored — see Task 5 for sourcing instructions)
│       └── libFLAC.dll               (vendored — bundled with qaac releases)
├── tests\fixtures\
│   ├── sample-ffprobe-mp3.json       (new: synthetic mp3 ffprobe output for classify tests)
│   ├── sample-ffprobe-aac.json       (new)
│   ├── sample-ffprobe-alac.json      (new)
│   ├── sample-ffprobe-vorbis.json    (new)
│   ├── sample-ffprobe-opus.json      (new)
│   ├── sample-ffprobe-wav.json       (new)
│   └── sample-ffprobe-unknown.json   (new: e.g. ac3 — should error from classify)
└── LEARNINGS.md                      (modify: Phase 3 gate result + decisions captured)
```

### Module responsibility delta

- **`transcode`** — gains `SourceAction { Passthrough, Transcode }`, `classify(probe, config) -> Result<SourceAction>` (the codec×container decision matrix), `passthrough(src, dst) -> Result<()>` (fs::copy with parent-dir mkdirs), `transcode_via_refalac(src, dst, art_jpg_opt, refalac_path) -> Result<()>` (ffmpeg-decode-to-WAV, then refalac-encode-to-M4A with optional `--artwork`), `temp_wav_path() -> PathBuf`, `verify_refalac(refalac_path) -> Result<()>` (startup probe), `ResolvedEncoder { name, version }` + `resolve_encoder(config) -> Result<ResolvedEncoder>` (resolves the choice into a name + version string for the manifest). `ProbeStream` gains a `codec_name: Option<String>` field so classify can read it; `ProbeFormat` gains `format_name: Option<String>` for container detection.
- **`cli`** — adds 4 new flags via clap. New `EncoderChoice` enum derives `clap::ValueEnum` so `--encoder ffmpeg|refalac` parses.
- **`config`** — adds `encoder: EncoderChoice`, `refalac_path: PathBuf`, `passthrough_wav: bool`, `force_reencode: bool` to `Config`. `resolve_with` merges them from CLI → persisted → defaults.
- **`config_file`** — `PersistedConfig` grows `encoder`, `passthrough_wav`, `refalac_path` (NOT `force_reencode` — that's intentionally CLI-only, see Task 3).
- **`manifest`** — `ManifestEntry` gains `encoder: String`, `encoder_version: String`, `source_format: String` with serde defaults that make Phase 2 manifests deserialize cleanly. `diff` signature grows a `force_reencode: bool` parameter and a `target_encoder: &str` parameter; internal `is_encoder_mismatch` helper compares each entry's `encoder` against `target_encoder` with carve-outs for `unknown` / `passthrough`. When mismatch on an otherwise-Unchanged entry → `Action::Modify`. Existing test fixtures upgrade with explicit field values.
- **`preflight`** — `verify_refalac(config, progress, decision_rx)` is a new gate, conditional on `config.encoder == EncoderChoice::Refalac`. Uses the Phase 3.z `await_prompt` helper for Retry/Abort on failure. `verify_ffmpeg` stays unchanged.
- **`apply_loop`** — `run` now resolves the encoder via `transcode::resolve_encoder(&config)` immediately after preflight and passes the resolved name into `diff` (so encoder-mismatch detection can compare against the right target). `add_one` calls `transcode::classify(&probe, config)` after `probe`; on `Passthrough` → `transcode::passthrough`; on `Transcode + ffmpeg` → existing `transcode_to_alac`; on `Transcode + refalac` → `transcode_via_refalac`. Recorded ManifestEntry includes `encoder`, `encoder_version`, `source_format`. `entry_from` signature grows to accept those three new strings.

---

## Task 1: `SourceAction` + `transcode::classify` + per-codec ffprobe fixtures + tests (parallel-safe with Task 3, Task 4, Task 5)

**Files:**
- Modify: `F:\repos\ipod-sync\src\transcode.rs`
- New: `F:\repos\ipod-sync\tests\fixtures\sample-ffprobe-mp3.json`
- New: `F:\repos\ipod-sync\tests\fixtures\sample-ffprobe-aac.json`
- New: `F:\repos\ipod-sync\tests\fixtures\sample-ffprobe-alac.json`
- New: `F:\repos\ipod-sync\tests\fixtures\sample-ffprobe-vorbis.json`
- New: `F:\repos\ipod-sync\tests\fixtures\sample-ffprobe-opus.json`
- New: `F:\repos\ipod-sync\tests\fixtures\sample-ffprobe-wav.json`
- New: `F:\repos\ipod-sync\tests\fixtures\sample-ffprobe-unknown.json`

Pure-logic addition: the decision matrix from the spec, exercised against synthetic ffprobe fixtures (one per source codec). No new runtime behavior; just a function and tests. **Depends only on `ProbeOutput` — no config wiring yet.**

This task introduces a small helper `ClassifyConfig { passthrough_wav: bool }` so the function doesn't need the full `Config` (which Task 3 hasn't added the encoder fields to yet). `apply_loop` will construct this from `&Config` in Task 6.

- [ ] **Step 1: Extend ProbeStream / ProbeFormat for classify inputs**

In `src/transcode.rs`, add `codec_name` to `ProbeStream` and `format_name` to `ProbeFormat`:

```rust
#[derive(Debug, Deserialize)]
pub struct ProbeStream {
    pub codec_type: String,
    #[serde(default)]
    pub codec_name: Option<String>,         // NEW — e.g. "flac", "mp3", "aac", "alac", "vorbis", "opus", "pcm_s16le"
    #[serde(default)]
    pub disposition: Option<ProbeDisposition>,
}

#[derive(Debug, Deserialize)]
pub struct ProbeFormat {
    #[serde(default)]
    pub format_name: Option<String>,        // NEW — comma-separated e.g. "mov,mp4,m4a,3gp,3g2,mj2"
    pub tags: Option<ProbeTags>,
}
```

Both are `Option<String>` + `#[serde(default)]` so existing fixtures (including `sample-ffprobe.json`, used by the Phase 1 tests) deserialize unchanged.

- [ ] **Step 2: Define `SourceAction` + `ClassifyConfig`**

Add to `src/transcode.rs` near the top:

```rust
/// Outcome of `classify` — tells `apply_loop::add_one` which pipeline to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAction {
    /// Copy the source byte-for-byte; iPod plays it natively.
    Passthrough,
    /// Decode + re-encode to ALAC via the configured encoder.
    Transcode,
}

/// Subset of Config that classify needs. Keeps the signature small and lets
/// classify stay in `transcode` without a circular dep on `config`.
#[derive(Debug, Clone, Copy)]
pub struct ClassifyConfig {
    pub passthrough_wav: bool,
}
```

- [ ] **Step 3: Implement `classify`**

```rust
/// Decision matrix per the Phase 3 spec § "Source classification":
///
/// | codec_name           | container               | action       |
/// |----------------------|-------------------------|--------------|
/// | flac                 | flac                    | Transcode    |
/// | mp3                  | mp3                     | Passthrough  |
/// | aac                  | m4a / mp4 / aac / mov   | Passthrough  |
/// | alac                 | m4a / mp4 / mov         | Passthrough  |
/// | pcm_s16le / s24le etc| wav / aiff              | Passthrough  |
/// |                      |                         | iff config.passthrough_wav else Transcode |
/// | vorbis               | ogg                     | Transcode    |
/// | opus                 | opus / ogg              | Transcode    |
/// | (anything else)      | *                       | Err          |
///
/// `format.format_name` is comma-separated for multi-format containers — we
/// split + match any component.
pub fn classify(probe: &ProbeOutput, config: &ClassifyConfig) -> Result<SourceAction> {
    let audio_codec = probe.streams.iter()
        .find(|s| s.codec_type == "audio")
        .and_then(|s| s.codec_name.as_deref())
        .ok_or_else(|| anyhow!("classify: no audio stream / codec_name in probe"))?;

    let containers: Vec<&str> = probe.format.format_name.as_deref()
        .map(|s| s.split(',').map(|x| x.trim()).collect())
        .unwrap_or_default();
    let in_container = |c: &str| containers.iter().any(|x| *x == c);

    // PCM family is open-ended (s16le, s24le, s32le, f32le, ...). Treat any
    // pcm_* codec as a single "pcm" bucket for the WAV/AIFF decision.
    let is_pcm = audio_codec.starts_with("pcm_");

    let action = match audio_codec {
        "flac" if in_container("flac") => SourceAction::Transcode,
        "mp3"  if in_container("mp3")  => SourceAction::Passthrough,
        "aac"  if in_container("m4a") || in_container("mp4")
                || in_container("aac") || in_container("mov") => SourceAction::Passthrough,
        "alac" if in_container("m4a") || in_container("mp4")
                || in_container("mov") => SourceAction::Passthrough,
        "vorbis" if in_container("ogg") => SourceAction::Transcode,
        "opus"   if in_container("opus") || in_container("ogg") => SourceAction::Transcode,
        c if is_pcm && (in_container("wav") || in_container("aiff")) => {
            if config.passthrough_wav { SourceAction::Passthrough } else { SourceAction::Transcode }
        }
        _ => return Err(anyhow!(
            "unsupported source: codec_name={audio_codec}, container={containers:?}.\n\
             ipod-sync v1 handles: flac, mp3, aac, alac, vorbis, opus, pcm (wav/aiff).\n\
             AC3, WMA, and other formats are out of scope."
        )),
    };

    Ok(action)
}
```

- [ ] **Step 4: Create the per-codec ffprobe fixtures**

Each fixture is a minimal ffprobe JSON with just the fields classify reads. Example for `sample-ffprobe-mp3.json`:

```json
{
  "streams": [
    { "codec_type": "audio", "codec_name": "mp3" }
  ],
  "format": {
    "format_name": "mp3",
    "tags": { "TITLE": "Test", "ARTIST": "Test" }
  }
}
```

Repeat for each codec. Container values to use per spec:

| Fixture | codec_name | format_name |
|---|---|---|
| `sample-ffprobe-mp3.json` | `mp3` | `mp3` |
| `sample-ffprobe-aac.json` | `aac` | `mov,mp4,m4a,3gp,3g2,mj2` |
| `sample-ffprobe-alac.json` | `alac` | `mov,mp4,m4a,3gp,3g2,mj2` |
| `sample-ffprobe-vorbis.json` | `vorbis` | `ogg` |
| `sample-ffprobe-opus.json` | `opus` | `ogg` |
| `sample-ffprobe-wav.json` | `pcm_s16le` | `wav` |
| `sample-ffprobe-unknown.json` | `ac3` | `ac3` |

- [ ] **Step 5: Tests for classify**

Append to the existing `mod tests` in `transcode.rs`:

```rust
    const FX_MP3:     &str = include_str!("../tests/fixtures/sample-ffprobe-mp3.json");
    const FX_AAC:     &str = include_str!("../tests/fixtures/sample-ffprobe-aac.json");
    const FX_ALAC:    &str = include_str!("../tests/fixtures/sample-ffprobe-alac.json");
    const FX_VORBIS:  &str = include_str!("../tests/fixtures/sample-ffprobe-vorbis.json");
    const FX_OPUS:    &str = include_str!("../tests/fixtures/sample-ffprobe-opus.json");
    const FX_WAV:     &str = include_str!("../tests/fixtures/sample-ffprobe-wav.json");
    const FX_UNKNOWN: &str = include_str!("../tests/fixtures/sample-ffprobe-unknown.json");
    // Re-use the existing SAMPLE constant for FLAC.

    fn cc(passthrough_wav: bool) -> ClassifyConfig {
        ClassifyConfig { passthrough_wav }
    }

    fn parse(s: &str) -> ProbeOutput { serde_json::from_str(s).unwrap() }

    #[test]
    fn classify_flac_is_transcode() {
        assert_eq!(classify(&parse(SAMPLE), &cc(false)).unwrap(), SourceAction::Transcode);
    }

    #[test]
    fn classify_mp3_is_passthrough() {
        assert_eq!(classify(&parse(FX_MP3), &cc(false)).unwrap(), SourceAction::Passthrough);
    }

    #[test]
    fn classify_aac_in_m4a_container_is_passthrough() {
        assert_eq!(classify(&parse(FX_AAC), &cc(false)).unwrap(), SourceAction::Passthrough);
    }

    #[test]
    fn classify_alac_in_m4a_container_is_passthrough() {
        assert_eq!(classify(&parse(FX_ALAC), &cc(false)).unwrap(), SourceAction::Passthrough);
    }

    #[test]
    fn classify_vorbis_is_transcode() {
        assert_eq!(classify(&parse(FX_VORBIS), &cc(false)).unwrap(), SourceAction::Transcode);
    }

    #[test]
    fn classify_opus_is_transcode() {
        assert_eq!(classify(&parse(FX_OPUS), &cc(false)).unwrap(), SourceAction::Transcode);
    }

    #[test]
    fn classify_wav_default_is_transcode() {
        assert_eq!(classify(&parse(FX_WAV), &cc(false)).unwrap(), SourceAction::Transcode);
    }

    #[test]
    fn classify_wav_with_passthrough_wav_is_passthrough() {
        assert_eq!(classify(&parse(FX_WAV), &cc(true)).unwrap(), SourceAction::Passthrough);
    }

    #[test]
    fn classify_unknown_codec_errors() {
        let err = classify(&parse(FX_UNKNOWN), &cc(false)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported source"), "got: {msg}");
        assert!(msg.contains("ac3"), "msg should name the offending codec_name: {msg}");
    }

    #[test]
    fn classify_no_audio_stream_errors() {
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"png"}],"format":{"format_name":"png_pipe"}}"#;
        let err = classify(&parse(json), &cc(false)).unwrap_err();
        assert!(err.to_string().contains("no audio stream"));
    }
```

- [ ] **Step 6: Build + test**

```powershell
cd F:\repos\ipod-sync
cargo build 2>&1 | Select-Object -Last 5
cargo test transcode:: 2>&1 | Select-Object -Last 15
```

Expected: clean build. All Phase 1 transcode tests still pass + 10 new classify tests pass.

- [ ] **Step 7: Commit**

```powershell
git -C F:\repos\ipod-sync add src\transcode.rs tests\fixtures\sample-ffprobe-mp3.json tests\fixtures\sample-ffprobe-aac.json tests\fixtures\sample-ffprobe-alac.json tests\fixtures\sample-ffprobe-vorbis.json tests\fixtures\sample-ffprobe-opus.json tests\fixtures\sample-ffprobe-wav.json tests\fixtures\sample-ffprobe-unknown.json
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(transcode): SourceAction + classify decision matrix

Adds the per-source codec x container classifier from the Phase 3 spec.
ProbeStream gains codec_name; ProbeFormat gains format_name (both optional,
back-compat with Phase 1 fixtures). classify returns Passthrough for
iPod-native codecs (mp3, aac, alac), Transcode for non-native (flac, vorbis,
opus). PCM (wav/aiff) defaults to Transcode; --passthrough-wav flips it to
Passthrough (Task 3 wires the config). Unknown codecs surface a clear error
naming the offending codec_name.

7 new per-codec ffprobe fixtures + 10 unit tests cover every row of the
decision matrix including --passthrough-wav both ways, unknown-codec error,
and no-audio-stream error.
EOF
)"
```

---

## Task 2: `transcode::passthrough` + `temp_wav_path` + tests (parallel-safe with Task 1, Task 3, Task 4, Task 5)

**Files:**
- Modify: `F:\repos\ipod-sync\src\transcode.rs`

Small surface — could be folded into Task 1 if you're worried about merge churn, but isolating makes the diff easier to review.

- [ ] **Step 1: Add `passthrough`**

```rust
/// Copy `src` to `dst` byte-for-byte. The destination's parent dir is created
/// if missing. Used by `apply_loop::add_one` when classify returns Passthrough.
///
/// Tags are NOT touched here — libgpod handles them via apply_tags, separate
/// from the file body. Cover art for passthrough lives inside the source file's
/// own metadata (e.g. ID3 APIC for MP3, MP4 covr atom for AAC/ALAC); we still
/// extract it via ffmpeg's extract_cover_art for libgpod's thumbnail-write
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
```

- [ ] **Step 2: Add `temp_wav_path`**

```rust
/// Path for the 2-step refalac pipeline's WAV intermediate.
pub fn temp_wav_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push("ipod-sync");
    p.push(format!("ipod-sync-{}.wav", std::process::id()));
    p
}
```

- [ ] **Step 3: Add `temp_passthrough_path`**

Passthrough's dst has the same extension as src (libgpod accepts .mp3 / .m4a / etc.).

```rust
/// Path for a passthrough copy. Same extension as the source so libgpod's
/// internal type-sniffing works without extra hints. Falls back to `.bin`
/// only if the source has no extension at all (shouldn't happen for files
/// the walker accepted, but defensive).
pub fn temp_passthrough_path(src: &Path) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push("ipod-sync");
    let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("bin");
    p.push(format!("ipod-sync-{}.{ext}", std::process::id()));
    p
}
```

- [ ] **Step 4: Tests**

Append to the existing `mod tests`:

```rust
    #[test]
    fn passthrough_copies_bytes_verbatim() {
        let src_dir = std::env::temp_dir().join(format!("ipod-sync-pt-test-{}", std::process::id()));
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
        assert!(temp_passthrough_path(Path::new(r"C:\a.mp3"))
            .extension().and_then(|s| s.to_str()) == Some("mp3"));
        assert!(temp_passthrough_path(Path::new(r"C:\a.m4a"))
            .extension().and_then(|s| s.to_str()) == Some("m4a"));
        assert!(temp_passthrough_path(Path::new(r"C:\noext"))
            .extension().and_then(|s| s.to_str()) == Some("bin"));
    }
```

- [ ] **Step 5: Build + test**

```powershell
cargo build 2>&1 | Select-Object -Last 3
cargo test transcode::tests::passthrough 2>&1 | Select-Object -Last 5
cargo test transcode::tests::temp_passthrough 2>&1 | Select-Object -Last 5
```

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\transcode.rs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(transcode): passthrough + temp_wav_path + temp_passthrough_path

passthrough is std::fs::copy with parent-dir mkdir. temp_wav_path builds
%TEMP%\ipod-sync\ipod-sync-<pid>.wav for the refalac 2-step pipeline's
intermediate. temp_passthrough_path mirrors temp_alac_path but preserves
the source extension so libgpod's type-sniffing sees an .mp3 as .mp3.

Two unit tests cover byte-for-byte fidelity + extension preservation.
EOF
)"
```

---

## Task 3: CLI + Config additions (parallel-safe with Task 1, Task 4, Task 5)

**Files:**
- Modify: `F:\repos\ipod-sync\src\cli.rs`
- Modify: `F:\repos\ipod-sync\src\config.rs`
- Modify: `F:\repos\ipod-sync\src\config_file.rs`

Adds the 4 new flags + `EncoderChoice` enum + corresponding `Config` fields + their persistence.

- [ ] **Step 1: `EncoderChoice` enum in `cli.rs`**

```rust
/// Encoder choice for the transcode pipeline. Passthrough sources never see
/// this (no encoding happens). See docs/superpowers/specs/2026-05-23-phase-3-addendum.md
/// Change 1 for why ffmpeg is the default (was: auto in the original spec).
//
// FUTURE: per-format encoder selection. If a future user wants per-source-codec
// encoder choice (e.g. flac -> refalac, opus -> ffmpeg), this enum stays as-is;
// add a `pub struct EncoderConfig { default: EncoderChoice, per_format: HashMap<String, EncoderChoice> }`
// and have apply_loop resolve `cfg.for_source(&probe.codec_name)` instead of
// passing the global `cfg.encoder`. Everything below this layer is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum EncoderChoice {
    Ffmpeg,
    Refalac,
}

impl EncoderChoice {
    pub fn as_str(&self) -> &'static str {
        match self { EncoderChoice::Ffmpeg => "ffmpeg", EncoderChoice::Refalac => "refalac" }
    }
}
```

- [ ] **Step 2: Add the 4 new fields to `Cli`**

```rust
    /// Encoder for transcoded tracks (non-passthrough). Default: ffmpeg.
    /// Passthrough source codecs (mp3, aac, alac) are unaffected.
    #[arg(long, value_enum, default_value_t = EncoderChoice::Ffmpeg)]
    pub encoder: EncoderChoice,

    /// Path to refalac64.exe. Defaults to "refalac64" (PATH lookup or vendored
    /// copy alongside the binary). Only consulted when --encoder refalac.
    #[arg(long)]
    pub refalac_path: Option<PathBuf>,

    /// Copy WAV/AIFF (PCM) sources bit-perfect to the iPod instead of
    /// transcoding to ALAC. Default: transcode (saves space).
    #[arg(long)]
    pub passthrough_wav: bool,

    /// Treat every Add/Modify track as "must re-encode" regardless of the
    /// manifest's stored encoder. Useful after an ffmpeg/refalac upgrade or
    /// to switch encoders for an existing library.
    #[arg(long)]
    pub force_reencode: bool,
```

- [ ] **Step 3: Extend the CLI tests**

In the existing `mod tests`:

```rust
    #[test]
    fn parses_default_encoder_is_ffmpeg() {
        let cli = Cli::try_parse_from(["ipod-sync"]).unwrap();
        assert_eq!(cli.encoder, EncoderChoice::Ffmpeg);
        assert!(!cli.passthrough_wav);
        assert!(!cli.force_reencode);
        assert!(cli.refalac_path.is_none());
    }

    #[test]
    fn parses_explicit_encoder_refalac() {
        let cli = Cli::try_parse_from(["ipod-sync", "--encoder", "refalac"]).unwrap();
        assert_eq!(cli.encoder, EncoderChoice::Refalac);
    }

    #[test]
    fn parses_explicit_encoder_ffmpeg() {
        let cli = Cli::try_parse_from(["ipod-sync", "--encoder", "ffmpeg"]).unwrap();
        assert_eq!(cli.encoder, EncoderChoice::Ffmpeg);
    }

    #[test]
    fn rejects_unknown_encoder() {
        assert!(Cli::try_parse_from(["ipod-sync", "--encoder", "auto"]).is_err(),
            "spec's 'auto' mode was dropped per the addendum");
        assert!(Cli::try_parse_from(["ipod-sync", "--encoder", "lame"]).is_err());
    }

    #[test]
    fn parses_refalac_path_passthrough_wav_force_reencode() {
        let cli = Cli::try_parse_from([
            "ipod-sync",
            "--encoder", "refalac",
            "--refalac-path", r"C:\bin\refalac64.exe",
            "--passthrough-wav",
            "--force-reencode",
        ]).unwrap();
        assert_eq!(cli.encoder, EncoderChoice::Refalac);
        assert_eq!(cli.refalac_path.as_deref().and_then(|p| p.to_str()),
                   Some(r"C:\bin\refalac64.exe"));
        assert!(cli.passthrough_wav);
        assert!(cli.force_reencode);
    }
```

Update the existing `parses_no_args_with_defaults` test to assert the new defaults too:
```rust
    assert_eq!(cli.encoder, EncoderChoice::Ffmpeg);
    assert!(!cli.passthrough_wav);
    assert!(!cli.force_reencode);
    assert!(cli.refalac_path.is_none());
```

- [ ] **Step 4: Extend `PersistedConfig` in `config_file.rs`**

Add 3 fields (NOT `force_reencode` — it's a one-shot flag, persisting `false` adds noise without value):

```rust
pub struct PersistedConfig {
    // ... existing ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<crate::cli::EncoderChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passthrough_wav: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refalac_path: Option<PathBuf>,
}
```

(`EncoderChoice` derived `Serialize + Deserialize` in Step 1, so this works.)

- [ ] **Step 5: Extend `Config` in `config.rs`**

```rust
pub struct Config {
    // ... existing ...
    pub encoder: crate::cli::EncoderChoice,
    pub refalac_path: PathBuf,
    pub passthrough_wav: bool,
    pub force_reencode: bool,
}
```

Update `resolve_with` to merge them:

```rust
    let encoder = if cli_encoder_was_provided(&cli) {
        cli.encoder
    } else {
        persisted.as_ref().and_then(|p| p.encoder).unwrap_or(crate::cli::EncoderChoice::Ffmpeg)
    };

    let refalac_path = cli
        .refalac_path
        .or_else(|| persisted.as_ref().and_then(|p| p.refalac_path.clone()))
        .unwrap_or_else(|| PathBuf::from("refalac64"));

    let passthrough_wav = cli.passthrough_wav
        || persisted.as_ref().and_then(|p| p.passthrough_wav).unwrap_or(false);

    // force_reencode is CLI-only (no persisted layer).
    let force_reencode = cli.force_reencode;
```

NOTE on `cli_encoder_was_provided`: clap's `default_value_t` makes the CLI field always populated, so the persisted layer never wins. There are two acceptable resolutions; pick (a):

**(a) [chosen] Recommended:** drop `default_value_t = EncoderChoice::Ffmpeg` from the clap attr and change `pub encoder: EncoderChoice` to `pub encoder: Option<EncoderChoice>`. The CLI is `Some` only when the user passes the flag; `None` falls through to persisted → default. This is the standard clap pattern for "CLI overrides persisted but persisted overrides default."

If you take (a), simplify the resolve block:
```rust
    let encoder = cli.encoder
        .or_else(|| persisted.as_ref().and_then(|p| p.encoder))
        .unwrap_or(crate::cli::EncoderChoice::Ffmpeg);
```

And the CLI tests change: `assert_eq!(cli.encoder, None);` for no-flag, `Some(EncoderChoice::Refalac)` when set. Adjust the Step 3 tests accordingly.

**(b) Alternative:** keep `default_value_t` and accept that CLI always shadows persisted for `encoder`. Documented gotcha. Less consistent with the other "CLI > env > persisted > default" fields. Don't pick this.

- [ ] **Step 6: Update `Config::to_persisted`**

```rust
    pub fn to_persisted(&self) -> PersistedConfig {
        PersistedConfig {
            source: Some(self.source.clone()),
            ipod: self.ipod.clone(),
            ffmpeg: Some(self.ffmpeg.clone()),
            no_delete: Some(self.no_delete),
            no_tui: Some(!self.use_tui),
            encoder: Some(self.encoder),                  // NEW
            passthrough_wav: Some(self.passthrough_wav),  // NEW
            refalac_path: Some(self.refalac_path.clone()),// NEW
        }
    }
```

- [ ] **Step 7: Update config.rs tests**

The existing `other_defaults_apply_when_source_is_present` should assert the new defaults:

```rust
    assert_eq!(config.encoder, crate::cli::EncoderChoice::Ffmpeg);
    assert_eq!(config.refalac_path, PathBuf::from("refalac64"));
    assert!(!config.passthrough_wav);
    assert!(!config.force_reencode);
```

Add three new tests:

```rust
    #[test]
    fn cli_encoder_wins_over_persisted_encoder() {
        let cli = Cli::try_parse_from([
            "ipod-sync", "--source", r"D:\m", "--encoder", "refalac",
        ]).unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"X:\x")),
            encoder: Some(crate::cli::EncoderChoice::Ffmpeg),
            ..Default::default()
        };
        let cfg = resolve_with(cli, None, Some(persisted), PathBuf::from("dummy.json")).unwrap();
        assert_eq!(cfg.encoder, crate::cli::EncoderChoice::Refalac);
    }

    #[test]
    fn persisted_encoder_used_when_no_cli_flag() {
        let cli = Cli::try_parse_from(["ipod-sync", "--source", r"D:\m"]).unwrap();
        let persisted = PersistedConfig {
            source: Some(PathBuf::from(r"X:\x")),
            encoder: Some(crate::cli::EncoderChoice::Refalac),
            ..Default::default()
        };
        let cfg = resolve_with(cli, None, Some(persisted), PathBuf::from("dummy.json")).unwrap();
        assert_eq!(cfg.encoder, crate::cli::EncoderChoice::Refalac);
    }

    #[test]
    fn force_reencode_is_cli_only() {
        let cli = Cli::try_parse_from(["ipod-sync", "--source", r"D:\m", "--force-reencode"]).unwrap();
        let cfg = resolve_with(cli, None, None, PathBuf::from("dummy.json")).unwrap();
        assert!(cfg.force_reencode);

        // Same persisted config, no CLI flag: force_reencode stays false.
        let cli_no_flag = Cli::try_parse_from(["ipod-sync", "--source", r"D:\m"]).unwrap();
        let cfg2 = resolve_with(cli_no_flag, None, None, PathBuf::from("dummy.json")).unwrap();
        assert!(!cfg2.force_reencode);
    }
```

- [ ] **Step 8: Build + test**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test cli:: 2>&1 | Select-Object -Last 10
cargo test config:: 2>&1 | Select-Object -Last 10
cargo test config_file:: 2>&1 | Select-Object -Last 10
```

Expected: clean build, all existing tests + ~7 new tests pass.

- [ ] **Step 9: Commit**

```powershell
git -C F:\repos\ipod-sync add src\cli.rs src\config.rs src\config_file.rs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(cli, config): --encoder, --refalac-path, --passthrough-wav, --force-reencode

EncoderChoice enum (clap::ValueEnum + serde) with Ffmpeg + Refalac variants
(no Auto — see Phase 3 addendum Change 1 for why). cli.encoder is
Option<EncoderChoice> so persisted-config layer can win when no flag.

Config grows encoder, refalac_path, passthrough_wav, force_reencode.
PersistedConfig grows encoder, passthrough_wav, refalac_path
(force_reencode intentionally CLI-only — one-shot, persisting false is noise).

resolve_with merges CLI > persisted > default for each new field.
Tests cover: defaults, explicit flag values, --encoder rejection of 'auto',
CLI-wins-over-persisted, persisted-wins-with-no-CLI, force_reencode-is-CLI-only.
EOF
)"
```

---

## Task 4: `ManifestEntry` schema additions + encoder-mismatch in diff (parallel-safe with Task 1, Task 2, Task 3, Task 5)

**Files:**
- Modify: `F:\repos\ipod-sync\src\manifest.rs`

Adds `encoder`, `encoder_version`, `source_format` fields with serde defaults that make Phase 2 / Phase 3.x manifests deserialize unchanged. Adds an encoder-mismatch branch to `diff` (gated on `--force-reencode` and carving out `unknown` + `passthrough`).

- [ ] **Step 1: Add fields to `ManifestEntry`**

```rust
pub struct ManifestEntry {
    // ... existing fields up through audio_fingerprint ...

    /// "refalac" | "ffmpeg" | "passthrough" | "unknown".
    /// Phase 2 manifests deserialize as "unknown" (no encoder field on disk).
    #[serde(default = "default_encoder")]
    pub encoder: String,

    /// e.g. "refalac 1.85" or "ffmpeg n7.0". Empty string for passthrough or unknown.
    #[serde(default)]
    pub encoder_version: String,

    /// ffprobe codec_name at sync time. "flac" | "mp3" | "aac" | "alac" |
    /// "vorbis" | "opus" | "pcm_s16le" | etc. Phase 2 manifests default to
    /// "flac" since that was Phase 2's only supported source. See
    /// docs/superpowers/specs/2026-05-23-phase-3-addendum.md Change 3.
    #[serde(default = "default_source_format")]
    pub source_format: String,
}

fn default_encoder() -> String { "unknown".to_string() }
fn default_source_format() -> String { "flac".to_string() }
```

(Keep `default_source_known` next to these.)

- [ ] **Step 2: Update `sample_entry` helper + existing tests**

The existing `sample_entry` helper builds entries with implicit Phase 2/3.x fields. To keep most tests minimal, give it the new fields default values:

```rust
    fn sample_entry(path: &str, fp: &str, size: u64) -> ManifestEntry {
        ManifestEntry {
            source_path: PathBuf::from(path),
            source_mtime: 1700000000,
            source_size: size,
            source_fingerprint: fp.to_string(),
            ipod_dbid: 12345678901234,
            ipod_relpath: r"iPod_Control\Music\F12\KLMN.m4a".to_string(),
            source_known: true,
            audio_fingerprint: String::new(),
            encoder: "ffmpeg".to_string(),       // Phase 3 era — what Phase 2 will UPGRADE to
            encoder_version: "ffmpeg n7.0".to_string(),
            source_format: "flac".to_string(),
        }
    }
```

(The existing `manifest_entry_supports_optional_audio_fingerprint` test will need to be extended with assertions for the new defaults; see Step 5.)

- [ ] **Step 3: Extend the diff signature with encoder-mismatch awareness**

Current signature:
```rust
pub fn diff(
    manifest: &Manifest,
    sources: &[SourceEntry],
    mut compute_fingerprint: impl FnMut(&Path) -> Result<String>,
    mut compute_audio_fingerprint: impl FnMut(&Path) -> Result<String>,
) -> Result<Vec<Action>>
```

Extend with two new params:
```rust
pub fn diff(
    manifest: &Manifest,
    sources: &[SourceEntry],
    mut compute_fingerprint: impl FnMut(&Path) -> Result<String>,
    mut compute_audio_fingerprint: impl FnMut(&Path) -> Result<String>,
    target_encoder: &str,        // NEW — e.g. "ffmpeg" or "refalac"
    force_reencode: bool,        // NEW
) -> Result<Vec<Action>>
```

Inside, where the fast-path emits `Action::Unchanged((*entry).clone())`, branch first on encoder-mismatch:

```rust
                if stat_matches {
                    // FAST PATH — no fingerprint read.
                    // Encoder-mismatch check: if the stored encoder differs
                    // from what we'd use now, the file body on iPod is the
                    // wrong encoder's output and needs re-encoding even
                    // though the source is unchanged.
                    if is_encoder_mismatch(entry, target_encoder, force_reencode) {
                        // Synthesize a SourceEntry from the manifest data so
                        // the existing Modify arm can re-add it. We have the
                        // matching `src` already in this loop iteration:
                        actions.push(Action::Modify(src.clone(), (*entry).clone()));
                    } else {
                        actions.push(Action::Unchanged((*entry).clone()));
                    }
                } else {
                    // slow path: existing logic unchanged, but the Unchanged-
                    // after-content-match branch ALSO needs the same check:
                    let fp = compute_fingerprint(&src.path)?;
                    let content_unchanged = fp == entry.source_fingerprint
                        && src.size == entry.source_size;
                    if content_unchanged {
                        if is_encoder_mismatch(entry, target_encoder, force_reencode) {
                            actions.push(Action::Modify(src.clone(), (*entry).clone()));
                        } else {
                            actions.push(Action::Unchanged((*entry).clone()));
                        }
                    } else if !entry.audio_fingerprint.is_empty() {
                        // existing MetadataOnly / Modify branch unchanged.
                        // NOTE: a MetadataOnly action carries no re-encode, so
                        // it's NOT subject to encoder-mismatch here — the audio
                        // bytes on iPod stay as-is, only tags/art update.
                        let audio_fp = compute_audio_fingerprint(&src.path)?;
                        if audio_fp == entry.audio_fingerprint {
                            actions.push(Action::MetadataOnly { source: src.clone(), entry: (*entry).clone() });
                        } else {
                            actions.push(Action::Modify(src.clone(), (*entry).clone()));
                        }
                    } else {
                        actions.push(Action::Modify(src.clone(), (*entry).clone()));
                    }
                }
```

- [ ] **Step 4: `is_encoder_mismatch` helper**

```rust
/// True iff this manifest entry's stored encoder differs from the target
/// encoder in a way that means we should re-encode.
///
/// Carve-outs:
/// - force = true: always returns true. User asked for it.
/// - encoder == "unknown": Phase 2 manifest (no encoder field). Don't
///   trigger spurious re-encodes on first Phase 3 run — let the entry
///   get populated naturally on its next normal Modify.
/// - encoder == "passthrough": there's no encoder for a copied file; the
///   on-iPod bytes are the source bytes regardless of what's set globally.
fn is_encoder_mismatch(entry: &ManifestEntry, target: &str, force: bool) -> bool {
    if force { return true; }
    if entry.encoder == "unknown" { return false; }
    if entry.encoder == "passthrough" { return false; }
    entry.encoder != target
}
```

- [ ] **Step 5: Update every caller of `diff`**

The only production caller is `apply_loop::run`; Task 6 updates that. For the tests in this file, update each call-site to pass `"ffmpeg", false`:

```rust
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(), "ffmpeg", false).unwrap();
```

All ~10 existing diff tests need this update.

- [ ] **Step 6: New tests for encoder-mismatch**

```rust
    #[test]
    fn diff_encoder_mismatch_forces_modify_on_fast_path() {
        // Stat matches → fast path. Stored encoder = ffmpeg; target = refalac.
        // Result: Modify, even though content is unchanged.
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.encoder = "ffmpeg".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(),
                           "refalac", false).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)),
            "encoder mismatch on otherwise-unchanged entry must trigger Modify; got {:?}",
            actions[0]);
    }

    #[test]
    fn diff_encoder_match_stays_unchanged() {
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.encoder = "ffmpeg".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(),
                           "ffmpeg", false).unwrap();
        assert!(matches!(actions[0], Action::Unchanged(_)));
    }

    #[test]
    fn diff_force_reencode_promotes_to_modify_regardless_of_encoder_match() {
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.encoder = "ffmpeg".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(),
                           "ffmpeg", true).unwrap();
        assert!(matches!(actions[0], Action::Modify(_, _)),
            "--force-reencode must promote everything to Modify");
    }

    #[test]
    fn diff_unknown_encoder_does_not_trigger_modify() {
        // Phase 2 manifests have encoder="unknown" (from default_encoder).
        // Switching to Phase 3 must NOT trigger a thundering re-encode.
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.encoder = "unknown".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(),
                           "refalac", false).unwrap();
        assert!(matches!(actions[0], Action::Unchanged(_)),
            "unknown-encoder entries are immune to encoder-mismatch (Phase 2 back-compat)");
    }

    #[test]
    fn diff_passthrough_encoder_does_not_trigger_modify() {
        // Passthrough files have no encoder; switching --encoder is irrelevant.
        let mut entry = sample_entry(r"C:\a.mp3", "blake3:aa", 100);
        entry.encoder = "passthrough".to_string();
        entry.source_format = "mp3".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let sources = vec![sample_source(r"C:\a.mp3", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(),
                           "refalac", false).unwrap();
        assert!(matches!(actions[0], Action::Unchanged(_)),
            "passthrough entries are immune to encoder-mismatch");
    }

    #[test]
    fn diff_force_reencode_promotes_unknown_too() {
        // --force-reencode wins over the unknown carve-out.
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.encoder = "unknown".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources, never_called(), never_called_audio(),
                           "ffmpeg", true).unwrap();
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_encoder_mismatch_slow_path_after_touch() {
        // Source mtime touched but content unchanged → slow-path content match.
        // Encoder mismatch → Modify (not Unchanged).
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.encoder = "ffmpeg".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let mut src = sample_source(r"C:\a.flac", "blake3:aa", 100);
        src.mtime = 1700099999; // touched
        let actions = diff(&manifest, &[src], returns("blake3:aa"), never_called_audio(),
                           "refalac", false).unwrap();
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }
```

- [ ] **Step 7: Extend the existing Phase 2 back-compat test**

The existing `manifest_entry_supports_optional_audio_fingerprint` test reads a Phase 2 JSON entry (no `audio_fingerprint`). Extend it to assert the new fields also default correctly:

```rust
    #[test]
    fn manifest_entry_supports_phase_2_and_phase_3_shapes() {
        // Phase 2: no audio_fingerprint, no encoder, no encoder_version, no source_format.
        let phase2 = r#"{
            "source_path": "C:\\a.flac",
            "source_mtime": 1700000000,
            "source_size": 100,
            "source_fingerprint": "blake3:aa",
            "ipod_dbid": 1234,
            "ipod_relpath": "iPod_Control\\Music\\F01\\AAAA.m4a",
            "source_known": true
        }"#;
        let entry: ManifestEntry = serde_json::from_str(phase2).unwrap();
        assert_eq!(entry.audio_fingerprint, "");
        assert_eq!(entry.encoder, "unknown",
            "Phase 2 entries must default to 'unknown' encoder so no spurious re-encode");
        assert_eq!(entry.encoder_version, "");
        assert_eq!(entry.source_format, "flac",
            "Phase 2 sources were always FLAC; default reflects history");

        // Phase 3.x: gained audio_fingerprint.
        let phase3x = r#"{
            "source_path": "C:\\b.flac",
            "source_mtime": 1700000000,
            "source_size": 200,
            "source_fingerprint": "blake3:bb",
            "ipod_dbid": 5678,
            "ipod_relpath": "iPod_Control\\Music\\F02\\BBBB.m4a",
            "source_known": true,
            "audio_fingerprint": "blake3-audio:cc"
        }"#;
        let entry: ManifestEntry = serde_json::from_str(phase3x).unwrap();
        assert_eq!(entry.audio_fingerprint, "blake3-audio:cc");
        assert_eq!(entry.encoder, "unknown");
        assert_eq!(entry.source_format, "flac");

        // Phase 3 full shape:
        let phase3 = r#"{
            "source_path": "C:\\c.mp3",
            "source_mtime": 1700000000,
            "source_size": 300,
            "source_fingerprint": "blake3:cc",
            "ipod_dbid": 9999,
            "ipod_relpath": "iPod_Control\\Music\\F03\\CCCC.mp3",
            "source_known": true,
            "audio_fingerprint": "",
            "encoder": "passthrough",
            "encoder_version": "",
            "source_format": "mp3"
        }"#;
        let entry: ManifestEntry = serde_json::from_str(phase3).unwrap();
        assert_eq!(entry.encoder, "passthrough");
        assert_eq!(entry.source_format, "mp3");
    }
```

(The original `manifest_entry_supports_optional_audio_fingerprint` test name can stay; this is a rename + expansion.)

- [ ] **Step 8: Build + test**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test manifest:: 2>&1 | Select-Object -Last 20
```

Expected: clean build. All existing manifest tests pass after the signature update + 7 new tests pass.

The `roundtrip_known_fixture` test reads `tests/fixtures/sample-manifest.json`. If that fixture is Phase 2-shaped (no encoder fields), the test still passes thanks to serde defaults. If it's Phase 3.x-shaped (audio_fingerprint present), same. Don't modify the fixture — Phase 2 back-compat IS the test.

- [ ] **Step 9: Commit**

```powershell
git -C F:\repos\ipod-sync add src\manifest.rs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(manifest): encoder + encoder_version + source_format fields; encoder-mismatch in diff

ManifestEntry gains three fields, all with serde defaults that make Phase 2
manifests deserialize cleanly: encoder defaults to "unknown" (so the
encoder-mismatch heuristic skips it), source_format defaults to "flac" (Phase
2's only supported source).

diff() gains target_encoder + force_reencode parameters. is_encoder_mismatch
helper compares each entry's encoder against target_encoder with carve-outs
for "unknown" (Phase 2 back-compat) and "passthrough" (no encoder to mismatch).
When mismatch on an otherwise-Unchanged entry, emits Modify instead.

7 new tests cover: fast-path mismatch -> Modify, match -> Unchanged,
--force-reencode -> Modify regardless, unknown-encoder carve-out, passthrough
carve-out, force+unknown still Modify, slow-path mismatch after mtime touch.
Phase 2/3.x/3 JSON shapes all deserialize via the existing back-compat test.
EOF
)"
```

---

## Task 5: Refalac vendoring + `build.rs` extension + `verify_refalac` + `transcode_via_refalac` skeleton (parallel-safe with Task 1, Task 3, Task 4)

**Files:**
- New: `F:\repos\ipod-sync\vendor\refalac\refalac64.exe` (binary; see Step 1 for sourcing)
- New: `F:\repos\ipod-sync\vendor\refalac\libFLAC.dll` (binary; bundled with qaac)
- Modify: `F:\repos\ipod-sync\build.rs`
- Modify: `F:\repos\ipod-sync\src\transcode.rs`
- Modify: `F:\repos\ipod-sync\src\preflight.rs`

Vendors refalac, wires `build.rs` to copy it alongside the exe, adds `verify_refalac` preflight gate (conditional on encoder=refalac), and implements `transcode_via_refalac` itself.

- [ ] **Step 1: Obtain refalac64.exe + libFLAC.dll**

This is a USER action; the implementer subagent cannot download binaries. The implementer should STOP at this step and request the binaries be placed manually, OR proceed with the placeholder + skip the live encode tests.

**Instructions for the user:**

1. Download the latest qaac release from https://github.com/nu774/qaac/releases (look for `qaac_X.YY.zip`).
2. Extract the zip. Inside, find `x64\refalac64.exe` and `x64\libFLAC.dll`.
3. Create `F:\repos\ipod-sync\vendor\refalac\` and copy both files into it.
4. Verify in PowerShell:
   ```powershell
   F:\repos\ipod-sync\vendor\refalac\refalac64.exe --check
   # Expected: prints version info (e.g. "refalac 1.85, CoreAudioToolbox <version>")
   ```
   If `--check` errors with "missing libFLAC.dll", `libFLAC.dll` isn't in the same dir.

**Fallback if vendoring is impractical:** create the `vendor/refalac/` directory but leave it empty. `build.rs` (Step 2) gracefully skips copying when the dir is empty. Users who want refalac install qaac themselves and pass `--refalac-path C:\bin\refalac64.exe`. Document this in the LEARNINGS gate-result entry.

(No commit at this step — binaries are gitignored or live-copied; see Step 3 for the .gitignore decision.)

- [ ] **Step 2: Extend `build.rs` to copy `vendor/refalac/*`**

Add after the existing libgpod DLL copy block (around line 100 of `build.rs`):

```rust
    // Phase 3: vendor refalac64.exe + libFLAC.dll alongside the existing
    // libgpod runtime DLLs. Skipped silently if vendor/refalac/ doesn't exist
    // (e.g. user opted not to vendor; they'll pass --refalac-path instead).
    let refalac_dir = manifest_dir.join("vendor").join("refalac");
    if refalac_dir.exists() {
        let entries = std::fs::read_dir(&refalac_dir)
            .unwrap_or_else(|e| panic!("read vendor/refalac dir {}: {}", refalac_dir.display(), e));
        for entry in entries {
            let entry = entry.expect("read vendor/refalac entry");
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str());
            if matches!(ext, Some("exe") | Some("dll")) {
                let dest = target_dir.join(path.file_name().unwrap());
                std::fs::copy(&path, &dest).unwrap_or_else(|e| {
                    panic!("copy refalac asset {} -> {}: {}", path.display(), dest.display(), e)
                });
            }
        }
        println!("cargo:rerun-if-changed={}", refalac_dir.display());
    }
```

- [ ] **Step 3: `.gitignore` (or not) for vendored binaries**

Decision call. Two options, pick one:

**(a) Recommended: commit the binaries.** Adds ~3 MB to the repo but makes `cargo build` self-sufficient. qaac is BSD-2 licensed — re-distribution is allowed. Add a `vendor/refalac/LICENSE.txt` derived from the qaac LICENSE file.

**(b) Gitignore them.** Smaller repo, but every clone needs the user-action download dance. Add to `.gitignore`:
```
/vendor/refalac/refalac64.exe
/vendor/refalac/libFLAC.dll
```
Document the download dance in a `vendor/refalac/README.md`.

If unsure, prefer (a). The build is more reliable when external binary dependencies are vendored.

- [ ] **Step 4: Add `verify_refalac` to `transcode.rs`**

```rust
/// Probe that refalac64 is reachable and responds to `--check`. Called by
/// preflight when config.encoder == EncoderChoice::Refalac.
pub fn verify_refalac(refalac_path: &Path) -> Result<()> {
    let out = Command::new(refalac_path)
        .arg("--check")
        .output()
        .map_err(|e| anyhow!(
            "failed to spawn refalac64 at {}: {e}\n\
             Install qaac (https://github.com/nu774/qaac/releases) and either\n\
             put refalac64.exe on PATH, vendor it under vendor/refalac/, or\n\
             pass --refalac-path <path>.",
            refalac_path.display()
        ))?;
    if !out.status.success() {
        return Err(anyhow!(
            "refalac64 --check failed (exit {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Parse the refalac --check output for a version string. Best-effort.
/// Stored in ManifestEntry.encoder_version. Falls back to "refalac unknown"
/// if parsing fails — we'd rather record a record than panic.
pub fn refalac_version(refalac_path: &Path) -> String {
    let out = match Command::new(refalac_path).arg("--check").output() {
        Ok(o) => o,
        Err(_) => return "refalac unknown".to_string(),
    };
    // refalac --check prints version on the first line of stdout, e.g.
    //   "refalac 1.85, CoreAudioToolbox 7.10.9.0"
    // Some builds put it on stderr instead. Try both, take the first non-empty.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    for src in [stdout.trim(), stderr.trim()] {
        if let Some(line) = src.lines().next() {
            if line.starts_with("refalac") {
                return line.split(',').next().unwrap_or(line).trim().to_string();
            }
        }
    }
    "refalac unknown".to_string()
}

/// Parse ffmpeg -version output for the build tag. Stored in ManifestEntry
/// .encoder_version. e.g. "ffmpeg n7.0".
pub fn ffmpeg_version(ffmpeg_path: &Path) -> String {
    let out = match Command::new(ffmpeg_path).arg("-version").output() {
        Ok(o) => o,
        Err(_) => return "ffmpeg unknown".to_string(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    if let Some(line) = stdout.lines().next() {
        // Typical: "ffmpeg version n7.0 Copyright (c) ..." or "ffmpeg version N-..."
        // We want "ffmpeg <tag>".
        let mut parts = line.split_whitespace();
        if let (Some(name), Some(_), Some(tag)) = (parts.next(), parts.next(), parts.next()) {
            return format!("{name} {tag}");
        }
    }
    "ffmpeg unknown".to_string()
}
```

- [ ] **Step 5: Add `ResolvedEncoder` + `resolve_encoder` to `transcode.rs`**

```rust
/// Snapshot of the encoder choice resolved at preflight: the name + version
/// strings get written into every new ManifestEntry produced this run.
#[derive(Debug, Clone)]
pub struct ResolvedEncoder {
    /// "ffmpeg" or "refalac" — written to ManifestEntry.encoder for transcoded tracks.
    pub name: String,
    /// e.g. "ffmpeg n7.0" or "refalac 1.85" — written to ManifestEntry.encoder_version.
    pub version: String,
    /// Path to refalac64.exe. Only consulted when name == "refalac".
    pub refalac_path: std::path::PathBuf,
    /// Path to ffmpeg.exe. Always populated (refalac path still uses ffmpeg
    /// for the decode-to-WAV step).
    pub ffmpeg_path: std::path::PathBuf,
}

pub fn resolve_encoder(config: &crate::config::Config) -> ResolvedEncoder {
    let ffmpeg_path = config.ffmpeg.clone();
    let refalac_path = config.refalac_path.clone();
    match config.encoder {
        crate::cli::EncoderChoice::Ffmpeg => ResolvedEncoder {
            name: "ffmpeg".to_string(),
            version: ffmpeg_version(&ffmpeg_path),
            refalac_path,
            ffmpeg_path,
        },
        crate::cli::EncoderChoice::Refalac => ResolvedEncoder {
            name: "refalac".to_string(),
            version: refalac_version(&refalac_path),
            refalac_path,
            ffmpeg_path,
        },
    }
}
```

- [ ] **Step 6: Add `transcode_via_refalac`**

```rust
/// 2-step pipeline: ffmpeg decodes `src` to WAV, then refalac encodes WAV →
/// ALAC m4a at `dst`. Optional `art_jpg` is passed to refalac as --artwork.
/// Tags are NOT carried here; libgpod's apply_tags handles them separately,
/// same as for the ffmpeg path.
pub fn transcode_via_refalac(
    src: &Path,
    dst: &Path,
    art_jpg: Option<&Path>,
    encoder: &ResolvedEncoder,
) -> Result<()> {
    let temp_wav = temp_wav_path();
    if let Some(parent) = temp_wav.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Step 1: ffmpeg decode to PCM WAV. -vn drops any embedded art stream
    // (refalac would refuse WAV with extra streams).
    let status = Command::new(&encoder.ffmpeg_path)
        .args(["-loglevel", "error", "-y"])
        .args(["-i"])
        .arg(src)
        .args(["-vn", "-c:a", "pcm_s16le", "-f", "wav"])
        .arg(&temp_wav)
        .status()
        .map_err(|e| anyhow!("spawn ffmpeg for refalac decode: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&temp_wav);
        return Err(anyhow!("ffmpeg decode-to-wav failed (exit {:?})", status.code()));
    }

    // Step 2: refalac encode WAV -> ALAC m4a.
    let mut cmd = Command::new(&encoder.refalac_path);
    cmd.args(["--silent", "-o"]).arg(dst);
    if let Some(art) = art_jpg {
        cmd.arg("--artwork").arg(art);
    }
    cmd.arg(&temp_wav);

    let status = cmd.status()
        .map_err(|e| anyhow!("spawn refalac at {}: {e}", encoder.refalac_path.display()))?;

    // Best-effort cleanup of the WAV regardless of refalac's exit.
    let _ = std::fs::remove_file(&temp_wav);

    if !status.success() {
        return Err(anyhow!("refalac encode failed (exit {:?})", status.code()));
    }
    Ok(())
}
```

- [ ] **Step 7: Add `verify_refalac` to `preflight.rs`**

Find the existing `verify_ffmpeg` function (Phase 3.z). Add an analogous gate:

```rust
/// Verify refalac64.exe is reachable. Only called when config.encoder ==
/// EncoderChoice::Refalac. Uses await_prompt for Retry/Abort on failure.
pub fn verify_refalac(
    config: &Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<()> {
    if config.encoder != crate::cli::EncoderChoice::Refalac {
        return Ok(()); // ffmpeg path doesn't need refalac
    }
    loop {
        match transcode::verify_refalac(&config.refalac_path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let msg = format!(
                    "refalac64 was not reachable at {}:\n  {e}\n\n\
                     Options:\n\
                     [1] Retry (after fixing the install)\n\
                     [2] Abort",
                    config.refalac_path.display()
                );
                let outcome = await_prompt(
                    progress, decision_rx, msg,
                    &["Retry", "Abort"],
                    &[PromptOutcome::Retry, PromptOutcome::Abort],
                )?;
                if outcome != PromptOutcome::Retry {
                    return Err(anyhow!(
                        "refalac64 required (--encoder refalac); aborted by user"
                    ));
                }
            }
        }
    }
}
```

(`use crate::cli;` may be needed at the top of preflight.rs if not already there.)

- [ ] **Step 8: Tests**

Pure-logic tests only — the live refalac probe needs the binary present so it's a gate-time exercise, not a unit test.

```rust
    #[test]
    fn refalac_version_returns_fallback_on_missing_binary() {
        let v = refalac_version(Path::new("definitely-not-a-real-binary-xyz123"));
        assert_eq!(v, "refalac unknown");
    }

    #[test]
    fn ffmpeg_version_returns_fallback_on_missing_binary() {
        let v = ffmpeg_version(Path::new("definitely-not-a-real-binary-xyz123"));
        assert_eq!(v, "ffmpeg unknown");
    }
```

(`resolve_encoder` is exercised by Task 6's apply-path; no unit test here because it borrows `Config` which needs Task 3's fields.)

- [ ] **Step 9: Build + test**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test transcode::tests 2>&1 | Select-Object -Last 10
cargo test preflight:: 2>&1 | Select-Object -Last 5
```

Verify that if `vendor/refalac/` is present, `target/debug/refalac64.exe` and `target/debug/libFLAC.dll` exist after `cargo build`. If absent, build still succeeds (graceful skip).

- [ ] **Step 10: Commit (one or two commits depending on vendoring choice)**

If you chose option (a) commit-the-binaries from Step 3:

```powershell
git -C F:\repos\ipod-sync add build.rs src\transcode.rs src\preflight.rs vendor\refalac\refalac64.exe vendor\refalac\libFLAC.dll vendor\refalac\LICENSE.txt
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(transcode, build): vendor refalac64.exe + verify/encode pipeline

Adds refalac64.exe + libFLAC.dll (qaac project, BSD-2 LICENSE included)
under vendor/refalac/, copied alongside the libgpod runtime DLLs by
build.rs. ~3 MB added to the repo; makes cargo build self-sufficient.

transcode::verify_refalac probes the binary via --check; refalac_version
and ffmpeg_version parse the tool's first-line version for ManifestEntry
.encoder_version. ResolvedEncoder bundles the resolved name + version +
both binary paths into one struct that apply_loop passes around.

transcode_via_refalac implements the 2-step pipeline from the spec: ffmpeg
decodes to PCM WAV (with -vn to drop attached art), refalac encodes WAV to
ALAC m4a with optional --artwork. WAV intermediate is cleaned up regardless
of refalac's exit.

preflight::verify_refalac gates startup when encoder=refalac with a
Retry/Abort TUI prompt (Phase 3.z await_prompt). No-op when encoder=ffmpeg
so users on the default path don't probe a binary they don't need.
EOF
)"
```

If you chose option (b) gitignore-the-binaries: same commit minus the two binary files plus the .gitignore change + README, with a note in the commit body explaining the install dance.

---

## Task 6: Wire `classify` + branch in `add_one` + encoder resolution in `run`

**Files:**
- Modify: `F:\repos\ipod-sync\src\apply_loop.rs`

This task is **sequential after Tasks 1, 2, 3, 4, 5** — it ties them together. Cannot parallelize.

- [ ] **Step 1: Resolve the encoder once at preflight + call verify_refalac**

In `apply_loop::run`, immediately after the existing `verify_ffmpeg` line:

```rust
    preflight::verify_ffmpeg(progress, decision_rx)?;
    preflight::verify_refalac(config, progress, decision_rx)?;  // NEW (no-op when encoder=ffmpeg)
    let mount = preflight::resolve_ipod_mount(config, progress, decision_rx)?;
    let sources = preflight::walk_source(config, progress, decision_rx)?;

    // Resolve the encoder once for this run; downstream `add_one` calls pass it.
    let resolved_encoder = transcode::resolve_encoder(config);
    progress.log(format!(
        "Encoder: {} ({})",
        resolved_encoder.name, resolved_encoder.version
    ));
```

- [ ] **Step 2: Pass target encoder + force_reencode into diff**

Update the existing `manifest::diff` call:

```rust
    let actions = manifest::diff(
        &manifest,
        &sources,
        source::fingerprint,
        source::audio_fingerprint,
        &resolved_encoder.name,
        config.force_reencode,
    )?;
```

- [ ] **Step 3: Refactor `add_one` to branch on classify result**

Current signature returns `(TrackHandle, String, String)` (handle, file fingerprint, audio fingerprint). Add 3 more strings: `encoder`, `encoder_version`, `source_format`.

```rust
pub(crate) struct AddedTrack {
    pub handle: TrackHandle,
    pub fingerprint: String,
    pub audio_fingerprint: String,
    pub encoder: String,
    pub encoder_version: String,
    pub source_format: String,
}

pub(crate) fn add_one(
    db: &OwnedDb,
    src: &SourceEntry,
    config: &Config,
    encoder: &transcode::ResolvedEncoder,
) -> Result<AddedTrack> {
    let probe = transcode::probe(&src.path)
        .with_context(|| format!("probe {}", src.path.display()))?;
    let tags = tags_from_probe(&probe);

    // Classify before deciding pipeline.
    let action = transcode::classify(
        &probe,
        &transcode::ClassifyConfig { passthrough_wav: config.passthrough_wav },
    )
        .with_context(|| format!("classify {}", src.path.display()))?;

    // Source format = ffprobe audio-stream codec_name. We just successfully
    // classified, so the stream + codec_name are present.
    let source_format = probe.streams.iter()
        .find(|s| s.codec_type == "audio")
        .and_then(|s| s.codec_name.clone())
        .unwrap_or_else(|| "unknown".to_string());

    // Extract embedded art ahead of time — both pipelines may need it.
    let art_bytes: Option<Vec<u8>> = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        transcode::extract_cover_art(&src.path, &art_path)?;
        let bytes = std::fs::read(&art_path)?;
        let _ = std::fs::remove_file(&art_path);
        Some(bytes)
    } else {
        None
    };

    let (temp, encoder_name, encoder_version) = match action {
        transcode::SourceAction::Passthrough => {
            let dst = transcode::temp_passthrough_path(&src.path);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            transcode::passthrough(&src.path, &dst)
                .with_context(|| format!("passthrough {}", src.path.display()))?;
            (dst, "passthrough".to_string(), String::new())
        }
        transcode::SourceAction::Transcode => {
            let dst = transcode::temp_alac_path();
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            match encoder.name.as_str() {
                "ffmpeg" => {
                    transcode::transcode_to_alac(&src.path, &dst)
                        .with_context(|| format!("transcode_to_alac {}", src.path.display()))?;
                }
                "refalac" => {
                    // Refalac needs the art on disk (as --artwork <jpg>). Write
                    // the bytes to a temp file just for this call.
                    let art_temp = if art_bytes.is_some() {
                        let p = transcode::temp_art_path();
                        std::fs::write(&p, art_bytes.as_ref().unwrap())?;
                        Some(p)
                    } else {
                        None
                    };
                    transcode::transcode_via_refalac(
                        &src.path,
                        &dst,
                        art_temp.as_deref(),
                        encoder,
                    )
                        .with_context(|| format!("transcode_via_refalac {}", src.path.display()))?;
                    if let Some(p) = art_temp { let _ = std::fs::remove_file(&p); }
                }
                other => return Err(anyhow!("unknown encoder name {other:?} in ResolvedEncoder")),
            }
            (dst, encoder.name.clone(), encoder.version.clone())
        }
    };

    let handle = db.add_track_with_file(&temp, &tags, art_bytes.as_deref())
        .with_context(|| format!("add_track_with_file for {}", src.path.display()))?;

    let _ = std::fs::remove_file(&temp);

    let fingerprint = source::fingerprint(&src.path)
        .with_context(|| format!("fingerprint {}", src.path.display()))?;
    let audio_fp = source::audio_fingerprint(&src.path)
        .with_context(|| format!("audio_fingerprint {}", src.path.display()))?;

    Ok(AddedTrack {
        handle,
        fingerprint,
        audio_fingerprint: audio_fp,
        encoder: encoder_name,
        encoder_version,
        source_format,
    })
}
```

- [ ] **Step 4: Update `entry_from` signature**

```rust
pub(crate) fn entry_from(src: &SourceEntry, added: &AddedTrack) -> ManifestEntry {
    ManifestEntry {
        source_path: src.path.clone(),
        source_mtime: src.mtime,
        source_size: src.size,
        source_fingerprint: added.fingerprint.clone(),
        ipod_dbid: added.handle.dbid,
        ipod_relpath: added.handle.ipod_relpath.clone(),
        source_known: true,
        audio_fingerprint: added.audio_fingerprint.clone(),
        encoder: added.encoder.clone(),
        encoder_version: added.encoder_version.clone(),
        source_format: added.source_format.clone(),
    }
}
```

- [ ] **Step 5: Update every call-site of `add_one` + `entry_from` in `run`**

In each of the four occurrences (Action::Add, Action::Modify second-half, etc.), replace:

```rust
        match add_one(&db, &src) { ... Ok(triple) => break Some(triple), ... }
        if let Some((handle, fp, audio_fp)) = added {
            manifest.tracks.push(entry_from(&src, &handle, &fp, &audio_fp));
        }
```

with:

```rust
        match add_one(&db, &src, config, &resolved_encoder) { ... Ok(added) => break Some(added), ... }
        if let Some(added) = added {
            manifest.tracks.push(entry_from(&src, &added));
        }
```

Adjust the loop's `let added: Option<...>` type to `Option<AddedTrack>`.

- [ ] **Step 6: `build_rebuild_manifest` — set the new fields**

When rebuilding from the iPod, we don't know what encoder was used historically. Use the "unknown" sentinel so future syncs don't trigger spurious re-encodes:

```rust
pub(crate) fn build_rebuild_manifest(db: &OwnedDb) -> Manifest {
    let handles = db.list_tracks_for_rebuild();
    let tracks = handles.into_iter().map(|h| ManifestEntry {
        source_path: std::path::PathBuf::new(),
        source_mtime: 0,
        source_size: 0,
        source_fingerprint: String::new(),
        ipod_dbid: h.dbid,
        ipod_relpath: h.ipod_relpath,
        source_known: false,
        audio_fingerprint: String::new(),
        encoder: "unknown".to_string(),
        encoder_version: String::new(),
        source_format: "unknown".to_string(),
    }).collect();
    Manifest { version: 1, ipod_serial: None, tracks }
}
```

(`source_format = "unknown"` is honest — we genuinely don't know. The is_encoder_mismatch helper already carves out encoder="unknown" so no surprises.)

- [ ] **Step 7: Update do_metadata_only**

MetadataOnly doesn't re-encode, so it should preserve the existing entry's encoder/version/source_format:

```rust
    Ok(ManifestEntry {
        // ... existing fields ...
        encoder: entry.encoder.clone(),                  // preserve
        encoder_version: entry.encoder_version.clone(),  // preserve
        source_format: entry.source_format.clone(),      // preserve (audio is unchanged)
    })
```

- [ ] **Step 8: Per-format counts in the action-plan log**

Optional polish (high value, low cost): when logging the action plan, include source_format counts for visibility into the classifier's decisions:

```rust
    let by_format = sources.iter()
        .map(|s| /* TODO probe per source to get codec? */ "...".to_string())
        .collect::<Vec<_>>();
    // ... aggregate ...
```

**Caveat:** generating per-format counts in the action plan requires probing every source up-front, which we currently DON'T do (probe happens lazily inside add_one). Adding it would cost the full ffprobe walk on the source library — not free over SMB.

Recommended decision for v1: **skip the per-format pre-count.** Log per-format counts AFTER the apply loop runs, derived from `manifest.tracks.iter().map(|e| e.source_format).counts()`. That's free (the data is already in the manifest entries we just wrote) and equally informative.

```rust
    // After the apply loop's final progress.log("Done. ..."):
    let mut by_format: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for e in &manifest.tracks {
        *by_format.entry(e.source_format.clone()).or_insert(0) += 1;
    }
    if !by_format.is_empty() {
        let summary = by_format.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>().join(" ");
        progress.log(format!("Manifest by source format: {summary}"));
    }
```

- [ ] **Step 9: Build + test**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test 2>&1 | grep "test result"
```

Expected: clean build. All ~70+ tests pass (Phase 1 + 2 + 3.x + 3.z + this Phase 3 work).

If a test imports `add_one` and now fails because of the signature change, those are integration-style tests under `tests/` (if any) — update them to pass the new args, or stub `Config` + `ResolvedEncoder` via fixtures.

- [ ] **Step 10: Commit**

```powershell
git -C F:\repos\ipod-sync add src\apply_loop.rs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(apply_loop): branch on classify + record encoder/version/source_format

run() resolves the encoder once at preflight via transcode::resolve_encoder,
logs it for the user, and threads it through diff (for encoder-mismatch
detection) and every add_one call.

add_one now: probes -> classifies -> branches on Passthrough vs Transcode;
Transcode further branches on encoder.name (ffmpeg uses the existing
transcode_to_alac; refalac uses transcode_via_refalac with art written to a
temp jpg for --artwork). Returns a new AddedTrack struct carrying handle +
both fingerprints + encoder + encoder_version + source_format.

entry_from rewrites against AddedTrack so the new fields land in every
new ManifestEntry. do_metadata_only preserves the existing entry's
encoder/version/source_format (no re-encode, audio bytes unchanged).
build_rebuild_manifest sets encoder="unknown" / source_format="unknown"
so the encoder-mismatch carve-out makes future syncs no-op them.

Post-apply log shows source_format breakdown derived from the manifest
(free — no extra probes).

preflight::verify_refalac gates startup when encoder=refalac (no-op
otherwise).
EOF
)"
```

---

## Task 7: Live gate scenarios

**Files:**
- Modify: `F:\repos\ipod-sync\LEARNINGS.md` (gate record)

Manual exercise of each spec acceptance criterion. The implementer's job ends here; the user runs the scenarios on a real iPod and records observations.

- [ ] **Scenario 1: Mixed-source dry-run preview**

```powershell
cd F:\repos\ipod-sync
cargo run --release -- --dry-run --apply
# Expected output includes:
#   - "Encoder: ffmpeg (ffmpeg n7.0)" or similar
#   - action plan: add=N modify=M ...
#   - on the user's library (~1,407 tracks), look for non-flac source counts
#     reported in the post-apply "Manifest by source format" line. If the
#     library is mostly FLAC, expect flac=~1400; the existence of mp3=X,
#     aac=Y entries confirms the classifier is working on a representative
#     mix.
```

Record: did the classifier correctly identify each source format? Were there any "unsupported source codec" errors (e.g. AC3 files that Phase 2 silently transcoded)?

- [ ] **Scenario 2: Passthrough byte-for-byte verify**

Pick ONE source MP3, force it through Phase 3:

```powershell
# Start with a fresh iPod + manifest (or use a test-only source dir).
cargo run --release -- --apply
# Wait for the run to complete.

# Find the dbid the run reported for that MP3 from the progress log,
# then locate the file on the iPod (e.g. G:\iPod_Control\Music\Fnn\XXXX.mp3):
$src = "\\HOST\share\music\Artist\Album\track.mp3"
$ipod_copy = "G:\iPod_Control\Music\F12\XXXX.mp3"  # from the log

$src_hash = (Get-FileHash $src -Algorithm SHA256).Hash
$ipod_hash = (Get-FileHash $ipod_copy -Algorithm SHA256).Hash
"src:  $src_hash"
"ipod: $ipod_hash"
# Expected: identical hashes.
```

Eject the iPod and play the track on the device. Expected: plays correctly with metadata + art.

- [ ] **Scenario 3: Refalac transcode**

```powershell
# Pick a single FLAC file, sync with --encoder refalac.
cargo run --release -- --encoder refalac --apply
# After completion:
ffprobe -loglevel error -of json -show_streams "G:\iPod_Control\Music\F12\YYYY.m4a" `
    | ConvertFrom-Json `
    | Select-Object -ExpandProperty streams `
    | Where-Object codec_type -eq "audio" `
    | Select-Object codec_name, sample_rate, channels, bits_per_raw_sample
# Expected: codec_name = "alac"
```

Eject + play on iPod. Expected: plays correctly.

- [ ] **Scenario 4: Encoder mismatch triggers Modify**

```powershell
# 1. Sync with ffmpeg encoder.
cargo run --release -- --encoder ffmpeg --apply

# 2. Re-run with refalac.
cargo run --release -- --encoder refalac --dry-run
# Expected: every transcodable track shows as Modify (not Unchanged).
# Passthrough tracks stay Unchanged (encoder mismatch doesn't apply to them).
```

- [ ] **Scenario 5: `--force-reencode` works**

```powershell
cargo run --release -- --encoder ffmpeg --force-reencode --dry-run
# Expected: every transcodable track (FLAC, OGG, etc.) shows as Modify
# regardless of stored encoder match. Passthrough tracks stay Unchanged
# (--force-reencode applies to re-encoding; passthrough doesn't re-encode).
```

- [ ] **Scenario 6: Existing Phase 2 manifest upgrades cleanly**

```powershell
# 1. Find an old manifest written by Phase 2 (entries lack encoder field).
#    If you don't have one, generate one synthetically by editing a current
#    manifest copy: delete all "encoder", "encoder_version", "source_format"
#    fields from every entry, then save as a backup-restore test.
Copy-Item "$env:APPDATA\ipod-sync\manifest.json" "$env:APPDATA\ipod-sync\manifest.json.phase2-bak"

# 2. Run Phase 3 against that manifest.
cargo run --release -- --dry-run
# Expected: action plan shows Unchanged=N for all existing files (no
# spurious re-encodes from encoder field being treated as "unknown").
# Restore:
Move-Item "$env:APPDATA\ipod-sync\manifest.json.phase2-bak" "$env:APPDATA\ipod-sync\manifest.json" -Force
```

- [ ] **Scenario 7: iPod-level acceptance — 5 tracks per source-codec category**

Pick 5 tracks from each of: FLAC, MP3, AAC/M4A, WAV-passthrough (with `--passthrough-wav`), WAV-transcoded (default). Sync them via `--apply`. Eject. On the iPod:

- For each of the 25 tracks: navigate to it in the iPod's library UI, play it. Verify:
  - Title + artist + album displayed correctly.
  - Embedded art displayed (if the source had it).
  - Track plays end-to-end without skipping.
  - Track length matches source (no truncation).

Record any failures by source-codec category — e.g. "MP3 #3 had art missing"; "WAV-passthrough #5 played but iPod displayed wrong title".

- [ ] **Step 8: Record gate result in LEARNINGS.md**

Append:

```markdown
## Phase 3 gate (YYYY-MM-DD) — PASS / FAIL

- **Result:** PASS / FAIL (<reason>)
- **Scenario 1 (mixed-source dry-run):** classifier output: flac=N mp3=N aac=N alac=N wav=N opus=N ...
  - Any unsupported-codec errors? <list files / "none">
- **Scenario 2 (passthrough byte-for-byte):** MP3 src SHA256 == iPod SHA256: YES / NO
  - Playback on iPod: tags=ok art=ok audio=ok / <failures>
- **Scenario 3 (refalac transcode):** ffprobe codec_name: alac / <other>
  - Playback on iPod: YES / NO
- **Scenario 4 (encoder mismatch -> Modify):** YES / NO — N tracks promoted to Modify
- **Scenario 5 (--force-reencode):** YES / NO — N tracks promoted; passthrough stayed Unchanged
- **Scenario 6 (Phase 2 manifest upgrade):** Unchanged=N as expected / spurious re-encodes
- **Scenario 7 (per-category iPod playback):** flac=N/5 mp3=N/5 aac=N/5 wav-pt=N/5 wav-tx=N/5
- **Observations:**
  - <any surprises, TUI glitches, performance numbers>
  - <decisions deferred to Phase 3.x.y>
```

- [ ] **Step 9: Commit + tag**

```powershell
git -C F:\repos\ipod-sync add LEARNINGS.md
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "docs: Phase 3 gate result"
git -C F:\repos\ipod-sync tag -a phase-3-complete -m "$(cat <<'EOF'
Format expansion + refalac encoder complete

- transcode::classify decides Passthrough (mp3/aac/alac, opt-in wav) vs
  Transcode (flac/vorbis/opus, default wav) from probe + container.
- transcode::passthrough is std::fs::copy with parent-dir mkdir;
  preserves bytes verbatim, libgpod still does tag/art via apply_tags.
- transcode_via_refalac runs the spec's 2-step ffmpeg-decode-to-WAV ->
  refalac-encode-to-m4a pipeline; refalac --artwork carries cover art.
- ResolvedEncoder bundles encoder name + version + paths so apply_loop
  resolves once and records on every new ManifestEntry.
- ManifestEntry gained encoder, encoder_version, source_format with serde
  defaults that make Phase 2/3.x manifests deserialize cleanly.
- diff's encoder-mismatch heuristic + --force-reencode flag promote
  otherwise-Unchanged entries to Modify when the encoder differs from
  what was stored; unknown / passthrough carve-outs prevent spurious
  re-encodes on Phase 2 -> 3 upgrade.
- vendored refalac64.exe + libFLAC.dll via build.rs (BSD-2 license).
EOF
)"
```

---

## Refalac install instructions (for opt-in encoder)

Default encoder is `ffmpeg` so most users never touch refalac. If you want `--encoder refalac` (Apple's reference ALAC encoder, closest-to-iTunes bitstream), grab the latest qaac release zip from <https://github.com/nu774/qaac/releases> (look for `qaac_X.YY.zip`), extract `x64\refalac64.exe` and `x64\libFLAC.dll`, then either (a) drop both files into `F:\repos\ipod-sync\vendor\refalac\` and `cargo build` will copy them alongside the libgpod runtime DLLs in `target/<profile>/`, or (b) put `refalac64.exe` somewhere on your `PATH` and let `preflight::verify_refalac` find it via the default `Config::refalac_path = "refalac64"`. `vendor/refalac/` is gitignored so the binaries never get committed; `build.rs` emits a `cargo::warning` when the dir is empty rather than failing the build.

---

## Self-review

**Spec coverage check (against the Phase 3 spec + 2026-05-23 addendum):**

- Source classification matrix (mp3/aac/alac passthrough, flac/vorbis/opus transcode, wav opt-in) -> Task 1 ✓
- Encoder selection (default ffmpeg per addendum Change 1, refalac opt-in) -> Task 3 (cli + config) + Task 5 (resolve_encoder, verify) ✓
- Single global --encoder flag (no per-format) -> Task 3 ✓ with FUTURE comment in cli.rs per addendum Change 2
- ManifestEntry new fields: encoder + encoder_version + source_format -> Task 4 ✓ (source_format addition per addendum Change 3)
- Refalac vendoring + build.rs -> Task 5 ✓ (with vendoring-or-skip decision documented per addendum Change 4)
- Refalac pipeline (ffmpeg -> WAV -> refalac) -> Task 5 (transcode_via_refalac) + Task 6 (branch in add_one) ✓
- Encoder-mismatch in diff + --force-reencode + unknown/passthrough carve-outs -> Task 4 ✓
- verify_refalac conditional on encoder=refalac -> Task 5 + Task 6 ✓ (addendum Change 4)
- Spec acceptance criteria #1-7 -> Task 7 covers each scenario ✓

**Placeholder scan:** No "TBD / TODO / handle edge cases" — every decision is made. The "FUTURE: per-format encoder" comment in cli.rs is explicit and references the addendum (not a TODO).

**Documented limitations:**
- Per-format encoder selection is parked (addendum Change 2). FUTURE comment in code points future contributors at the addendum's shape.
- `force_reencode` is CLI-only, not persisted (Task 3 Step 4 rationale).
- Refalac vendoring is optional — graceful skip in build.rs when `vendor/refalac/` is absent. Users can pass `--refalac-path` to a system install instead (Task 5 Step 1 fallback).
- Pre-action source-format breakdown deferred (Task 6 Step 8) — would require probing every source up front; the post-apply manifest-derived breakdown is free and equally informative.

**Type consistency check:**
- `SourceAction { Passthrough, Transcode }` — same shape in classify return, add_one match, no other consumers.
- `EncoderChoice { Ffmpeg, Refalac }` — clap::ValueEnum + serde derive both; flows CLI -> Config -> PersistedConfig -> resolve_encoder.
- `ResolvedEncoder { name, version, refalac_path, ffmpeg_path }` — built once per run, passed to add_one + transcode_via_refalac.
- `AddedTrack { handle, fingerprint, audio_fingerprint, encoder, encoder_version, source_format }` — return of add_one; entry_from constructs ManifestEntry from it.
- `ManifestEntry` gains 3 String fields with serde defaults that round-trip Phase 2 -> Phase 3 cleanly.

**Scope check:** Phase 3 only. Doesn't change Phase 3.x sync semantics (MetadataOnly preserves stored encoder/version/source_format). Doesn't change Phase 3.z error-UX flow (verify_refalac reuses the same await_prompt helper).

**Sequencing:** Tasks 1, 2, 3, 4, 5 are parallel-safe with each other (touch disjoint files: transcode classify/passthrough; cli + config; manifest; vendor + transcode helpers). Task 6 is sequential (depends on all five). Task 7 is the live gate, manual.

Concentrated complexity sits in Task 6 (apply_loop wiring) and Task 5 (refalac pipeline + version probing). Tasks 1-4 are straightforward additive surface changes.
