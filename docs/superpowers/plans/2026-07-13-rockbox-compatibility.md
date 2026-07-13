# Rockbox Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make classick's transcoded ALAC (`.m4a`) output self-describing — embedded MP4 tags + a normalized cover-art `covr` atom — so one shared library serves Apple firmware (via iTunesDB) *and* Rockbox (via its own tag scan), gated by an opt-in toggle, with a one-press backfill for already-synced libraries.

**Architecture:** A new pure-Rust `artwork` module normalizes cover art to a ≤600px baseline JPEG and embeds tags+art into a `.m4a` via `lofty`. The transcode pipeline is unified to audio-only output; the embed step (gated by `rockbox_compat`) becomes the single writer of file-embedded metadata, and the normalized art also feeds libgpod's Apple thumbnails. A backfill routine embeds tags+art into existing on-device files in place. The toggle and a backfill button are surfaced in the macOS UI over the existing daemon wire.

**Tech Stack:** Rust (`std`, `anyhow`, `tracing`, `image`, `lofty`), the vendored libgpod, existing `Config`/`DaemonSettings`/apply-loop/daemon machinery, SwiftUI (`ui/macos`).

## Global Constraints

- **New dep:** `image = "0.25"` (mainstream, actively maintained) for decode/resize/baseline-JPEG encode. `lofty = "0.22"` is already present and writes MP4 tags + art.
- **Art normalization:** `MAX_ART_EDGE = 600` (longest edge), re-encode as **baseline** JPEG (the `image` crate's `JpegEncoder` is baseline by default). The one normalized image feeds BOTH the embedded `covr` atom (Rockbox) AND libgpod's `itdb_track_set_thumbnails_from_data` (Apple).
- **Toggle:** `rockbox_compat`, **default `false`**. Lives on `DaemonSettings` (the existing UI-settable sync-prefs bag: `firstSyncMode`/`subsequentSyncMode`/`notifyOn`), so it rides the existing `SaveConfig` + `config_update` wire with no new command fields. Resolved into `Config.rockbox_compat` as: CLI `--rockbox-compat` (on-only override) OR `persisted.daemon.rockbox_compat` OR `false`.
- **Scaffolding toward always-on:** the embed logic is unconditional inside `artwork`; only the call site is gated by the bool. A later default-flip is a one-line change.
- **Non-fatal everywhere:** never fail a sync (or a backfill track) because embedding/normalization failed — `warn!` and continue. Match `ipod/db.rs`'s "add track without art rather than fail" philosophy.
- **Unify transcode to audio-only:** both backends output audio-only ALAC; the embed step is the sole writer of embedded tags/art. Apple firmware reads art from the ithmb `ArtworkDB`, never the `.m4a` atoms — unaffected.
- **Scope of embedding:** only **transcoded** (FLAC→ALAC) output. Passthrough sources (mp3/aac/alac) already carry their own tags+art and are NOT modified going-forward.
- **Backfill:** in place, **no re-transcode**; updates the iTunesDB track `size` (grew the file) via a new `OwnedDb::set_track_size`; reuses the DB-open/provision preamble + `CheckpointClock`. Per-track failures skip, never abort.
- **Wire changes are additive/optional** (serde `#[serde(default)]`, Swift `decodeIfPresent`). Document in `docs/ipc-protocol.md`; minor daemon-proto bump.
- **`tracing` only, no `println!`** outside `examples/`.
- **macOS is the build+verify target** (`cargo test -p classick`, `swift test`, on-device smoke). Windows/Linux: compile-clean.
- **Commits:** Conventional Commits; scopes `artwork`, `transcode`, `apply-loop`, `config`, `daemon`, `ipc`, `ui`, `docs`. Stage named files; never `git add -A`; never amend; never `--no-verify`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/classick/src/artwork.rs` | **New.** `normalize` (image) + `embed_track_metadata` (lofty) + `MAX_ART_EDGE`. Pure + tested. |
| `crates/classick/src/lib.rs` | Declare `pub mod artwork;`. |
| `crates/classick/Cargo.toml` | Add `image = "0.25"`. |
| `crates/classick/src/config.rs` | `Config.rockbox_compat: bool`; resolve it. |
| `crates/classick/src/config_file.rs` | `DaemonSettings.rockbox_compat: bool`. |
| `crates/classick/src/cli.rs` | `--rockbox-compat` + `--backfill-rockbox` flags. |
| `crates/classick/src/transcode.rs` | `ffmpeg_args` audio-only; refalac audio-only. |
| `crates/classick/src/apply_loop.rs` | `transcode_one` normalize+embed; new `backfill_rockbox`. |
| `crates/classick/src/ipod/db.rs` | New `OwnedDb::set_track_size`. |
| `crates/classick/src/orchestrator.rs` | Dispatch `--backfill-rockbox` → `backfill_rockbox`. |
| `crates/classick/src/ipc_daemon.rs` | `DaemonCommand::BackfillRockbox`; `DaemonSettings` already carries the toggle. |
| `crates/classick/src/daemon/runtime.rs` | Handle `BackfillRockbox`. |
| `crates/classick/src/daemon/sync_orchestrator.rs` | `build_command` passes `--rockbox-compat`; backfill variant. |
| `ui/macos/Sources/Classick/Ipc/WireModels.swift` | `DaemonSettings.rockboxCompat`; `DaemonCommand.backfillRockbox`. |
| `ui/macos/Sources/Classick/Views/SettingsView.swift` | Toggle + "Update existing library" button. |
| `ui/macos/Sources/Classick/ClassickApp.swift` | Wire toggle into `saveSettings`; `onBackfill`. |
| `docs/ipc-protocol.md`, `LEARNINGS.md` | Document the toggle, backfill command, coexistence guidance. |

---

## Task 1: `artwork` module — normalize + embed

**Files:**
- Create: `crates/classick/src/artwork.rs`
- Modify: `crates/classick/src/lib.rs` (add `pub mod artwork;` next to the other `pub mod` lines)
- Modify: `crates/classick/Cargo.toml` (add `image = "0.25"`)
- Test: in `artwork.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `pub const MAX_ART_EDGE: u32 = 600;`
- Produces: `pub fn normalize(source_art: &[u8]) -> anyhow::Result<Vec<u8>>` — decode any common format, downscale so the longest edge ≤ `MAX_ART_EDGE`, re-encode baseline JPEG.
- Produces: `pub fn embed_track_metadata(m4a: &std::path::Path, tags: &crate::ipod::db::Tags, art: Option<&[u8]>) -> anyhow::Result<()>` — write MP4 `ilst` tags + optional `covr`, overwriting existing (idempotent).

**Implementer notes:**
- For `lofty` 0.22 API usage, mirror the existing lofty call sites in this repo (`crates/classick/src/transcode.rs` macOS `macos_probe` / `extract_cover_art_via_lofty`) for the version-correct types. If unsure of the exact 0.22 tag/picture API, confirm via context7 docs for `lofty`. **The tests are the source of truth** — make them pass.
- `image` `JpegEncoder` emits baseline JPEG by default (no progressive). Use `DynamicImage::resize(w, h, FilterType::Lanczos3)` which fits within a box preserving aspect ratio; only downscale (never upscale) — if both dims ≤ MAX_ART_EDGE, encode as-is.

- [ ] **Step 1: Add the dependency.** In `crates/classick/Cargo.toml` `[dependencies]`, add:

```toml
image = "0.25"
```

Run: `cargo fetch -p classick` (or `cargo build -p classick`) to confirm it resolves.

- [ ] **Step 2: Write the failing tests** — create `crates/classick/src/artwork.rs`:

```rust
//! Normalize cover art to a small baseline JPEG and embed MP4 tags + art into
//! an `.m4a`, so a transcoded track is self-describing for Rockbox (which reads
//! tags/art from the file) while Apple firmware keeps reading the iTunesDB +
//! ithmb ArtworkDB. See
//! docs/superpowers/specs/2026-07-13-rockbox-compatibility-design.md.

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
    let img = image::load_from_memory(source_art)
        .context("decoding source cover art")?;
    let (w, h) = (img.width(), img.height());
    let scaled = if w > MAX_ART_EDGE || h > MAX_ART_EDGE {
        img.resize(MAX_ART_EDGE, MAX_ART_EDGE, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    // Encode baseline JPEG (image's JpegEncoder is baseline). RGB8 drops any
    // alpha, which JPEG cannot represent anyway.
    let rgb = scaled.to_rgb8();
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 85);
    enc.encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
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
    let tag = file.primary_tag_mut().expect("primary tag present after insert");

    if let Some(v) = &tags.title { tag.set_title(v.clone()); }
    if let Some(v) = &tags.artist { tag.set_artist(v.clone()); }
    if let Some(v) = &tags.album { tag.set_album(v.clone()); }
    if let Some(v) = &tags.genre { tag.set_genre(v.clone()); }
    if let Some(v) = &tags.album_artist {
        tag.insert_text(ItemKey::AlbumArtist, v.clone());
    }
    if let Some(v) = tags.year { tag.set_year(v as u32); }
    if let Some(v) = tags.track_nr { tag.set_track(v as u32); }
    if let Some(v) = tags.tracks { tag.set_track_total(v as u32); }
    if let Some(v) = tags.disc_nr { tag.set_disk(v as u32); }
    if let Some(v) = tags.discs { tag.set_disk_total(v as u32); }

    if let Some(bytes) = art {
        use lofty::picture::{MimeType, Picture, PictureType};
        // Replace any existing pictures with our normalized JPEG cover.
        while tag.picture_count() > 0 { tag.remove_picture(0); }
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
        let img = image::RgbImage::from_fn(w, h, |x, _| {
            image::Rgb([(x % 256) as u8, 100, 150])
        });
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
        assert!(!out.windows(2).any(|w| w == [0xFF, 0xC2]), "must be baseline JPEG");
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
        let tmp = std::env::temp_dir().join(format!("classick-embed-noart-{}.m4a", std::process::id()));
        std::fs::copy(fixture, &tmp).unwrap();
        embed_track_metadata(&tmp, &tags_fixture(), None).unwrap();
        let f = lofty::read_from_path(&tmp).unwrap();
        assert_eq!(f.primary_tag().unwrap().title().as_deref(), Some("Wake Me Up Tomorrow"));
        let _ = std::fs::remove_file(&tmp);
    }
}
```

- [ ] **Step 3: Provide the `bare.m4a` fixture.** The embed tests need a small tagless ALAC file. Create it from an existing test FLAC fixture (find one under `crates/classick/tests/fixtures/`):

```bash
# From repo root. The repo ships tests/fixtures/tagged.flac (used by transcode.rs
# tests). afconvert's CAF round-trip strips tags/art, yielding a tagless .m4a:
/usr/bin/afconvert -f caff -d LEI16@44100 crates/classick/tests/fixtures/tagged.flac /tmp/bare.caf
/usr/bin/afconvert -f m4af -d alac /tmp/bare.caf crates/classick/tests/fixtures/bare.m4a
rm -f /tmp/bare.caf
ls -la crates/classick/tests/fixtures/bare.m4a   # exists, non-empty
```

- [ ] **Step 4: Declare the module + run tests.**

Add `pub mod artwork;` to `crates/classick/src/lib.rs` (alongside the other `pub mod` declarations).

Run: `cargo test -p classick artwork`
Expected: PASS (4 tests). If a lofty API call doesn't compile against 0.22, adjust per the repo's existing lofty usage until the round-trip tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/artwork.rs crates/classick/src/lib.rs crates/classick/Cargo.toml crates/classick/Cargo.lock crates/classick/tests/fixtures/bare.m4a
git commit -m "feat(artwork): normalize cover art + embed MP4 tags/art via lofty"
```

---

## Task 2: Config — `rockbox_compat` toggle + CLI flags

**Files:**
- Modify: `crates/classick/src/config_file.rs` (`DaemonSettings`)
- Modify: `crates/classick/src/config.rs` (`Config` + resolve)
- Modify: `crates/classick/src/cli.rs` (`--rockbox-compat`, `--backfill-rockbox`)
- Test: in `config.rs` `#[cfg(test)]` (mirror the existing encoder-resolution tests)

**Interfaces:**
- Produces: `Config.rockbox_compat: bool`, `Config.backfill_rockbox: bool`, `Cli.rockbox_compat: bool`, `Cli.backfill_rockbox: bool`, `DaemonSettings.rockbox_compat: bool`.

- [ ] **Step 1: Add the field to `DaemonSettings`.** In `crates/classick/src/config_file.rs`, add to the `DaemonSettings` struct (with a serde default so old configs/UIs deserialize):

```rust
    /// When true, transcoded .m4a files are made self-describing (embedded
    /// tags + normalized cover art) so Rockbox can read the library. Default
    /// false. See the Rockbox-compatibility design.
    #[serde(default)]
    pub rockbox_compat: bool,
```

(Add `rockbox_compat: false` to `DaemonSettings`'s `Default` impl / `default_daemon_settings` if it constructs fields explicitly.)

- [ ] **Step 2: Add CLI flags.** In `crates/classick/src/cli.rs`, add to the `Cli` struct near the encoder flags:

```rust
    /// Make transcoded .m4a files self-describing (embed tags + cover art) so
    /// an iPod running Rockbox can read the library. Persist with --save-config.
    #[arg(long)]
    pub rockbox_compat: bool,

    /// Embed tags + cover art into the EXISTING on-iPod .m4a files in place
    /// (no re-transcode), then exit. Makes an already-synced library
    /// Rockbox-readable. Requires --ipod (or auto-detect).
    #[arg(long)]
    pub backfill_rockbox: bool,
```

- [ ] **Step 3: Write the failing resolution tests.** In `crates/classick/src/config.rs` `#[cfg(test)]`, add (mirroring `persisted_encoder_used_when_no_cli_flag` / `encoder_falls_back_to_default_when_neither_set`):

```rust
    #[test]
    fn rockbox_compat_defaults_false() {
        let cfg = resolve(minimal_cli(), None).unwrap();
        assert!(!cfg.rockbox_compat);
    }

    #[test]
    fn rockbox_compat_from_persisted_daemon_settings() {
        let mut persisted = PersistedConfig::default();
        persisted.daemon = Some(DaemonSettings { rockbox_compat: true, ..Default::default() });
        let cfg = resolve(minimal_cli(), Some(persisted)).unwrap();
        assert!(cfg.rockbox_compat);
    }

    #[test]
    fn rockbox_compat_cli_flag_overrides_off_persisted() {
        let mut cli = minimal_cli();
        cli.rockbox_compat = true;
        let cfg = resolve(cli, None).unwrap();
        assert!(cfg.rockbox_compat);
    }
```

(Use whatever the existing tests use to build a minimal `Cli` + `resolve` signature — match `cli_encoder_wins_over_persisted_encoder`'s construction exactly, including how `PersistedConfig`/`DaemonSettings` are imported.)

- [ ] **Step 4: Run to verify it fails**

Run: `cargo test -p classick rockbox_compat`
Expected: FAIL — `Config.rockbox_compat` / `Cli.rockbox_compat` not found.

- [ ] **Step 5: Add `Config` fields + resolution.** In `crates/classick/src/config.rs`, add to `Config`:

```rust
    pub rockbox_compat: bool,
    pub backfill_rockbox: bool,
```

In `resolve(...)`, compute (near the `encoder` resolution):

```rust
    // Rockbox-compat: CLI flag (on-only) OR persisted daemon setting OR false.
    let rockbox_compat = cli.rockbox_compat
        || persisted
            .as_ref()
            .and_then(|p| p.daemon.as_ref())
            .map(|d| d.rockbox_compat)
            .unwrap_or(false);
```

and add `rockbox_compat,` + `backfill_rockbox: cli.backfill_rockbox,` to the `Config { ... }` construction. (If `Config::to_persisted` builds `daemon`, ensure `rockbox_compat` is carried through; otherwise the daemon setting is the store of record.)

- [ ] **Step 6: Run to verify it passes**

Run: `cargo test -p classick` (config + build)
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/classick/src/config.rs crates/classick/src/config_file.rs crates/classick/src/cli.rs
git commit -m "feat(config): rockbox_compat toggle + --rockbox-compat/--backfill-rockbox flags"
```

---

## Task 3: Unify transcode to audio-only

**Files:**
- Modify: `crates/classick/src/transcode.rs` (`ffmpeg_args`, refalac path)
- Modify: `crates/classick/src/apply_loop.rs` (remove the refalac-only art extraction in `transcode_one`, since art embedding is centralized in Task 4)
- Test: `transcode.rs` `#[cfg(test)]` (`ffmpeg_args` assertions)

**Interfaces:** unchanged signatures; `ffmpeg_args` now returns audio-only args.

- [ ] **Step 1: Update the `ffmpeg_args` test.** In `crates/classick/src/transcode.rs` `#[cfg(test)]`, replace the video-mapping assertions with audio-only ones:

```rust
        let joined = args.join(" ");
        assert!(joined.contains("-map 0:a"));
        assert!(!joined.contains("0:v"), "unified pipeline: transcode is audio-only");
        assert!(!joined.contains("attached_pic"));
        assert!(joined.contains("-c:a alac"));
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p classick ffmpeg_args`
Expected: FAIL — current args still contain `0:v` / `attached_pic`.

- [ ] **Step 3: Make `ffmpeg_args` audio-only.** In `crates/classick/src/transcode.rs`, edit `ffmpeg_args` to drop the video mapping — the art is now owned by `artwork::embed_track_metadata`:

```rust
pub fn ffmpeg_args(src: &Path, dst: &Path) -> Vec<String> {
    vec![
        "-nostdin".into(),
        "-loglevel".into(), "error".into(),
        "-y".into(),
        "-i".into(), src.to_string_lossy().into_owned(),
        "-map".into(), "0:a".into(),
        "-c:a".into(), "alac".into(),
        "-vn".into(),  // audio-only: embedded art is written later by artwork::embed
        "-f".into(), "ipod".into(),
        dst.to_string_lossy().into_owned(),
    ]
}
```

- [ ] **Step 4: Make the refalac path audio-only.** In `crates/classick/src/apply_loop.rs` `transcode_one`, the `EncoderChoice::Refalac` arm currently extracts art and passes it to `transcode_via_refalac(..., art_path_opt)`. Since art is now centralized, pass `None` for the artwork and drop the extraction block:

```rust
            EncoderChoice::Refalac => {
                let dst = transcode::temp_alac_path();
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                let ffmpeg_path = config.ffmpeg.as_path();
                transcode::transcode_via_refalac(
                    &src.path,
                    &dst,
                    &config.refalac_path,
                    ffmpeg_path,
                    None, // audio-only: art embedded later by artwork::embed
                )
                .with_context(|| format!("refalac transcode {}", src.path.display()))?;
                let ver = refalac_version
                    .clone()
                    .unwrap_or_else(|| "refalac (version unknown)".to_string());
                ("refalac".to_string(), ver, dst)
            }
```

(Removes the `art_path_opt` extraction + cleanup for the refalac arm. `transcode_via_refalac`'s `art: Option<&Path>` parameter stays; it just receives `None` now.)

- [ ] **Step 5: Run to verify it passes + build**

Run: `cargo test -p classick && cargo build -p classick`
Expected: PASS; clean build (macOS afconvert path already audio-only, unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/classick/src/transcode.rs crates/classick/src/apply_loop.rs
git commit -m "refactor(transcode): unify to audio-only output; art owned by embed step"
```

---

## Task 4: Wire embed into `transcode_one` (going-forward path)

**Files:**
- Modify: `crates/classick/src/apply_loop.rs` (`transcode_one` art block)
- Test: build + existing suite (behavior verified end-to-end in Task 8's on-device smoke; the unit-level embed/normalize is covered by Task 1)

**Interfaces:**
- Consumes: `crate::artwork::{normalize, embed_track_metadata}`, `config.rockbox_compat`.

- [ ] **Step 1: Normalize art + embed in `transcode_one`.** In `crates/classick/src/apply_loop.rs`, capture whether this was a transcode before the encoder match:

```rust
    let is_transcode = matches!(action, SourceAction::Transcode);
```

Then replace the existing `let art = if has_embedded_art(&probe) { ... }` block with normalize-then-optionally-embed:

```rust
    // Extract source art once; normalize to a small baseline JPEG that feeds
    // BOTH libgpod's Apple thumbnails AND (when rockbox_compat) the embedded
    // covr atom. Non-fatal: on any art failure, fall back to no art.
    let art: Option<Vec<u8>> = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        let raw = transcode::extract_cover_art(&src.path, &art_path, &config.ffmpeg)
            .and_then(|()| std::fs::read(&art_path).map_err(Into::into));
        let _ = std::fs::remove_file(&art_path);
        match raw {
            Ok(bytes) => match crate::artwork::normalize(&bytes) {
                Ok(norm) => Some(norm),
                Err(e) => {
                    tracing::warn!("art normalize failed for {}: {e:#}; using raw bytes", src.path.display());
                    Some(bytes)
                }
            },
            Err(e) => {
                tracing::warn!("art extract failed for {}: {e:#}", src.path.display());
                None
            }
        }
    } else {
        None
    };

    // Rockbox: make the transcoded .m4a self-describing (tags + art). Only for
    // transcoded output — passthrough files keep their own metadata. Non-fatal.
    if config.rockbox_compat && is_transcode {
        if let Err(e) = crate::artwork::embed_track_metadata(&temp, &tags, art.as_deref()) {
            tracing::warn!("rockbox embed failed for {}: {e:#}", src.path.display());
        }
    }
```

(`temp`, `tags`, `action` are all already in scope in `transcode_one`. `art` continues to flow into `Transcoded { art, .. }` → `commit_transcoded` → libgpod, now normalized.)

- [ ] **Step 2: Build + test**

Run: `cargo build -p classick && cargo test -p classick`
Expected: clean build; all tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/classick/src/apply_loop.rs
git commit -m "feat(apply-loop): embed tags+normalized art into transcoded m4a when rockbox_compat"
```

---

## Task 5: Backfill core — `backfill_rockbox` + `set_track_size` + dispatch

**Files:**
- Modify: `crates/classick/src/ipod/db.rs` (new `set_track_size`)
- Modify: `crates/classick/src/apply_loop.rs` (new `backfill_rockbox`)
- Modify: `crates/classick/src/orchestrator.rs` (dispatch `--backfill-rockbox`)
- Test: `db.rs` (size setter unit test if feasible with the daemon test-DB helper) + a `backfill_rockbox` integration test over a fake mount/manifest

**Interfaces:**
- Produces: `OwnedDb::set_track_size(&self, dbid: u64, size: u32) -> anyhow::Result<()>`
- Produces: `pub fn backfill_rockbox(config: &mut Config, progress: &Progress, decision_rx: &Receiver<Decision>) -> anyhow::Result<RunOutcome>` — same param shape as `apply_loop::run` so it reuses `preflight::resolve_ipod_mount` and the DB preamble.

- [ ] **Step 1: Add `set_track_size`.** In `crates/classick/src/ipod/db.rs`, model it on `update_track_metadata` (which uses `find_track_by_dbid`):

```rust
    /// Update a track's stored file `size` (bytes) by dbid. Used by the Rockbox
    /// backfill after embedding tags/art grows an on-device file, so the
    /// iTunesDB stays consistent. Idempotent; does NOT call `itdb_write`.
    pub fn set_track_size(&self, dbid: u64, size: u32) -> Result<()> {
        unsafe {
            let found = self.find_track_by_dbid(dbid);
            if found.is_null() {
                return Ok(()); // idempotent: track not present
            }
            (*found).size = size as _;
        }
        Ok(())
    }
```

(Confirm the `Itdb_Track` `size` field name/type via `crates/classick/src/ffi.rs`; cast to its type with `as _`.)

- [ ] **Step 2: Write the failing backfill test.** In `crates/classick/src/apply_loop.rs` `#[cfg(test)]` (or a new test module), add a test that a backfill over a fake mount + manifest embeds tags/art into the target file. Since `backfill_rockbox` opens a real libgpod DB, keep the *unit* test focused on the file-embed loop by extracting the per-track work into a helper you can test without a DB:

```rust
    #[test]
    fn backfill_embeds_into_existing_device_file() {
        // Copy the bare fixture as a stand-in on-device .m4a.
        let dir = std::env::temp_dir().join(format!("classick-backfill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dev = dir.join("track.m4a");
        std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bare.m4a"), &dev).unwrap();

        // The per-track backfill step: probe source tags → normalize art → embed.
        let src = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tagged.flac")); // existing fixture: tags + embedded PNG art
        let before = std::fs::metadata(&dev).unwrap().len();
        backfill_one_file(&dev, src, &std::path::PathBuf::from("ffmpeg")).unwrap();
        let after = std::fs::metadata(&dev).unwrap().len();
        assert!(after >= before, "embedding should not shrink the file");

        use lofty::file::TaggedFileExt;
        use lofty::prelude::*;
        let tag = lofty::read_from_path(&dev).unwrap();
        assert!(tag.primary_tag().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p classick backfill`
Expected: FAIL — `backfill_one_file` not defined.

- [ ] **Step 4: Implement `backfill_one_file` + `backfill_rockbox`.** In `crates/classick/src/apply_loop.rs`:

```rust
/// Embed tags + normalized art from `source` into the on-device `.m4a` at
/// `device_file`, in place (no re-transcode). Returns the new file size.
/// Non-fatal caller decides skip vs abort. Public(crate) for unit tests.
pub(crate) fn backfill_one_file(
    device_file: &Path,
    source: &Path,
    ffmpeg: &Path,
) -> Result<u64> {
    let probe = transcode::probe(source, ffmpeg)
        .with_context(|| format!("probe {}", source.display()))?;
    let tags = tags_from_probe(&probe);
    let art: Option<Vec<u8>> = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        let raw = transcode::extract_cover_art(source, &art_path, ffmpeg)
            .and_then(|()| std::fs::read(&art_path).map_err(Into::into));
        let _ = std::fs::remove_file(&art_path);
        raw.ok().and_then(|b| crate::artwork::normalize(&b).ok().or(Some(b)))
    } else {
        None
    };
    crate::artwork::embed_track_metadata(device_file, &tags, art.as_deref())
        .with_context(|| format!("embed into {}", device_file.display()))?;
    Ok(std::fs::metadata(device_file)?.len())
}

/// Backfill the existing on-device library: for each manifest entry with a
/// source file + on-device .m4a, embed tags+art in place and update the
/// iTunesDB size. No re-transcode. Per-track failures skip (warn), never abort.
pub fn backfill_rockbox(
    config: &mut Config,
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<RunOutcome> {
    // Same preflight + DB preamble as run() (copy verbatim from run()):
    let mount = preflight::resolve_ipod_mount(config, progress, decision_rx)?;
    let identity = device::resolve_libgpod_identity(Path::new(&mount))?;
    progress.log(format!(
        "iPod identity: FirewireGuid={}, ModelNumStr={}",
        identity.firewire_guid, identity.model_num_str,
    ));
    if let Err(e) = crate::ipod::sysinfo_provision::provision(Path::new(&mount), &identity) {
        progress.log(format!("SysInfoExtended provisioning failed: {e:#}"));
    }
    let db = OwnedDb::open(Path::new(&mount))?;
    unsafe {
        let device_ptr = (*db.as_ptr()).device;
        device::set_firewire_guid(device_ptr, &identity.firewire_guid)?;
        device::set_model_num(device_ptr, &identity.model_num_str)?;
    }

    // Load the manifest the same way run() does (match run()'s manifest load
    // call — likely `manifest::load(&config.manifest_path)` or `Manifest::load`).
    let manifest = manifest::load(&config.manifest_path)?;
    // Construct CheckpointClock with the SAME args run() uses (see run() ~L387).
    let mut ckpt = CheckpointClock::new(/* same args as run() */);
    let (mut updated, mut skipped) = (0usize, 0usize);
    for entry in manifest.tracks.iter().filter(|e| e.source_known && !e.ipod_relpath.is_empty()) {
        let device_file = Path::new(&mount)
            .join(entry.ipod_relpath.replace('\\', std::path::MAIN_SEPARATOR_STR));
        if !device_file.exists() || !entry.source_path.exists() {
            skipped += 1;
            continue;
        }
        match backfill_one_file(&device_file, &entry.source_path, &config.ffmpeg) {
            Ok(size) => {
                db.set_track_size(entry.ipod_dbid, size.min(u32::MAX as u64) as u32).ok();
                updated += 1;
            }
            Err(e) => {
                tracing::warn!("backfill skip {}: {e:#}", entry.source_path.display());
                skipped += 1;
            }
        }
        progress.log(format!("backfill: {updated} updated, {skipped} skipped"));
        // Flush per run()'s checkpoint cadence (match run()'s should_checkpoint call).
        if ckpt.should_checkpoint(updated) { db.write()?; }
    }
    db.write()?;
    progress.log(format!("Rockbox backfill complete: {updated} updated, {skipped} skipped"));
    Ok(RunOutcome::Completed)
}
```

**Implementer note:** the preflight + DB preamble above is copied from `run()` (lines ~116, ~312–334). The only lines that need confirming against `run()` are the manifest-load call and the `CheckpointClock::new(...)` args (run() ~L387) — use exactly what `run()` uses. Do not invent new APIs.

- [ ] **Step 5: Dispatch from the orchestrator.** In `crates/classick/src/orchestrator.rs`, before the normal `apply_loop::run`, branch on the flag:

```rust
    if config.backfill_rockbox {
        return apply_loop::backfill_rockbox(&mut config, progress, decision_rx);
    }
    apply_loop::run(&mut config, progress, decision_rx)
```

- [ ] **Step 6: Run tests + build**

Run: `cargo test -p classick && cargo build -p classick`
Expected: PASS; clean build.

- [ ] **Step 7: Commit**

```bash
git add crates/classick/src/ipod/db.rs crates/classick/src/apply_loop.rs crates/classick/src/orchestrator.rs
git commit -m "feat(apply-loop): Rockbox backfill — in-place embed of existing library + DB size"
```

---

## Task 6: Daemon wire — `BackfillRockbox` command + `--rockbox-compat` spawn

**Files:**
- Modify: `crates/classick/src/ipc_daemon.rs` (`DaemonCommand::BackfillRockbox`)
- Modify: `crates/classick/src/daemon/sync_orchestrator.rs` (`build_command`)
- Modify: `crates/classick/src/daemon/runtime.rs` (handle `BackfillRockbox`)
- Modify: `docs/ipc-protocol.md`
- Test: `ipc_daemon.rs` (command deserializes) + `sync_orchestrator.rs` (`build_command` flag test)

**Interfaces:**
- Produces: `DaemonCommand::BackfillRockbox` wire variant (`{"type":"backfill_rockbox"}`).

- [ ] **Step 1: Add the command + test.** In `crates/classick/src/ipc_daemon.rs`, add to `DaemonCommand`:

```rust
    /// Embed tags + cover art into the existing on-iPod library in place so
    /// Rockbox can read it. Spawns a `--backfill-rockbox` subprocess; reports
    /// sync-style progress. No-op if a sync is already running.
    BackfillRockbox,
```

Add a deserialization test near `save_config_with_partial_payload_deserializes`:

```rust
    #[test]
    fn backfill_rockbox_deserializes() {
        let json = r#"{"type":"backfill_rockbox"}"#;
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(json).unwrap(),
            DaemonCommand::BackfillRockbox
        ));
    }
```

- [ ] **Step 2: Parameterize `build_command`.** In `crates/classick/src/daemon/sync_orchestrator.rs`, extend `build_command` to append `--rockbox-compat` when the setting is on, and add a backfill variant. Update the signature to accept the flag (thread `persisted.daemon.rockbox_compat` from the caller):

```rust
pub fn build_command(exe: &std::path::Path, drive: &str, rockbox_compat: bool) -> Command {
    // ... existing setup ...
    cmd.arg("--ipc-mode").arg("--apply").arg("--ipod").arg(drive);
    if rockbox_compat {
        cmd.arg("--rockbox-compat");
    }
    // ... stdin/stdout/no_console ...
    cmd
}

pub fn build_backfill_command(exe: &std::path::Path, drive: &str) -> Command {
    // Same as build_command but --backfill-rockbox instead of --apply.
    // (Factor the shared stdio/no_console setup into a helper if convenient.)
}
```

Update the existing `build_command_passes_apply_and_ipod_flags` test call to pass `false`, and add:

```rust
    #[test]
    fn build_command_adds_rockbox_flag_when_enabled() {
        let cmd = build_command(&PathBuf::from("classick.exe"), "G:\\", true);
        assert!(format!("{cmd:?}").contains("--rockbox-compat"));
    }
```

Update all existing `build_command(...)` call sites to pass the resolved `rockbox_compat` bool.

- [ ] **Step 3: Handle `BackfillRockbox` in the runtime.** In `crates/classick/src/daemon/runtime.rs`, add a match arm alongside `TriggerSync` that launches a backfill subprocess through the same orchestrator machinery (mirror how `TriggerSync` spawns + relays events, but with `build_backfill_command`). Reuse the running-sync guard so a backfill and a sync can't overlap.

**Implementer note:** read the `DaemonCommand::TriggerSync` arm (`runtime.rs:940`) and the sync-orchestrator spawn path it calls; replicate it for backfill with the backfill command builder. Keep the event vocabulary identical (`summary`/`track_done`/`finish`/`error`).

- [ ] **Step 4: Document the wire.** In `docs/ipc-protocol.md`, add `backfill_rockbox` to the daemon command list, note the additive `DaemonSettings.rockbox_compat` field, and bump the daemon-proto minor version per the doc's conventions.

- [ ] **Step 5: Build + test**

Run: `cargo test -p classick && cargo build -p classick`
Expected: PASS; clean build.

- [ ] **Step 6: Commit**

```bash
git add crates/classick/src/ipc_daemon.rs crates/classick/src/daemon/sync_orchestrator.rs crates/classick/src/daemon/runtime.rs docs/ipc-protocol.md
git commit -m "feat(daemon): BackfillRockbox command + pass --rockbox-compat to sync subprocess"
```

---

## Task 7: macOS UI — toggle + backfill button

**Files:**
- Modify: `ui/macos/Sources/Classick/Ipc/WireModels.swift` (`DaemonSettings.rockboxCompat`; `DaemonCommand.backfillRockbox`)
- Modify: `ui/macos/Sources/Classick/Views/SettingsView.swift` (toggle + button)
- Modify: `ui/macos/Sources/Classick/ClassickApp.swift` (thread toggle into `saveSettings`; add `onBackfill`)
- Test: `ui/macos/Tests/ClassickTests/WireCodecTests.swift` (encode `backfillRockbox`; `DaemonSettings` round-trip with `rockboxCompat`)

**Interfaces (mirror the Rust wire):**
- `DaemonSettings.rockboxCompat: Bool`
- `DaemonCommand.backfillRockbox` → `{"type":"backfill_rockbox"}`

- [ ] **Step 1: Add the wire fields + test.** In `WireModels.swift`, add `rockboxCompat` to the Swift `DaemonSettings` (with a `CodingKeys` entry `case rockboxCompat = "rockbox_compat"` and a `decodeIfPresent`-with-default-false in its decoder, matching the struct's existing pattern). Add the command case:

```swift
    case backfillRockbox
```

and in `encode(to:)`:

```swift
        case .backfillRockbox:
            try container.encode("backfill_rockbox", forKey: .type)
```

Add to `WireCodecTests.swift`:

```swift
    func testBackfillRockboxEncodes() throws {
        let data = try JSONEncoder().encode(DaemonCommand.backfillRockbox)
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        XCTAssertEqual(obj["type"] as? String, "backfill_rockbox")
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd ui/macos && swift test --filter WireCodec`
Expected: FAIL — `rockboxCompat` / `backfillRockbox` not defined.

- [ ] **Step 3: Add the toggle + button to Settings.** In `SettingsView.swift` `GeneralTab`, add `@State private var rockboxCompat = false`, populate it in `syncFromConfig` (`rockboxCompat = daemon.rockboxCompat`), include it in `scheduleSave`'s `DaemonSettings(...)` construction, and add the controls to the `Form`:

```swift
            Toggle(
                "Rockbox compatibility (embed tags & art in files)",
                isOn: Binding(
                    get: { rockboxCompat },
                    set: { rockboxCompat = $0; scheduleSave() }
                ))
            Text("Lets an iPod running Rockbox read your library. Applies to newly synced tracks; use the button below to convert what's already on the iPod. Keep any files you copy to the iPod yourself outside iPod_Control.")
                .font(.footnote)
                .foregroundStyle(.secondary)

            Button("Update existing library for Rockbox") { onBackfill() }
```

Add `var onBackfill: () -> Void` to `GeneralTab` (and `SettingsView`), threading it from the app.

- [ ] **Step 4: Wire `onBackfill` + toggle in the app.** In `ClassickApp.swift`, add an `onBackfill` closure that sends the command, and pass it into `SettingsView`:

```swift
    func backfillRockbox() {
        Task { await daemonClient.send(.backfillRockbox) }
    }
```

```swift
            SettingsView(
                model: appModel,
                onSave: appDelegate.saveSettings,
                onForgetIpod: appDelegate.forgetIpod,
                onBackfill: appDelegate.backfillRockbox
            )
```

(`saveSettings` already forwards the `DaemonSettings` it's given, so once `GeneralTab` includes `rockboxCompat` in the `DaemonSettings` it builds, the toggle persists with no other change.)

- [ ] **Step 5: Run tests + build**

Run: `cd ui/macos && swift test`
Expected: PASS.
Run: `cd ui/macos && swift build`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add ui/macos/Sources/Classick/Ipc/WireModels.swift ui/macos/Sources/Classick/Views/SettingsView.swift ui/macos/Sources/Classick/ClassickApp.swift ui/macos/Tests/ClassickTests/WireCodecTests.swift
git commit -m "feat(ui): Rockbox compatibility toggle + Update existing library button"
```

---

## Task 8: Docs + final verification (build, tests, on-device smoke)

**Files:**
- Modify: `LEARNINGS.md`
- Verification only otherwise

- [ ] **Step 1: Add a `LEARNINGS.md` bullet** (check for duplicates first):

```markdown
- Rockbox reads track tags + cover art from the FILE, not the iTunesDB, and
  ignores embedded art in FLAC/Vorbis. classick's afconvert path produced bare
  .m4a (no tags/art). The `rockbox_compat` toggle embeds MP4 tags + a
  normalized (≤600px baseline JPEG) covr atom into transcoded output
  (`artwork.rs`), and the "Update existing library" backfill embeds them into
  already-synced files in place. One normalized image feeds both the covr atom
  (Rockbox) and libgpod's ithmb thumbnails (Apple). Keep hand-copied Rockbox
  files OUTSIDE iPod_Control/ — reconcile deletes non-DB files under
  iPod_Control/Music as orphans.
```

- [ ] **Step 2: Full build + tests**

Run: `cargo build --release -p classick && cargo test -p classick`
Expected: clean; all pass.
Run: `cd ui/macos && swift test && swift build`
Expected: clean; all pass.

- [ ] **Step 3: On-device smoke — toggle (REQUIRED before merge).** With the iPod mounted:

```bash
# Enable via the UI toggle (or --rockbox-compat), wipe, sync one album:
cargo run --release --example wipe-tracks -- /Volumes/IPOD
rm -f "$HOME/Library/Application Support/classick/manifest.json"
./target/release/classick --apply --rockbox-compat \
  --source "/Volumes/data/media/music/Luttrell/Intergalactic Plastic EP" --ipod /Volumes/IPOD
# Verify embedded tags+art in an on-device .m4a:
ls /Volumes/IPOD/iPod_Control/Music/F*/  # note a file, then:
#   (use ffprobe/mp4 tooling to confirm title/artist/album + a covr atom present)
```

Then **boot Rockbox → Settings → Database → "Initialize now" → confirm tracks show correct title/artist/album AND cover art on the Rockbox screen.** Reboot Apple firmware → confirm cover art still displays + tracks play (safety constraints 1–2: the art pipeline now feeds libgpod normalized bytes).

- [ ] **Step 4: On-device smoke — backfill (REQUIRED before merge).**

```bash
# Fresh library synced with the toggle OFF (bare files):
cargo run --release --example wipe-tracks -- /Volumes/IPOD
rm -f "$HOME/Library/Application Support/classick/manifest.json"
./target/release/classick --apply \
  --source "/Volumes/data/media/music/Luttrell/Intergalactic Plastic EP" --ipod /Volumes/IPOD
# Now backfill in place (or press the UI "Update existing library" button):
./target/release/classick --backfill-rockbox --ipod /Volumes/IPOD
```

Boot Rockbox → "Initialize now" → confirm the previously-bare tracks now show tags + art. Reboot Apple firmware → confirm playback still works.

Expected: both smokes pass — Rockbox shows tags + art, Apple firmware unregressed.

- [ ] **Step 5: Commit docs**

```bash
git add LEARNINGS.md
git commit -m "docs: record Rockbox compatibility (embedded tags/art) learnings"
```

---

## Self-review notes

- **Spec coverage:** artwork normalize+embed (Task 1), toggle default-off in DaemonSettings + CLI (Task 2), unify audio-only (Task 3), going-forward embed gated + only transcoded output + normalized art feeds both consumers (Task 4), backfill in-place + DB size (Task 5), daemon command + `--rockbox-compat` spawn + ipc-protocol doc (Task 6), macOS toggle + button (Task 7), coexistence guidance + on-device smoke for both paths incl. Apple-regression check (Tasks 7–8). All covered.
- **Type consistency:** `normalize(&[u8]) -> Result<Vec<u8>>`, `embed_track_metadata(&Path, &Tags, Option<&[u8]>) -> Result<()>`, `set_track_size(u64, u32) -> Result<()>`, `Config.rockbox_compat/backfill_rockbox: bool`, `DaemonSettings.rockbox_compat: bool`, `DaemonCommand::BackfillRockbox`, Swift `rockboxCompat`/`backfillRockbox` used consistently across tasks.
- **Known adaptation points (flagged in-task, not placeholders):** lofty 0.22 exact API (Task 1 — follow repo's existing lofty usage, tests are truth); `Itdb_Track.size` field type (Task 5 — confirm in `ffi.rs`); `backfill_rockbox` must reuse `apply_loop::run`'s real mount/DB-open/RunOutcome/Progress/CheckpointClock APIs (Task 5); daemon `TriggerSync` spawn pattern to mirror for backfill (Task 6). Each names the exact existing code to copy from.
- **Non-fatal contract:** every embed/normalize/backfill-track failure warns + continues; never aborts a sync.
