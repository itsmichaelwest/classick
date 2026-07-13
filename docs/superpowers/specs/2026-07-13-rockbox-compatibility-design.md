# Rockbox Compatibility (self-describing ALAC library) — Design

**Status:** approved design, ready for implementation plan
**Date:** 2026-07-13
**Scope:** Make classick's synced ALAC library readable by Rockbox firmware
*and* Apple firmware from a single shared copy of each track, gated by an
opt-in toggle. No separate Rockbox library, no on-device DB generation.

## Problem & goal

The user dual-boots their iPod Classic (MC293) between Apple firmware and
Rockbox. They want **both firmwares to play the same music** — Apple firmware
via the iTunesDB (as today), Rockbox via its own tag-scanning database — from
**one copy** of each track, not a duplicated library.

Apple firmware cannot play FLAC, so the shared on-device format is **ALAC**
(`.m4a`) — lossless, played by both firmwares, and already what classick
writes. The obstacle: **Rockbox reads track metadata and cover art from the
file itself, not from the iTunesDB.** On macOS, classick's afconvert path
produces a **bare** `.m4a` (no embedded tags, no embedded art); metadata lives
only in the iTunesDB and art only in libgpod's `Artwork` ithmb blobs. Booting
Rockbox against such a library shows a pile of untitled, art-less tracks.

**Goal:** when enabled, make each transcoded `.m4a` **self-describing** —
embedded Vorbis-equivalent MP4 tags + a normalized cover-art `covr` atom — so
the iTunesDB serves Apple firmware and the file's own tags serve Rockbox, from
one shared file. This is exactly what real iTunes-synced `.m4a` files look
like; classick's afconvert path stripping them is a latent bug.

## Confirmed facts (research + on-device, 2026-07-13)

- **Format:** ALAC is lossless and played by both Apple firmware and Rockbox;
  Rockbox reads embedded art from MP4/ALAC (`covr`) but **not** from FLAC.
  ALAC-for-both is bit-identical audio — no quality trade-off vs FLAC.
- **Rockbox library access:** Rockbox catalogs by tag content, not path, and
  scans the obfuscated `iPod_Control/Music/F**/*.m4a` tree fine. It reads
  standard MP4 `ilst` tags (title/artist/album/albumartist/track/disc/
  genre/year) and embedded `covr` art.
- **Rockbox database:** built **on-device** ("Initialize now" once, then
  auto-update). The host-side `database_*.tcd` format is reverse-engineered and
  version-fragile — classick will **not** generate it.
- **Rockbox art format:** needs **baseline** (not progressive) JPEG; no single
  mandated pixel size (theme-driven, ~100–500px typical).
- **Dual-boot:** Apple firmware, `iTunesDB`, and `iPod_Control` remain fully
  present and untouched; Rockbox bypasses them entirely. Detected via
  `.rockbox/rockbox-info.txt` — but classick needs **no** Rockbox detection
  (see Architecture).
- **classick today:** afconvert (macOS) outputs bare `.m4a`; ffmpeg
  (Windows/Linux) implicitly copies the attached picture via `-map 0:v`.
  Passthrough sources (mp3/aac/alac) keep their own tags + art already.
  `lofty 0.22` is already a dependency and writes MP4 tags + cover art.
  `SaveConfig` daemon command + macOS SwiftUI app (`ui/macos`) already exist.

## Architecture

The insight that collapses this feature: it is the **same Apple-firmware iPod**
classick already syncs. classick still writes the iTunesDB via libgpod exactly
as now. Therefore:

- **No** separate music tree, **no** `--rockbox` sync mode, **no** Rockbox
  mount detection, **no** manifest fork, **no** parallel apply loop.
- The entire feature is: **transcoded `.m4a` output carries embedded tags +
  normalized art**, gated by one config bool.

A single config flag `rockbox_compat` (**default `false`**) gates one new
unified metadata-embed step. Designed as **scaffolding toward always-on**: the
embed logic is unconditional and isolated in its own module; only the gate is
the bool, so changing the default to `true` (or removing the gate) later is a
one-line change with no restructuring.

### Unify transcode to audio-only

