# Phase 3 — Format expansion + refalac encoder (design)

**Goal:** stop forcing every source file through ffmpeg-ALAC transcoding. (a) When the source codec is already something the iPod plays natively, copy it bit-perfect instead. (b) When transcoding IS needed, prefer Apple's reference ALAC encoder (refalac64 from the qaac project) for the closest-to-iTunes output quality.

**Goal phrased operationally:** for a typical mixed library of FLAC + MP3 + AAC, Phase 3 changes the per-track work as follows:

| Source today | Phase 2 behavior | Phase 3 behavior |
|---|---|---|
| FLAC | ffmpeg → ALAC (~3 sec/track) | refalac (default) or ffmpeg (`--encoder ffmpeg`) → ALAC |
| MP3 | ffmpeg → ALAC (re-encodes, quality loss) | **copy** (~0.1 sec/track, bit-perfect) |
| AAC / M4A | ffmpeg → ALAC (re-encodes) | **copy** |
| ALAC | ffmpeg → ALAC (re-encodes pointlessly) | **copy** |
| WAV / AIFF (PCM) | ffmpeg → ALAC | ffmpeg → ALAC (default) or **copy** (`--passthrough-wav`) |
| OGG Vorbis / Opus | ffmpeg → ALAC | refalac or ffmpeg → ALAC |
| Anything else | error | error (out of scope) |

**Scope:** Phase 3 only. Does not touch multi-iPod, daemon, GUI — see `2026-05-18-post-v1-roadmap.md` for context.

---

## What the iPod Classic 7G plays natively

Verified against Apple's spec sheet for the 7th-generation Classic and against the `Itdb_FileType` enum in libgpod's source. Anything not in this list will be rejected by the iPod's firmware at playback time even if libgpod accepts it in the DB.

| Codec | Containers | Notes |
|---|---|---|
| MP3 | `.mp3` | CBR 16-320 kbps; VBR up to 320 |
| AAC-LC, HE-AAC | `.m4a`, `.mp4`, `.aac` | Up to 320 kbps |
| Apple Lossless (ALAC) | `.m4a`, `.mp4` | This is what we transcode FLAC to today |
| Audible | `.aa`, `.aax` | DRM; out of scope |
| AIFF (PCM) | `.aif`, `.aiff` | Uncompressed |
| WAV (PCM) | `.wav` | Uncompressed |

Things NOT natively supported: FLAC, OGG Vorbis, Opus, WMA, AC3.

## Source classification

A new function `transcode::classify(probe: &ProbeOutput, config: &Config) -> SourceAction` produces one of:

```rust
pub enum SourceAction {
    /// Copy the source file to the iPod as-is. ffprobe-extracted tags + the
    /// embedded art are preserved verbatim. iPod plays the file with its
    /// native decoder.
    Passthrough,
    /// Transcode the source to ALAC via the configured encoder.
    Transcode,
}
```

Decision matrix (matches the codec/container heuristic):

| Source codec_name | Container | Action | Notes |
|---|---|---|---|
| `flac` | `flac` | `Transcode` | Today's behavior. iPod doesn't play FLAC. |
| `mp3` | `mp3` | `Passthrough` | |
| `aac` | `m4a`, `mp4`, `aac`, `mov` | `Passthrough` | Includes AAC-LC and HE-AAC variants |
| `alac` | `m4a`, `mp4`, `mov` | `Passthrough` | |
| `pcm_s16le`, `pcm_s24le`, etc. | `wav`, `aiff` | `Passthrough` if `config.passthrough_wav` else `Transcode` | Default transcode; user opts in to bit-perfect |
| `vorbis` | `ogg` | `Transcode` | |
| `opus` | `opus`, `ogg` | `Transcode` | |
| anything else | * | (return error from classify) | Caller surfaces "unsupported source codec X in <file>" and aborts per SPEC §8 row 5 stop-on-first-error |

ffprobe's `format.format_name` is comma-separated for multi-format containers (`mov,mp4,m4a,3gp,3g2,mj2`). The classifier checks all components.

## Encoder selection

