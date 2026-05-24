# Phase 3 spec addendum — 2026-05-23 design tweaks

**Companion to:** `2026-05-18-phase-3-formats-and-encoders.md` (original Phase 3 design).

## Why an addendum

The original Phase 3 spec was reviewed in a 2026-05-23 scoping session. ~95% of it survives unchanged. This addendum captures the four adjustments that came out of that conversation so the implementation plan (`docs/superpowers/plans/2026-05-23-phase-3-formats-encoder.md`) has one canonical source of truth for the decisions that differ from the original.

If a section of the original spec isn't mentioned here, assume it stands as written.

---

## Change 1 — Default encoder: `ffmpeg`, not `auto`

The spec proposed `--encoder <auto|refalac|ffmpeg>` with `auto` defaulting to refalac-when-available and silently falling back to ffmpeg. The user prefers an **explicit default of `ffmpeg`** with refalac as opt-in via `--encoder refalac`.

**Rationale:**

- ffmpeg has been the encoder for every track currently on the user's iPod (the Phase 2 Gate C run processed 1,407 tracks through ffmpeg without issue). It is the known-good baseline.
- The spec's `auto` mode introduces a runtime probe + a fallback warning line that the user would never want to see in steady state. Either refalac is there or it's not — the user knows which they want.
- Refalac value (Apple's reference encoder, closest-to-iTunes output) is real but niche. Opt-in is the right ergonomic.
- Re-encoder-mismatch churn on the first switch to refalac is now user-initiated (`--encoder refalac` on an ffmpeg manifest) instead of auto-triggered by tool detection. Predictable.

**Resulting enum:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum EncoderChoice { Ffmpeg, Refalac }
```

No `Auto` variant. The spec's `auto`-mode probe-and-fallback code is dropped.

`--encoder` defaults to `ffmpeg`. `config.toml` `encoder = "ffmpeg"` is the persisted default.

`refalac` explicit: hard error at preflight if the binary isn't reachable (vendored copy or PATH). No surprise fallback to ffmpeg — the user asked for refalac specifically.

---

## Change 2 — Single global `--encoder` flag, not per-format

In the scoping session the user suggested per-format encoder config (e.g. `[encoder.flac] = "refalac"`, `[encoder.opus] = "ffmpeg"` in config.toml). After discussion, **rejected** in favor of a single global `--encoder`.

**Rationale (push-back):**

- The encoder operates on **already-decoded audio**. By the time the bytes reach refalac or ffmpeg's ALAC encoder, the source codec is irrelevant — it's all PCM. There is no quality-of-encoder-output dependency on source format. The choice of refalac-vs-ffmpeg is a single decision: "do I want Apple's reference ALAC bitstream or ffmpeg's?"
- Per-format adds a config surface area (a TOML table with N keys) without delivering any user-observable benefit. The most a user gains is the ability to say "use ffmpeg for OGG but refalac for FLAC" — but the resulting ALAC stream is no better or worse for one source codec than another.
- Per-format complicates the manifest. The encoder-mismatch heuristic in `manifest::diff` would need to compare-by-source-format rather than globally; that's a per-entry resolution path that nobody asked for.
- The original spec's per-format design is **trivial to add later** if user feedback ever wants it (see "Future-revival shape" below). Keeping it out of v1 isn't a one-way door.

**Future-revival shape** (if a future user disagrees and wants per-format encoder selection):

```toml
# config.toml — future design, NOT in Phase 3 v1
[encoder]
default = "ffmpeg"          # global fallback
flac    = "refalac"         # per source codec_name override
vorbis  = "ffmpeg"
opus    = "ffmpeg"
```

In code:

```rust
// Future design — NOT shipped in Phase 3 v1
pub struct EncoderConfig {
    pub default: EncoderChoice,
    pub per_format: std::collections::HashMap<String, EncoderChoice>,
}
impl EncoderConfig {
    pub fn for_source(&self, codec_name: &str) -> EncoderChoice {
        self.per_format.get(codec_name).copied().unwrap_or(self.default)
    }
}
```

The rest of the Phase 3 design (classify(), passthrough(), transcode_via_refalac(), the manifest encoder-mismatch heuristic) is **completely unaffected** by this future restructure. Everything below the encoder-resolution layer takes a single `EncoderChoice` per track; whether that choice came from a global or per-format lookup is invisible to downstream code.

Per-format encoder selection is parked. Future-contributor note: search for "FUTURE: per-format encoder" comments left in `src/config.rs` next to the `encoder: EncoderChoice` field if revisiting.

---

## Change 3 — `ManifestEntry.source_format` field added

In addition to the spec's `encoder` + `encoder_version` fields, the user explicitly asked to record the per-track **source format** in the manifest.

**Field shape:**

```rust
pub struct ManifestEntry {
    // ... existing Phase 2/3.x fields (source_path, source_mtime, source_size,
    //     source_fingerprint, ipod_dbid, ipod_relpath, source_known, audio_fingerprint) ...

    /// ffprobe codec_name of the source at sync time. "flac" | "mp3" | "aac" |
    /// "alac" | "vorbis" | "opus" | "pcm_s16le" | "pcm_s24le" | etc.
    /// Phase 2 manifests deserialize with the default "flac" since that's the
    /// only format the Phase 2 pipeline supported.
    #[serde(default = "default_source_format")]
    pub source_format: String,

    /// "refalac" | "ffmpeg" | "passthrough" | "unknown".
    #[serde(default = "default_encoder")]
    pub encoder: String,

    /// e.g. "refalac 1.85" or "ffmpeg n7.0". Empty string for passthrough or unknown.
    #[serde(default)]
    pub encoder_version: String,
}

fn default_source_format() -> String { "flac".to_string() }
fn default_encoder() -> String { "unknown".to_string() }
```

**Where it gets set:** in `apply_loop::add_one`, immediately after `classify(&probe, &config)` returns. The codec_name comes from `probe.streams[].codec_name` of the first audio stream (we add a `codec_name` field to `ProbeStream` as part of Task 1).

**Used for:**

1. **Statistics in dry-run / review output.** Instead of just `add=200 modify=1100 unchanged=107`, the summary can show per-source-format counts: `add: flac=180 mp3=15 aac=5`. Helps the user sanity-check that the classifier is doing what they expect on a mixed library.
2. **Future format-change detection.** If a source file's codec_name ever changes (e.g. user re-rips a CD from MP3 to FLAC), the manifest entry's `source_format` flags it as a meaningful change worth surfacing in the action plan rather than just "Modify". Not exercised in Phase 3 v1 — the diff still treats it the same as any other Modify — but the data is in place when Phase 4+ wants it.
3. **Encoder-mismatch carve-out for passthrough.** `is_encoder_mismatch` already special-cases `encoder == "passthrough"` (the spec's existing logic). `source_format` is the field that lets a future report break down passthrough counts by source codec.

**Back-compat:** Phase 2 manifests (and any Phase 2→3 migration) read with `source_format = "flac"` by default. This is **safe** because Phase 2 only ever processed FLAC sources — there are no manifest entries where the real source format was anything else. The default is technically a guess, but it's a correct guess given the historical scope.

---

## Change 4 — Refalac vendoring stays, with one tweak

The spec's plan to vendor `refalac64.exe` + `libFLAC.dll` under `vendor/refalac/` is retained. The tweak: the **startup probe is conditional on the user requesting refalac**, not unconditional.

In the original spec, `auto` mode required probing refalac at startup to decide which encoder to use. With Change 1 (no `auto` mode), there's no reason to probe refalac when `encoder = ffmpeg` — the binary may not even be vendored on this user's build. The probe runs ONLY when `config.encoder == EncoderChoice::Refalac`.

**Result:**

- `preflight::verify_refalac(config)` is a new gate that runs after `verify_ffmpeg` and BEFORE `resolve_ipod_mount`, conditional on encoder=refalac.
- On failure, it surfaces a TUI prompt with the same Retry/Abort options as `verify_ffmpeg` (using the Phase 3.z `await_prompt` helper). Hint text: "refalac64.exe not found at <path>. Either ship a vendored copy under vendor/refalac/ or pass --refalac-path <path>."
- `verify_ffmpeg` is unchanged — ffmpeg is always required (we always need it for decode-to-WAV when encoder=refalac, and for the entire ffmpeg encoder path).

**Vendoring practicality note:** the implementation plan's Task 5 documents the download/install steps for refalac64.exe + libFLAC.dll. If vendoring the binary itself proves awkward (e.g. license-redistribution concerns or upstream-mirror flakiness), the v1 fallback is to skip vendoring and require the user to install qaac themselves; the `--refalac-path` flag covers that case cleanly.

---

## Updated CLI surface

Resulting set of new flags (consolidating the spec's table with Changes 1 and 4):

```
--encoder <ffmpeg|refalac>          Default: ffmpeg (was: auto in spec)
--refalac-path <PATH>               Default: refalac64 (PATH lookup or vendored copy)
--passthrough-wav                   Copy WAV/AIFF bit-perfect (default: transcode to ALAC for space)
--force-reencode                    Treat every Add/Modify track as "must re-encode"
                                     (ignores manifest encoder field)
```

`--passthrough-wav` is **kept as opt-in** as the spec defined. The original spec's "WAV/AIFF default to Transcode" decision survives: bit-perfect WAV files are large; most users prefer the space savings.

The existing `--ffmpeg <PATH>` (Phase 1) stays unchanged.

## Updated `config.toml` example

```toml
source = '\\HOST\share\music'
encoder = "ffmpeg"        # or "refalac" — see --encoder
passthrough_wav = false   # opt-in for bit-perfect WAV/AIFF; spec default
force_reencode = false    # one-shot only; not really a persistent setting,
                          # but persisted-default = false makes it explicit
refalac_path = "refalac64"  # only consulted when encoder = "refalac"
                            # default tries PATH then the vendored copy
```

`encoder` and `passthrough_wav` are the additions worth persisting in v1. `force_reencode` is borderline (it's almost always a one-shot CLI flag); persisting `false` as the default is harmless and keeps the file schema uniform.

## Out of scope (unchanged from spec, plus one addition)

Everything in the spec's "Out of scope" section still applies. One addition motivated by Change 2:

- **Per-format encoder selection** — see Change 2 for the future-revival shape. Tracked as a future enhancement; not in Phase 3 v1.

The spec's other out-of-scopes (piping intermediates, smart-playlist sync, metadata-only smart-update [separate Phase 3.x], multi-iPod, daemon, GUI, other encoders, bulk re-encoding) stand.