Both transcode backends are changed to output **audio-only** ALAC (afconvert
already does; ffmpeg drops its `-map 0:v` / `-disposition attached_pic` video
mapping). The **one** new embed step becomes the single place that writes tags
+ art into the `.m4a`. This makes both platforms produce byte-identical
behaviour per the toggle (bare when off, self-describing when on) and gives one
code path to reason about and test. Apple firmware is unaffected — it reads art
from the ithmb `ArtworkDB`, never the `.m4a`'s atoms.

### Unified art pipeline

Album art is **normalized once** and reused for both consumers:
`decode (any format) → downscale to ≤600px → encode baseline JPEG`. The
normalized image feeds **both** the embedded `covr` atom (Rockbox) **and**
libgpod's thumbnail generator (Apple firmware, replacing the raw source bytes
currently passed to `itdb_track_set_thumbnails_from_data`). 600px is generous
enough that Apple's thumbnails (max ~320px for the F1069 cover format) see no
quality loss, and it guarantees Rockbox gets a decodable baseline JPEG. This
also makes the Apple art path more robust to odd source formats.

## Components & interfaces

New module `crates/classick/src/artwork.rs` (art normalization is not
Rockbox-specific — it serves the Apple path too):

```rust
/// Decode cover-art bytes of any common format, downscale so the longest edge
/// is ≤ MAX_ART_EDGE (600), and re-encode as a baseline JPEG. Returns the
/// normalized JPEG bytes. Used for BOTH the embedded covr atom (Rockbox) and
/// libgpod's ithmb thumbnail input (Apple firmware).
pub fn normalize(source_art: &[u8]) -> anyhow::Result<Vec<u8>>;

/// Embed MP4 `ilst` metadata tags + a `covr` cover-art atom into an existing
/// `.m4a` file, using lofty. Overwrites any existing tags/art (idempotent).
/// Called on the transcode worker (pure file I/O — never touches libgpod).
pub fn embed_track_metadata(
    m4a: &std::path::Path,
    tags: &crate::ipod::db::Tags,
    art: Option<&[u8]>,
) -> anyhow::Result<()>;
```

- **New dependency:** `image` crate (mainstream, actively maintained; pin a
  current version, e.g. `image = "0.25"`) for decode/resize/baseline-JPEG
  encode. `lofty` (already present) handles the MP4 tag/art write.
- **Config:** `PersistedConfig.rockbox_compat: bool` (default `false`),
  resolved into `Config.rockbox_compat` exactly like `encoder` is
  (`config.rs`), round-tripped in `config_file.rs`.
- **CLI:** `--rockbox-compat` flag on `Cli` (`cli.rs`), one-shot override
  persistable with `--save-config`, mirroring the existing encoder flags.
- **Daemon wire:** the existing `DaemonCommand::SaveConfig` payload
  (`ipc_daemon.rs`) gains an **optional** `rockbox_compat: Option<bool>` field
  (additive, backward-compatible). Documented in `docs/ipc-protocol.md`; treat
  as a minor daemon-proto bump. The `--ipc-mode --apply` subprocess spawned by
  `daemon/sync_orchestrator.rs::build_command` passes `--rockbox-compat` when
  the setting is on.
- **macOS UI:** a Settings toggle in `ui/macos` (SwiftUI), default off, wired
  through `SaveConfig`, with help text noting the retroactive caveat (below).

## Data flow

In `apply_loop::transcode_one` (worker thread, filesystem-only):

```
transcode source → temp .m4a   (audio-only, both backends)
if source has embedded art:
    normalized = artwork::normalize(extracted_art_bytes)   [once]
    art_for_libgpod = normalized                           [replaces raw bytes]
    if config.rockbox_compat:
        artwork::embed_track_metadata(temp_m4a, &tags, Some(&normalized))
else if config.rockbox_compat:
    artwork::embed_track_metadata(temp_m4a, &tags, None)   [tags only]
→ commit_transcoded  (libgpod copies the now-self-describing file + writes DB)
```

Embedding happens on the worker **before** `commit_transcoded` hands the file
to libgpod (libgpod copies the file as-is to `iPod_Control/Music`), so the
embedded metadata rides along into the device copy. Tags are already available
from the probe in `transcode_one`.