```rust
pub enum EncoderChoice { Auto, Refalac, Ffmpeg }
```

CLI:
```
--encoder <auto|refalac|ffmpeg>   Default: auto
--refalac-path <PATH>             Default: refalac64 (PATH lookup or vendored copy)
```

`auto` semantics at startup:
- Try `refalac64 --check` (or whatever the existence probe is — verify against actual refalac CLI; likely `refalac --formats` or just `refalac --help` returns 0).
- If that succeeds → use refalac.
- If it fails → use ffmpeg, emit ONE warning line:
  ```
  warn: refalac64 not found on PATH; falling back to ffmpeg ALAC encoder.
        For Apple's reference encoder, install qaac (https://github.com/nu774/qaac/releases)
        and put refalac64.exe on PATH, or rebuild ipod-sync with the vendored copy.
  ```

`refalac` explicit: hard error if missing. No surprise fallback.
`ffmpeg` explicit: always use ffmpeg.

### Why default to refalac

Refalac wraps Apple's reference ALAC encoder (libalac, BSD-licensed by Apple). It produces the closest-to-iTunes-native output and is what audiophile ALAC archives are typically encoded with. Quality vs ffmpeg ALAC is debatable for listening but bit-identical-to-iTunes matters for purists and for confidence that the iPod hardware decoder is exercising its happiest code path.

The downside is the install friction. Mitigation: vendor `refalac64.exe` + `libFLAC.dll` alongside the existing libgpod runtime DLLs (~3 MB added). With the vendor copy, `auto` mode reliably lands on refalac without the user needing to install anything separately.

### Refalac pipeline (Option A from brainstorming — 2-step via WAV)

Refalac only reads WAV/AIFF (+ FLAC with libFLAC.dll alongside). For uniform handling of all source formats, Phase 3 uses a 2-step pipeline:

```
source.<anything>  →  ffmpeg decode  →  temp.wav  →  refalac64  →  temp.m4a  →  libgpod
```

Both temps get cleaned up after libgpod's `cp_track_to_ipod` succeeds.

The 2-step is intentionally simple, not optimal. Future work (Phase 4+, captured in roadmap) is to investigate piping (ffmpeg stdout → refalac stdin) to eliminate the WAV intermediate. The 2-step's correctness is the priority for v1 of Phase 3.

Refalac command shape:
```
refalac64 --silent -o <temp.m4a> --artwork <temp_art.jpg> <temp.wav>
```
Tags are NOT carried by the WAV — refalac would lose them. Tags are populated via libgpod's `apply_tags` from the ffprobe metadata captured before transcoding (same pattern as Phase 1). Cover art is extracted by the existing `extract_cover_art` and passed via `--artwork`.

### ffmpeg pipeline (unchanged from Phase 2)

```
source.<anything>  →  ffmpeg → temp.m4a (ALAC, art in-band)  →  libgpod
```

Existing `ffmpeg_args` in `transcode.rs` already produces this.

## Pass-through pipeline

```
source.mp3 (or .m4a or whatever)  →  std::fs::copy → temp.<ext>  →  libgpod
```

That's it. No re-encoding. The temp file has the same extension as the source (libgpod doesn't care). Tags are still extracted via ffprobe + applied via libgpod's `apply_tags` (the iPod's iTunesDB stores tags separately from the file). Art is preserved as part of the source file's existing structure — libgpod's thumbnail-write path uses the cover art bytes we extract via ffmpeg's `extract_cover_art`, which works on MP3/M4A/etc. just as it does on FLAC.

One subtlety: pass-through tracks are RECORDED IN THE MANIFEST as `encoder: "passthrough"`. The diff logic treats that as immune to encoder-mismatch checks — switching `--encoder` doesn't re-process a pass-through file (there's no encoding to redo).

## Manifest schema impact

`ManifestEntry` gains two fields:

```rust
pub struct ManifestEntry {
    // existing fields ...
    /// "refalac" | "ffmpeg" | "passthrough" | "unknown"
    #[serde(default = "default_encoder")]
    pub encoder: String,
    /// e.g. "refalac 1.85" or "ffmpeg n7.0". Empty string for passthrough or unknown.
    #[serde(default)]
    pub encoder_version: String,
}

fn default_encoder() -> String { "unknown".to_string() }
```

Diff logic gains one new branch on top of the Phase 2 `(fingerprint match && size match) → Unchanged` check:

```rust
fn is_encoder_mismatch(entry: &ManifestEntry, target: &str, force: bool) -> bool {
    if force { return true; }
    if entry.encoder == "unknown" { return false; }      // backwards-compat: don't punish Phase 2 manifests
    if entry.encoder == "passthrough" { return false; }  // pass-through has no encoder
    entry.encoder != target
}
```

When `is_encoder_mismatch` returns true on an otherwise-unchanged source, the action becomes `Modify` instead of `Unchanged`.

`--force-reencode` flag bypasses the heuristic: every Add or Modify track is treated as "must (re-)transcode" regardless of encoder field. Useful for refreshing the iPod after an ffmpeg or refalac upgrade.

Schema version stays at 1. Existing Phase 2 manifests deserialize cleanly because both new fields are `#[serde(default)]`.

## CLI surface summary

New flags added to `cli::Cli`:

```
--encoder <auto|refalac|ffmpeg>     Default: auto
--refalac-path <PATH>               Default: refalac64 (PATH or vendored)
--passthrough-wav                   Copy WAV/AIFF bit-perfect (default: transcode to ALAC for space)
--force-reencode                    Treat every Add/Modify track as "must re-encode" (ignores manifest encoder field)
```

The existing `--ffmpeg <PATH>` stays unchanged.

## Code surface changes

| File | Change |
|---|---|
| `src/cli.rs` | Add the 4 new `#[arg]` fields. Add tests for defaults + override behavior. |
| `src/config.rs` | Add `encoder: EncoderChoice` + `refalac_path: PathBuf` + `passthrough_wav: bool` + `force_reencode: bool` to `Config`. New `EncoderChoice` enum with `clap::ValueEnum` derive. Tests for resolve(). |
| `src/source.rs` | `SourceEntry` is **unchanged** — we don't probe codec at walk time (would add ~80 sec to walks over SMB). Probing happens lazily during the per-track action processing. |
| `src/transcode.rs` | New `classify(&ProbeOutput, &Config) -> Result<SourceAction>`. New `passthrough(src, dst) -> Result<()>` (basically `std::fs::copy`). New `temp_wav_path()`. New `transcode_via_refalac(src, dst, art_jpg, encoder_version) -> Result<()>` doing the ffmpeg→WAV→refalac 2-step. New `EncoderChoice` resolution / `resolve_encoder(&Config) -> Result<ResolvedEncoder>` returning a struct with the chosen encoder name + version string (for the manifest entry). Updated `verify_tools_available` to also probe refalac when needed. New unit tests for `classify` covering each row of the decision matrix. |
| `src/manifest.rs` | `ManifestEntry` gets `encoder` + `encoder_version` fields with serde defaults. `diff` gains the encoder-mismatch branch (with `--force-reencode` and `unknown`/`passthrough` carve-outs). New unit tests for the encoder-mismatch + force-reencode + unknown-preserved cases. |
| `src/main.rs` | The per-track `add_one` gains a `classify` call after `probe`. Branches into `passthrough` or `transcode_via_<chosen-encoder>` based on the result. Records `encoder` + `encoder_version` in the resulting `ManifestEntry`. `tool_check` at startup verifies the chosen encoder exists. |
| `vendor/refalac/` (new dir) | Vendored `refalac64.exe` + `libFLAC.dll` (qaac project, BSD-2 licensed). `build.rs` copies them to `target/<profile>/` next to the existing libgpod runtime DLLs. |
| `build.rs` | Extend the DLL/exe copy block to include the new `vendor/refalac/` files. |
| `LEARNINGS.md` | Phase 3 carry-forwards: piping investigation parked for Phase 4+, decision rationale for refalac default, schema migration approach. |

## Out of scope