## Error handling

Non-fatal, matching the existing "add track without art rather than fail the
sync" philosophy:
- Art normalization failure → `warn!`, fall back to the raw source bytes for
  the libgpod path, skip the embedded `covr` (tags still embedded). Never abort.
- Tag/art embed failure → `warn!`, continue with the (bare-but-valid) `.m4a`.
  Apple firmware still works via the iTunesDB; only Rockbox metadata degrades.
- Never fail a sync because Rockbox-compat embedding failed.

## Safety constraints (must hold; verify on-device)

1. **Apple art unchanged.** The unified art pipeline now feeds libgpod a
   normalized 600px baseline JPEG instead of raw source bytes. The on-device
   smoke must confirm Apple-firmware cover art still displays (this is the path
   we just fixed via SysInfoExtended — do not regress it).
2. **Apple playback unchanged.** Embedded `.m4a` tags/art must not perturb the
   iTunesDB or libgpod's file copy; Apple firmware reads the DB, not the file's
   atoms (real iTunes files carry these atoms). Verify tracks still play under
   Apple firmware.
3. **Dual-boot integrity.** Nothing touches `iTunesDB`/`iPod_Control` structure
   differently; we only enrich `.m4a` atoms. No partition/signature impact.

## Edge cases

- **Retroactive:** the toggle affects only **newly transcoded** tracks. Flipping
  it on does not rewrite an already-synced bare library; to convert an existing
  library, re-sync with `--force-reencode`. The UI toggle's help text states
  this. (Auto-marking affected tracks for re-encode on a toggle change is a
  possible future enhancement — out of scope.)
- **Passthrough sources** (mp3/aac/alac) already carry their own tags + art and
  are Rockbox-ready regardless of the toggle. Scope is strictly transcoded
  (FLAC→ALAC) output; passthrough files are not modified.
- **No source art:** when the toggle is on and a track has no embedded art,
  embed tags only (no `covr`).
- **Rockbox database:** classick writes none. The user runs Rockbox's
  "Initialize now" once; subsequent syncs are picked up by auto-update.

## Testing

- **Unit (`artwork.rs`):** `normalize` accepts JPEG/PNG input, outputs a
  baseline JPEG with the longest edge ≤600px; `embed_track_metadata` writes the
  expected tags + `covr` and a lofty read-back round-trips them; embed with
  `art: None` writes tags only.
- **Unit (config):** `rockbox_compat` resolves CLI > persisted > default(false)
  like `encoder`; round-trips through `PersistedConfig`.
- **Integration:** a transcoded fixture emerges self-describing (tags+art) when
  the toggle is on, bare when off; the ffmpeg path no longer bakes in a video
  stream.
- **On-device smoke (merge gate):** enable the toggle → sync an album → boot
  Rockbox → "Initialize now" → confirm tracks show correct title/artist/album
  **and cover art** on the Rockbox screen; reboot Apple firmware → confirm cover
  art + playback still correct (safety constraint 1–2).

## Out of scope (deferred / rejected)

- **Separate FLAC library for Rockbox.** Rejected: Apple can't play FLAC, so it
  would mean two copies per track (~2x space) — not "the same music."
- **Host-side Rockbox `database_*.tcd` generation.** Deferred: reverse-
  engineered, version-fragile format. On-device scan is the robust default.
- **Always-on (no toggle).** Deferred by decision: ship the toggle now
  (default off) with clean scaffolding so a later default-flip to always-on is
  trivial.
- **Auto re-encode on toggle change.** Out of scope; `--force-reencode` covers
  the manual path.

## Decisions log

- **ALAC-only shared library** (not FLAC+ALAC, not FLAC-only): user-selected;
  lossless, one copy, both firmwares.
- **Toggle, default off, with scaffolding toward always-on, surfaced in the
  macOS UI:** user-directed.
- **Normalize art to baseline JPEG (≤600px), shared by Rockbox covr + Apple
  ithmb:** user-selected (helps the Apple path too).
- **Unify transcode to audio-only; one embed step owns tags+art:**
  user-selected for a single, platform-consistent code path.