- **Piping intermediates** (ffmpeg stdout → refalac stdin) — Phase 4+ investigation per roadmap.
- **Smart-playlist / play-count / ratings sync** — still SPEC §7 out of scope.
- **Multi-iPod** — Phase 4.
- **Daemon / tray / auto-sync** — Phase 5.
- **GUI** — Phase 6.
- **Other encoders** (qaac for AAC output, fdkaac, etc.) — ALAC is the only lossless target the iPod plays. Anyone wanting smaller files with AAC can pass-through their pre-encoded source.
- **Re-encoding existing iPod tracks in bulk** when refalac becomes default and old tracks are ffmpeg-encoded — handled organically through `--force-reencode` or by waiting for source-file modifications to trigger the normal Modify path. Phase 3 does not include a "scan and re-encode mismatched tracks even though sources are unchanged" mode (the encoder-mismatch heuristic already does this on the next normal run; bulk-scan as an explicit one-shot is unneeded).

## Acceptance criteria for Phase 3 gate

1. **Mixed-source dry-run preview** against the user's library shows accurate Pass-through vs Transcode counts. (E.g., if 200 of 1,407 tracks are MP3, we see "passthrough: 200, transcode: 1,207".)
2. **Pass-through track on iPod**: take one MP3 from source, sync it, verify the on-iPod file is the SAME bytes as the source (byte-for-byte compare against `G:\iPod_Control\Music\Fnn\*.mp3`).
3. **Refalac transcode**: sync one FLAC with `--encoder refalac`, verify the resulting iPod `.m4a` is a valid ALAC file (ffprobe shows `codec_name: alac`) and the iPod plays it.
4. **Encoder mismatch triggers Modify**: sync a FLAC with `--encoder ffmpeg`, then re-run with `--encoder refalac` — the second run shows the track as `Modify` and re-encodes it.
5. **`--force-reencode` works**: re-run with the flag, every transcodable track shows as `Modify` regardless of encoder match.
6. **Existing Phase 2 manifest upgrades cleanly**: a manifest written by Phase 2 (no encoder field) loads in Phase 3 without errors, and a no-changes run after Phase 3 upgrade shows `Unchanged=N` (no spurious re-encoding due to encoder field treated as "unknown").
7. **iPod-level acceptance**: pick 5 tracks from each source-codec category (FLAC, MP3, AAC, WAV-passthrough, WAV-transcoded), sync, eject, verify each plays with correct metadata + art on the iPod.

## Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Refalac on Windows MSVC has quirks (process spawning, stdin/stdout handling) | Medium | Medium | Vendor a known-good build from the qaac project releases. Test the 2-step pipeline first; pipes can come later (Phase 4+). |
| iPod rejects refalac's ALAC output (different bitstream variant than ffmpeg's) | Low | Medium | Test on real hardware in Acceptance criterion #3 before committing the default. If rejected, demote refalac to opt-in. |
| ffprobe codec_name vs format_name disagreement on edge cases (e.g. `mp3` codec in `m4a` container, or some weird MOV-wrapped AAC) | Low | Low | Decision matrix uses BOTH codec_name AND container; explicit error on anything ambiguous. |
| Pass-through MP3 with non-iPod-friendly bit rates (e.g. 384 kbps Suno-style files) | Low | Low | iPod plays up to 320 kbps; higher rates may stutter. Phase 3 doesn't validate bit rates — pass-through trusts the source. Phase 4 could add a `--validate-bitrates` check. |
| Pass-through file gets a different `ipod_relpath` than transcoded files would | Low | None | libgpod handles the Fnn distribution by content, not extension. .mp3 + .m4a land in the same Fnn buckets per `cp_track_to_ipod`. |
| Manifest encoder-mismatch creates a thundering re-encode on first refalac default switch | Medium (the FIRST upgrade run) | Medium (user waits a long time unexpectedly) | The `unknown` sentinel for existing Phase 2 entries means there's NO thundering re-encode on the Phase 2→Phase 3 transition. The thundering happens only if user explicitly re-runs an old Phase 3 manifest with the other encoder. That's user-initiated and expected. |
