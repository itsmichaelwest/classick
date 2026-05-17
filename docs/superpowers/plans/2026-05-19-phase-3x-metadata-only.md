# Phase 3.x: Metadata-Only Smart-Update — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Detect "source FLAC's tags or embedded art changed but the audio samples are bit-identical" via a separate audio-only fingerprint, then perform a lightweight in-place tag + thumbnail update on the iPod without re-transcoding or re-copying the audio file.

**Architecture:** Adds an `audio_fingerprint: String` field to `ManifestEntry` (BLAKE3 of just the FLAC audio frames, computed by skipping past METADATA_BLOCKs using a tiny hand-rolled parser — no new crate dependencies). Adds a new `Action::MetadataOnly` variant the diff emits when the file fingerprint differs but the audio fingerprint matches the manifest's stored value. Adds a new `OwnedDb::update_track_metadata` method that finds an existing iPod track by dbid and overwrites its tag fields + thumbnails via libgpod, then leaves `itdb_write` to be called once at the end as today. Bootstrap is intentional: Phase 2 manifest entries have no audio_fingerprint, so the first sync after upgrade re-Modifies anything that changed (and populates audio_fingerprint along the way); subsequent edits flow through the fast MetadataOnly path.

**Tech Stack:** Rust stable (`x86_64-pc-windows-msvc`), existing `blake3` crate from Phase 2 (no new deps). Hand-rolled FLAC metadata-block skipper (~30 lines, format is trivially documented at https://xiph.org/flac/format.html). Existing libgpod FFI surface — `itdb_track_set_thumbnails_from_data` is already in bindgen output from Phase 1.

**Plan scope:** Phase 3.x only. Format pass-through (Phase 3) and refalac encoder (Phase 3) are deferred to their own plan. Multi-iPod (Phase 4), daemon (Phase 5), GUI (Phase 6) all later.

**Gate:** end-to-end live verification — sync a FLAC, externally edit its tags (via Picard or `metaflac --set-tag`), re-sync, observe `MetadataOnly` action fire with sub-second per-track time (vs ~3-5s for transcode), confirm the iPod's Now Playing displays the new tags + art after eject + plug-back-in.

---

## File Structure

```
F:\repos\ipod-sync\
├── src\
│   ├── source.rs          (modify: add `audio_fingerprint(path) -> Result<String>`)
│   ├── manifest.rs        (modify: ManifestEntry field; Action variant; diff signature + logic)
│   ├── ipod\
│   │   └── db.rs          (modify: new `update_track_metadata` method)
│   └── main.rs            (modify: orchestrator handles MetadataOnly; computes audio_fingerprint at Add/Modify/MetadataOnly time)
└── tests\
    └── fixtures\
        └── sample-manifest.json   (modify: add audio_fingerprint to existing entries)
```

No new crate deps. `blake3` is already a runtime dep from Phase 2; the FLAC parser is hand-rolled in `source.rs`.

### Module responsibility delta

- **`source::audio_fingerprint`** — opens a FLAC, walks past metadata blocks, hashes the rest of the file (audio frames + footer) with BLAKE3. Pure I/O + computation, no dependencies on other ipod-sync modules.
- **`manifest`** — gains the new field, the new Action variant, and one new branch in `diff` that's gated on `compute_audio_fingerprint` callback (added next to the existing `compute_fingerprint` parameter).
- **`ipod::db::OwnedDb::update_track_metadata`** — finds an existing track by dbid, calls the same `apply_tags` helper Phase 1 already uses, sets thumbnails via `itdb_track_set_thumbnails_from_data`. Does NOT call `itdb_write` (orchestrator batches that at end of run, same as today).
- **`main`** — orchestrator gains the MetadataOnly arm in its action-apply loop. Also threads `source::audio_fingerprint` as the second callback into `diff`. Computes audio_fingerprint on every Add and Modify to populate the new field in fresh manifest entries.

---

## Task 1: `source::audio_fingerprint` — hand-rolled FLAC parser + BLAKE3

**Files:**
- Modify: `F:\repos\ipod-sync\src\source.rs` (append function + tests)

The FLAC format is:
```
[0..4]   "fLaC" magic
[4..]    sequence of METADATA_BLOCKs; each is:
           [0]      1-bit last-flag (MSB) + 7-bit block type
           [1..4]   24-bit big-endian block length (payload bytes)
           [4..4+N] payload (skip)
         continues until the block with last-flag=1
[after]  audio frames until EOF
```

We never need to decode anything — just walk past the metadata blocks to find where audio starts, then hash from there to EOF.

- [ ] **Step 1: Write the failing tests**

Append to `src/source.rs` (inside the existing `#[cfg(test)] mod tests` block):

```rust
    #[test]
    fn audio_fingerprint_invariant_across_tag_edits() {
        let tmp = tempdir_under_target();
        // Synthesize two FLACs with IDENTICAL audio but DIFFERENT metadata, via ffmpeg.
        // Same lavfi sine source → bit-identical PCM → bit-identical FLAC audio frames.
        let a = tmp.join("a.flac");
        let b = tmp.join("b.flac");
        ffmpeg_synth_flac(&a, "Title A", "Artist A");
        ffmpeg_synth_flac(&b, "Title B", "Artist B");

        let fa = audio_fingerprint(&a).unwrap();
        let fb = audio_fingerprint(&b).unwrap();
        assert_eq!(fa, fb,
            "tag-only differences must not change the audio fingerprint");
        assert!(fa.starts_with("blake3-audio:"),
            "fingerprint must be prefixed to distinguish from file fingerprint");
        assert_eq!(fa.len(), "blake3-audio:".len() + 64);

        // Sanity: confirm the FILE fingerprints DO differ (tags changed the file bytes)
        let file_a = fingerprint(&a).unwrap();
        let file_b = fingerprint(&b).unwrap();
        assert_ne!(file_a, file_b,
            "file fingerprints SHOULD differ when tags differ — this confirms the test setup");
    }

    #[test]
    fn audio_fingerprint_differs_when_audio_differs() {
        let tmp = tempdir_under_target();
        let a = tmp.join("a.flac");
        let b = tmp.join("b.flac");
        ffmpeg_synth_flac_with_freq(&a, "Same Title", 440.0);
        ffmpeg_synth_flac_with_freq(&b, "Same Title", 880.0);  // different sine frequency
        let fa = audio_fingerprint(&a).unwrap();
        let fb = audio_fingerprint(&b).unwrap();
        assert_ne!(fa, fb,
            "different audio content must produce different audio fingerprints");
    }

    #[test]
    fn audio_fingerprint_rejects_non_flac() {
        let tmp = tempdir_under_target();
        let p = tmp.join("not-a-flac.txt");
        std::fs::write(&p, b"hello world").unwrap();
        let err = audio_fingerprint(&p).unwrap_err();
        assert!(err.to_string().contains("fLaC"),
            "error message must mention the missing FLAC magic: {err}");
    }

    /// Helper: synthesize a 1-second 440Hz sine FLAC with the given title/artist tags via ffmpeg.
    /// Used by audio_fingerprint tests.
    fn ffmpeg_synth_flac(path: &std::path::Path, title: &str, artist: &str) {
        ffmpeg_synth_flac_with_freq_and_tags(path, 440.0, title, artist);
    }

    fn ffmpeg_synth_flac_with_freq(path: &std::path::Path, title: &str, freq: f64) {
        ffmpeg_synth_flac_with_freq_and_tags(path, freq, title, "Test Artist");
    }

    fn ffmpeg_synth_flac_with_freq_and_tags(
        path: &std::path::Path,
        freq: f64,
        title: &str,
        artist: &str,
    ) {
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-loglevel", "error", "-y",
                "-f", "lavfi",
                "-i", &format!("sine=frequency={freq}:duration=1:sample_rate=44100"),
                "-c:a", "flac",
                "-metadata", &format!("TITLE={title}"),
                "-metadata", &format!("ARTIST={artist}"),
            ])
            .arg(path)
            .status()
            .expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg synth failed for {}", path.display());
    }
```

The tests depend on ffmpeg being on PATH (which Phase 1+ also requires).

- [ ] **Step 2: Run the tests to verify they fail**

```powershell
cd F:\repos\ipod-sync
cargo test source::tests::audio_fingerprint 2>&1 | Select-Object -Last 10
```
Expected: FAIL — `audio_fingerprint` is undefined.

- [ ] **Step 3: Implement `audio_fingerprint`**

Add to `src/source.rs` next to the existing `fingerprint` function:

```rust
use std::io::{Read, Seek, SeekFrom};

/// BLAKE3 of just the FLAC audio frames — bypasses METADATA_BLOCKs entirely so
/// tag/art edits don't change the fingerprint.
///
/// FLAC spec: https://xiph.org/flac/format.html
/// - 4-byte "fLaC" magic
/// - Sequence of METADATA_BLOCKs (each: 1 byte last-flag+type, 3 bytes BE length, payload)
/// - Audio frames until EOF
///
/// We walk past every metadata block (cheap — header + skip), then hash the rest.
pub fn audio_fingerprint(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)
        .map_err(|e| anyhow!("open for audio fingerprint: {e}"))?;

    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)
        .map_err(|e| anyhow!("read fLaC magic from {}: {e}", path.display()))?;
    if &magic != b"fLaC" {
        return Err(anyhow!(
            "not a FLAC file (missing fLaC magic) at {}",
            path.display()
        ));
    }

    // Skip every metadata block to position the cursor at the start of audio frames.
    loop {
        let mut header = [0u8; 4];
        f.read_exact(&mut header)
            .map_err(|e| anyhow!("read metadata block header from {}: {e}", path.display()))?;
        let is_last = (header[0] & 0x80) != 0;
        // 24-bit big-endian payload length follows the type byte.
        let length = u32::from_be_bytes([0, header[1], header[2], header[3]]);
        f.seek(SeekFrom::Current(length as i64))
            .map_err(|e| anyhow!("seek past metadata block in {}: {e}", path.display()))?;
        if is_last {
            break;
        }
    }

    // Cursor is now at the start of audio frames. Stream-hash from here to EOF.
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)
            .map_err(|e| anyhow!("read audio frames from {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("blake3-audio:{}", hasher.finalize().to_hex()))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```powershell
cargo test source::tests::audio_fingerprint 2>&1 | Select-Object -Last 10
```
Expected: 3 tests pass.

Run the full source test suite to confirm no regressions:
```powershell
cargo test source:: 2>&1 | Select-Object -Last 5
```
Expected: 8 tests pass (5 from Phase 2 + 3 new).

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add src\source.rs
git -C F:\repos\ipod-sync commit -m "feat(source): audio_fingerprint skips FLAC metadata blocks

BLAKE3 over just the audio frames so tag/art edits don't change the
fingerprint. Hand-rolled FLAC metadata parser (~30 lines) — no new deps.
Output is prefixed 'blake3-audio:' to distinguish from the file
fingerprint's 'blake3:' prefix."
```

---

## Task 2: `ManifestEntry.audio_fingerprint` schema field

**Files:**
- Modify: `F:\repos\ipod-sync\src\manifest.rs`
- Modify: `F:\repos\ipod-sync\tests\fixtures\sample-manifest.json`

Additive, backwards-compatible. Phase 2 manifests deserialize cleanly with an empty-string default.

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `src/manifest.rs`:

```rust
    #[test]
    fn manifest_entry_supports_optional_audio_fingerprint() {
        // Old-shape JSON (Phase 2 manifest with no audio_fingerprint field):
        let old = r#"{
            "source_path": "C:\\a.flac",
            "source_mtime": 1700000000,
            "source_size": 100,
            "source_fingerprint": "blake3:aa",
            "ipod_dbid": 1234,
            "ipod_relpath": "iPod_Control\\Music\\F01\\AAAA.m4a",
            "source_known": true
        }"#;
        let entry: ManifestEntry = serde_json::from_str(old).unwrap();
        assert_eq!(entry.audio_fingerprint, "",
            "Phase 2 entries must deserialize with empty audio_fingerprint");

        // New-shape JSON (Phase 3.x manifest):
        let new = r#"{
            "source_path": "C:\\b.flac",
            "source_mtime": 1700000000,
            "source_size": 200,
            "source_fingerprint": "blake3:bb",
            "ipod_dbid": 5678,
            "ipod_relpath": "iPod_Control\\Music\\F02\\BBBB.m4a",
            "source_known": true,
            "audio_fingerprint": "blake3-audio:cc"
        }"#;
        let entry: ManifestEntry = serde_json::from_str(new).unwrap();
        assert_eq!(entry.audio_fingerprint, "blake3-audio:cc");
    }
```

- [ ] **Step 2: Run to verify FAIL**

```powershell
cargo test manifest::tests::manifest_entry_supports_optional_audio_fingerprint 2>&1 | Select-Object -Last 5
```
Expected: FAIL — `ManifestEntry` has no `audio_fingerprint` field.

- [ ] **Step 3: Add the field**

Find the `pub struct ManifestEntry` in `src/manifest.rs` and add the new field at the end:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub source_path: PathBuf,
    pub source_mtime: i64,
    pub source_size: u64,
    pub source_fingerprint: String,
    pub ipod_dbid: u64,
    pub ipod_relpath: String,
    #[serde(default = "default_source_known")]
    pub source_known: bool,
    /// BLAKE3 of just the FLAC audio frames (Phase 3.x). Empty string for
    /// entries written by Phase 2 or earlier — those fall through the diff's
    /// normal Modify path on first content change after upgrade, and the
    /// orchestrator populates this field on the resulting re-write.
    #[serde(default)]
    pub audio_fingerprint: String,
}
```

- [ ] **Step 4: Update the test fixture**

Edit `F:\repos\ipod-sync\tests\fixtures\sample-manifest.json` — add `"audio_fingerprint"` to BOTH entries. The existing `source_known: true` entry gets a real-looking value; the `source_known: false` entry can have an empty string (rebuild-manifest entries don't know audio details):

```json
{
  "version": 1,
  "ipod_serial": "<placeholder-serial>",
  "tracks": [
    {
      "source_path": "\\\\<source-host>\\data\\media\\music\\Beck\\Sea Change\\1-09 Already Dead.flac",
      "source_mtime": 1700000000,
      "source_size": 28349123,
      "source_fingerprint": "blake3:1111111111111111111111111111111111111111111111111111111111111111",
      "ipod_dbid": 12345678901234,
      "ipod_relpath": "iPod_Control\\Music\\F12\\KLMN.m4a",
      "source_known": true,
      "audio_fingerprint": "blake3-audio:2222222222222222222222222222222222222222222222222222222222222222"
    },
    {
      "source_path": "",
      "source_mtime": 0,
      "source_size": 0,
      "source_fingerprint": "",
      "ipod_dbid": 98765432109876,
      "ipod_relpath": "iPod_Control\\Music\\F01\\ABCD.m4a",
      "source_known": false,
      "audio_fingerprint": ""
    }
  ]
}
```

(Keep the existing host/serial placeholders that the history-rewrite produced — don't reintroduce real names.)

The existing `roundtrip_known_fixture` test should still pass with the new field present.

- [ ] **Step 5: Run all manifest tests**

```powershell
cargo test manifest:: 2>&1 | Select-Object -Last 12
```
Expected: 11 tests pass (10 from Phase 2 + 1 new).

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\manifest.rs tests\fixtures\sample-manifest.json
git -C F:\repos\ipod-sync commit -m "feat(manifest): ManifestEntry.audio_fingerprint field

Additive Phase 3.x schema change. serde default = '' for backwards-compat
with Phase 2 manifests. Diff logic to consume this field comes in Task 3."
```

---

## Task 3: `Action::MetadataOnly` variant + diff logic

**Files:**
- Modify: `F:\repos\ipod-sync\src\manifest.rs` (Action enum + diff signature + diff body)

The new diff branch: when the file fingerprint differs AND the manifest entry has a non-empty audio_fingerprint AND we can compute the source's audio_fingerprint AND they match → MetadataOnly. Otherwise (no manifest audio_fingerprint, or audio_fingerprint differs) → Modify.

- [ ] **Step 1: Write the failing tests**

Append to the manifest `tests` block:

```rust
    #[test]
    fn diff_classifies_metadata_only_when_audio_matches() {
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.audio_fingerprint = "blake3-audio:zz".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        // Source: file fingerprint changed (tag edit), but audio is identical.
        let sources = vec![sample_source(r"C:\a.flac", "blake3:bb", 100)];
        let actions = diff(
            &manifest,
            &sources,
            returns("blake3:bb"),
            returns_audio("blake3-audio:zz"),
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::MetadataOnly { .. }),
            "got {:?}", actions[0]);
    }

    #[test]
    fn diff_falls_back_to_modify_when_manifest_has_no_audio_fingerprint() {
        // Phase 2 manifest entry — audio_fingerprint is empty string.
        let entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        // (Don't set audio_fingerprint — sample_entry leaves it empty per default.)
        assert_eq!(entry.audio_fingerprint, "",
            "test premise: sample_entry produces empty audio_fingerprint");
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:bb", 100)];
        let actions = diff(
            &manifest,
            &sources,
            returns("blake3:bb"),
            never_called_audio(),  // audio callback MUST NOT be invoked — we can't compare without manifest value
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)),
            "bootstrap path: missing audio_fingerprint in manifest forces Modify, got {:?}", actions[0]);
    }

    #[test]
    fn diff_classifies_modify_when_audio_actually_changed() {
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.audio_fingerprint = "blake3-audio:zz".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        // Source: file fingerprint changed AND audio is different.
        let sources = vec![sample_source(r"C:\a.flac", "blake3:bb", 100)];
        let actions = diff(
            &manifest,
            &sources,
            returns("blake3:bb"),
            returns_audio("blake3-audio:different"),
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_skips_audio_fingerprint_when_stat_matches() {
        let mut entry = sample_entry(r"C:\a.flac", "blake3:aa", 100);
        entry.audio_fingerprint = "blake3-audio:zz".to_string();
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        // Source: stat matches manifest (mtime + size identical) — fast path.
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(
            &manifest,
            &sources,
            never_called(),
            never_called_audio(),
        ).unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)));
    }

    fn returns_audio(fp: &str) -> impl FnMut(&Path) -> Result<String> + '_ {
        move |_| Ok(fp.to_string())
    }

    fn never_called_audio() -> impl FnMut(&Path) -> Result<String> {
        |_| panic!("audio fingerprint callback should not be called in this scenario")
    }
```

Also update the existing diff tests that currently pass only one callback — they all need a second `never_called_audio()` callback now since the signature is changing. Specifically:
- `diff_classifies_unchanged` → add `never_called_audio()` 4th arg (and probably needs no other change)
- `diff_classifies_new` → add `never_called_audio()` 4th arg
- `diff_classifies_modified_when_fingerprint_changes` → audio callback should fire and return a different fingerprint. Easier: have it return `"blake3-audio:something-else"` and assert Modify still classifies (since no manifest audio_fingerprint anyway, the bootstrap-Modify rule applies regardless).
- `diff_classifies_modified_when_size_changes` → same — bootstrap path means callback can be `never_called_audio()` since manifest has no audio_fingerprint to compare against.
- `diff_classifies_removed` → no source-side processing; `never_called_audio()`
- `diff_preserves_unknown_source_entries` → `never_called_audio()`
- `diff_unchanged_after_touch_but_same_content` → manifest entry has empty audio_fingerprint (sample_entry default), so the new audio-fingerprint code path doesn't even consider it; `never_called_audio()`

Be careful: any test whose manifest entries have `audio_fingerprint=""` should expect Modify (not MetadataOnly) on a fingerprint mismatch. The new MetadataOnly path is only reached when manifest's audio_fingerprint is non-empty.

- [ ] **Step 2: Run to verify FAIL**

```powershell
cargo test manifest:: 2>&1 | Select-Object -Last 15
```
Expected: compile errors — `diff` signature wrong (extra arg), `Action::MetadataOnly` undefined.

- [ ] **Step 3: Add the Action variant**

Find the `pub enum Action` in `src/manifest.rs`. Add the new variant:

```rust
#[derive(Debug, Clone)]
pub enum Action {
    Add(SourceEntry),
    Modify(SourceEntry, ManifestEntry),
    Remove(ManifestEntry),
    Unchanged(ManifestEntry),
    /// File fingerprint changed (tag/art edit) but the audio frames are
    /// bit-identical. Orchestrator updates iPod-side tags + thumbnails
    /// without re-transcoding or re-copying the audio file.
    MetadataOnly {
        source: SourceEntry,
        entry: ManifestEntry,
    },
}
```

- [ ] **Step 4: Update the `diff` signature + body**

Replace the existing `pub fn diff(...)` with:

```rust
pub fn diff(
    manifest: &Manifest,
    sources: &[SourceEntry],
    mut compute_fingerprint: impl FnMut(&Path) -> Result<String>,
    mut compute_audio_fingerprint: impl FnMut(&Path) -> Result<String>,
) -> Result<Vec<Action>> {
    let manifest_by_path: HashMap<&PathBuf, &ManifestEntry> = manifest
        .tracks
        .iter()
        .filter(|e| e.source_known)
        .map(|e| (&e.source_path, e))
        .collect();
    let source_paths: std::collections::HashSet<&PathBuf> =
        sources.iter().map(|s| &s.path).collect();

    let mut actions = Vec::new();

    for src in sources {
        match manifest_by_path.get(&src.path) {
            None => actions.push(Action::Add(src.clone())),
            Some(entry) => {
                let stat_matches = entry.source_mtime == src.mtime
                    && entry.source_size == src.size;
                if stat_matches {
                    actions.push(Action::Unchanged((*entry).clone()));
                } else {
                    let fp = compute_fingerprint(&src.path)?;
                    let file_unchanged = fp == entry.source_fingerprint
                        && src.size == entry.source_size;
                    if file_unchanged {
                        actions.push(Action::Unchanged((*entry).clone()));
                    } else if !entry.audio_fingerprint.is_empty() {
                        // Phase 3.x path: compare audio-only fingerprints.
                        let audio_fp = compute_audio_fingerprint(&src.path)?;
                        if audio_fp == entry.audio_fingerprint {
                            actions.push(Action::MetadataOnly {
                                source: src.clone(),
                                entry: (*entry).clone(),
                            });
                        } else {
                            actions.push(Action::Modify(src.clone(), (*entry).clone()));
                        }
                    } else {
                        // Phase 2 manifest entry — no audio_fingerprint to compare
                        // against. Bootstrap path: fall through to Modify.
                        actions.push(Action::Modify(src.clone(), (*entry).clone()));
                    }
                }
            }
        }
    }

    for entry in &manifest.tracks {
        if !entry.source_known {
            continue;
        }
        if !source_paths.contains(&entry.source_path) {
            actions.push(Action::Remove(entry.clone()));
        }
    }

    Ok(actions)
}
```

- [ ] **Step 5: Run all manifest tests**

```powershell
cargo test manifest:: 2>&1 | Select-Object -Last 18
```
Expected: 15 tests pass (11 from Phase 2 + 4 new).

If any old tests still fail because they used the old single-callback signature, update them with `never_called_audio()` per Step 1's notes.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\manifest.rs
git -C F:\repos\ipod-sync commit -m "feat(manifest): Action::MetadataOnly + audio_fingerprint diff branch

Diff now takes a second callback for the audio-only fingerprint. When a
file's fingerprint differs from the manifest BUT the manifest has a
stored audio_fingerprint AND it matches the source's current audio
fingerprint, emit MetadataOnly instead of Modify.

Bootstrap fallback: Phase 2 manifest entries (audio_fingerprint=='')
still go through Modify on the first content change after upgrade. The
orchestrator (Task 5) populates audio_fingerprint on the resulting
re-write so subsequent edits hit the fast path."
```

---

## Task 4: `OwnedDb::update_track_metadata` — in-place iPod tag + art update

**Files:**
- Modify: `F:\repos\ipod-sync\src\ipod\db.rs`

This is the FFI side. Find the existing iPod track by `dbid`, overwrite its tag fields, set its thumbnails. Does NOT call `itdb_write` — the orchestrator batches that at the end of the run (same pattern as `add_track_with_file` and `delete_track`).

- [ ] **Step 1: Verify bindgen exposes the needed symbols**

```powershell
$bindings = Get-ChildItem F:\repos\ipod-sync\target\debug\build\ipod-sync-*\out\libgpod_bindings.rs | Sort-Object LastWriteTime -Descending | Select-Object -First 1
$needed = @("itdb_track_set_thumbnails_from_data")
foreach ($name in $needed) {
    $hit = Select-String -Path $bindings.FullName -Pattern "\bfn $name\b" -Quiet
    "$name : $(if ($hit) { 'OK' } else { 'MISSING' })"
}
```
Expected: OK. (Phase 1 already added this to the allowlist via `itdb_.*`.)

- [ ] **Step 2: Implement `update_track_metadata`**

Append to the `impl OwnedDb` block in `src/ipod/db.rs`:

```rust
    /// Update an existing iPod track's tags + thumbnails without touching the
    /// audio file. Used by the Phase 3.x MetadataOnly path: the source file's
    /// audio is bit-identical to what's already on the iPod, so we just refresh
    /// the metadata libgpod tracks for it.
    ///
    /// Does NOT call `itdb_write` — caller batches that at end of run.
    /// Returns `Ok(())` even if the dbid isn't found (idempotent, matches
    /// `delete_track`'s semantics).
    pub fn update_track_metadata(
        &self,
        dbid: u64,
        tags: &Tags,
        art: Option<&[u8]>,
    ) -> Result<()> {
        unsafe {
            let mut node = (*self.0).tracks;
            let mut found: *mut ffi::Itdb_Track = std::ptr::null_mut();
            while !node.is_null() {
                let t = (*node).data as *mut ffi::Itdb_Track;
                if !t.is_null() && (*t).dbid as u64 == dbid {
                    found = t;
                    break;
                }
                node = (*node).next;
            }
            if found.is_null() {
                return Ok(()); // idempotent: track not present
            }

            apply_tags(found, tags);

            if let Some(bytes) = art {
                let ok = ffi::itdb_track_set_thumbnails_from_data(
                    found,
                    bytes.as_ptr(),
                    bytes.len() as _,
                );
                if ok == 0 {
                    return Err(anyhow!(
                        "itdb_track_set_thumbnails_from_data failed for dbid {dbid}"
                    ));
                }
            }
        }
        Ok(())
    }
```

(`apply_tags` already exists in this module from Phase 1 — same helper `add_track_with_file` uses.)

- [ ] **Step 3: Build + confirm clean**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test ipod:: 2>&1 | Select-Object -Last 5
```
Expected: clean build, existing ipod tests still pass (no new unit tests — the FFI path is exercised live in Task 6).

- [ ] **Step 4: Commit**

```powershell
git -C F:\repos\ipod-sync add src\ipod\db.rs
git -C F:\repos\ipod-sync commit -m "feat(ipod::db): update_track_metadata for in-place tag+art refresh

Phase 3.x MetadataOnly path. Finds existing track by dbid, calls the
shared apply_tags helper for string fields, sets thumbnails via
itdb_track_set_thumbnails_from_data. Idempotent (Ok if dbid not found,
matching delete_track's pattern). Defers itdb_write to the orchestrator's
end-of-run batch."
```

---

## Task 5: Orchestrator — handle MetadataOnly + populate audio_fingerprint

**Files:**
- Modify: `F:\repos\ipod-sync\src\main.rs`

Three changes:

1. The `diff` call passes `source::audio_fingerprint` as the new fourth argument.
2. The action-apply loop gains a `MetadataOnly` arm that extracts new tags + art from the source FLAC (via the existing `transcode::probe` + `transcode::extract_cover_art` + the existing `tags_from_probe` helper), then calls `OwnedDb::update_track_metadata`.
3. `add_one` (used by Add and Modify) ALSO computes `audio_fingerprint` and returns it alongside the file fingerprint + TrackHandle, so the new manifest entry carries both. `entry_from` takes both fingerprints.

- [ ] **Step 1: Update `add_one` to return the audio fingerprint too**

Find `fn add_one(db: &OwnedDb, src: &SourceEntry) -> Result<(TrackHandle, String)>` in `src/main.rs`. Change return type and body:

```rust
fn add_one(db: &OwnedDb, src: &SourceEntry) -> Result<(TrackHandle, String, String)> {
    let probe = transcode::probe(&src.path)
        .with_context(|| format!("probe {}", src.path.display()))?;
    let tags = tags_from_probe(&probe);

    let temp = transcode::temp_alac_path();
    if let Some(parent) = temp.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    transcode::transcode_to_alac(&src.path, &temp)
        .with_context(|| format!("transcode {}", src.path.display()))?;

    let art = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        transcode::extract_cover_art(&src.path, &art_path)?;
        let bytes = std::fs::read(&art_path)?;
        let _ = std::fs::remove_file(&art_path);
        Some(bytes)
    } else {
        None
    };

    // Phase 3.x: compute the audio-only fingerprint so the manifest entry can
    // detect "this file's tags changed but audio didn't" on future runs.
    let audio_fp = source::audio_fingerprint(&src.path)
        .with_context(|| format!("audio_fingerprint {}", src.path.display()))?;
    let file_fp = source::fingerprint(&src.path)
        .with_context(|| format!("fingerprint {}", src.path.display()))?;

    let handle = db.add_track_with_file(&temp, &tags, art.as_deref())
        .with_context(|| format!("add_track_with_file for {}", src.path.display()))?;

    let _ = std::fs::remove_file(&temp);
    Ok((handle, file_fp, audio_fp))
}
```

Update every caller of `add_one` to bind both fingerprints:
- Add action: `let (handle, file_fp, audio_fp) = add_one(&db, &src)?;` then push `entry_from(&src, &handle, &file_fp, &audio_fp)`
- Modify action: same shape

- [ ] **Step 2: Update `entry_from` signature**

Find `fn entry_from(src: &SourceEntry, handle: &TrackHandle, fingerprint: &str) -> ManifestEntry` and change to:

```rust
fn entry_from(
    src: &SourceEntry,
    handle: &TrackHandle,
    fingerprint: &str,
    audio_fingerprint: &str,
) -> ManifestEntry {
    ManifestEntry {
        source_path: src.path.clone(),
        source_mtime: src.mtime,
        source_size: src.size,
        source_fingerprint: fingerprint.to_string(),
        ipod_dbid: handle.dbid,
        ipod_relpath: handle.ipod_relpath.clone(),
        source_known: true,
        audio_fingerprint: audio_fingerprint.to_string(),
    }
}
```

Update both callers (Add and Modify arms) accordingly.

- [ ] **Step 3: Pass the audio_fingerprint callback into `diff`**

Find the existing `manifest::diff(&manifest, &sources, source::fingerprint)?` call. Change to:

```rust
let actions = manifest::diff(
    &manifest,
    &sources,
    source::fingerprint,
    source::audio_fingerprint,
)?;
```

- [ ] **Step 4: Handle the new MetadataOnly action in the apply loop**

In the `for action in actions` loop, add a new arm (place it near `Action::Modify`):

```rust
        Action::MetadataOnly { source, entry } => {
            i += 1;
            progress.track_start(
                i,
                total_planned,
                format!("METADATA {}", source.path.display()),
            );
            let probe = transcode::probe(&source.path)
                .with_context(|| format!("probe {}", source.path.display()))?;
            let tags = tags_from_probe(&probe);
            let art = if has_embedded_art(&probe) {
                let art_path = transcode::temp_art_path();
                transcode::extract_cover_art(&source.path, &art_path)?;
                let bytes = std::fs::read(&art_path)?;
                let _ = std::fs::remove_file(&art_path);
                Some(bytes)
            } else {
                None
            };
            db.update_track_metadata(entry.ipod_dbid, &tags, art.as_deref())?;

            // Refresh the manifest entry — same iPod identity, new source
            // fingerprint + mtime/size from the touched file, audio_fingerprint
            // unchanged (we verified it matched in the diff).
            let new_file_fp = source::fingerprint(&source.path)?;
            manifest.tracks.retain(|e| e.ipod_dbid != entry.ipod_dbid);
            manifest.tracks.push(ManifestEntry {
                source_path: source.path.clone(),
                source_mtime: source.mtime,
                source_size: source.size,
                source_fingerprint: new_file_fp,
                ipod_dbid: entry.ipod_dbid,
                ipod_relpath: entry.ipod_relpath.clone(),
                source_known: true,
                audio_fingerprint: entry.audio_fingerprint.clone(),
            });
            progress.track_done();
        }
```

The MetadataOnly arm needs to recompute `source::fingerprint(&source.path)` so the manifest's `source_fingerprint` reflects the NEW file state — otherwise the next run would still see "manifest fingerprint mismatch" and re-process unnecessarily.

- [ ] **Step 5: Adjust `total_planned` to count MetadataOnly actions**

Find the `count_actions` function and add a counter for MetadataOnly:

```rust
fn count_actions(actions: &[Action]) -> (usize, usize, usize, usize, usize) {
    let mut add = 0;
    let mut modify = 0;
    let mut metadata_only = 0;
    let mut remove = 0;
    let mut unchanged = 0;
    for a in actions {
        match a {
            Action::Add(_) => add += 1,
            Action::Modify(_, _) => modify += 1,
            Action::MetadataOnly { .. } => metadata_only += 1,
            Action::Remove(_) => remove += 1,
            Action::Unchanged(_) => unchanged += 1,
        }
    }
    (add, modify, metadata_only, remove, unchanged)
}
```

Update the caller (in `run`) to destructure the 5-tuple:
```rust
let (add, modify, metadata_only, remove, unchanged) = count_actions(&actions);
```

Add to the action plan printout (use `progress.log` or in the existing summary line):
```rust
progress.log(format!(
    "action plan: add={add} modify={modify} metadata={metadata_only} remove={remove} unchanged={unchanged}"
));
```

Update `total_planned` to include MetadataOnly:
```rust
let total_planned = add + modify + metadata_only
    + if config.no_delete { 0 } else { remove };
```

And in the "Nothing to do" early-exit, add `metadata_only` to the condition:
```rust
if add == 0 && modify == 0 && metadata_only == 0
    && (remove == 0 || config.no_delete)
{
    progress.log("Nothing to do.".to_string());
    progress.finish();
    return Ok(());
}
```

- [ ] **Step 6: Build + run all tests**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test 2>&1 | Select-Object -Last 8
```
Expected: clean build, all existing tests still pass (we didn't add main.rs tests in this task — the integration test is the live verification in Task 6).

- [ ] **Step 7: Commit**

```powershell
git -C F:\repos\ipod-sync add src\main.rs
git -C F:\repos\ipod-sync commit -m "feat(main): handle MetadataOnly action + populate audio_fingerprint

Orchestrator threads source::audio_fingerprint as the diff's audio
callback. add_one returns the computed audio fingerprint alongside the
file fingerprint, both written into the manifest entry. New MetadataOnly
arm re-probes the source, applies new tags + art via
OwnedDb::update_track_metadata (no transcode, no file copy), and refreshes
the manifest entry's file fingerprint so the next run sees a clean state.

count_actions + total_planned + early-exit + action-plan log line all
extended to include metadata_only."
```

---

## Task 6: Live verification — Gate

**Files:**
- Modify: `F:\repos\ipod-sync\LEARNINGS.md` (gate record)

End-to-end test against real iPod hardware + real source files. This is the gate; if it doesn't pass, do not tag.

The test plan:

1. **Setup**: iPod at G:, manifest at `%APPDATA%\ipod-sync\manifest.json` (Phase 2 state — 1,407 tracks, no audio_fingerprints).
2. **First-run baseline** (the bootstrap): sync (no changes). Should be Unchanged=1407, sub-second. Manifest entries STILL have empty audio_fingerprint (no diff path computed them).
3. **Force a single tag-only change on one source FLAC**: pick a track, use ffmpeg to write a new TITLE tag without touching audio. Confirm via metaflac that the audio frames are unchanged.
4. **Second sync** (the bootstrap-Modify): the touched track shows as Modify (audio_fingerprint was empty), gets re-transcoded + re-copied. After this run, that track's manifest entry HAS audio_fingerprint.
5. **Edit the SAME track's tags again** (different TITLE).
6. **Third sync** (the actual gate): the touched track should now show as **MetadataOnly**, the action should complete in well under a second (no transcode), iPod plays the updated tags after eject.

- [ ] **Step 1: Pre-flight checks**

```powershell
Test-Path G:\iPod_Control\iTunes\iTunesDB
(Get-ChildItem G:\iPod_Control\Music -Recurse -Filter *.m4a -Force | Measure-Object).Count
(Get-Content $env:APPDATA\ipod-sync\manifest.json | ConvertFrom-Json).tracks.Count
```
Expected: True, ~1407, ~1407. (If the iPod was re-wiped since Phase 2 Gate C, you'll need to re-sync first; that's a separate exercise.)

- [ ] **Step 2: Baseline no-change run**

```powershell
$env:IPOD_SYNC_SOURCE = "\\<source-host>\data\media\music"
$timer = [System.Diagnostics.Stopwatch]::StartNew()
cargo run --release -- --no-tui
$timer.Stop()
"baseline elapsed: $($timer.Elapsed.TotalSeconds.ToString('F2'))s"
```
Expected: `add=0 modify=0 metadata=0 remove=0 unchanged=1407`, total elapsed < 5s (same fast-path SPEC §6 #2 baseline as Gate C).

- [ ] **Step 3: Force a tag-only change on one track**

Pick a small album that's quick to find. Use ffmpeg to rewrite with a changed TITLE but identical audio:

```powershell
$src = "\\<source-host>\data\media\music\<some-artist>\<some-album>\01 <some-track>.flac"
# Read original metadata for restoration later
ffprobe -loglevel error -of json -show_format $src | Out-File F:\repos\ipod-sync\target\test-original-meta.json
# Re-mux with a new TITLE, copy audio bit-exact (no re-encode)
$tmp = "$env:TEMP\phase3x-edited.flac"
ffmpeg -loglevel error -y -i $src -c:a copy -metadata "TITLE=PHASE3X TEST EDIT" $tmp
# Replace the original with the edited file
Move-Item $tmp $src -Force
```

Optional: verify the audio is bit-identical via metaflac or equivalent:
```powershell
# Compare audio MD5 (FLAC stores an MD5 of the decoded audio in STREAMINFO)
ffprobe -loglevel error -show_streams -of default=noprint_wrappers=1 $src | Select-String "_md5"
```
Note the value; it should match what the file had before the tag edit (since `-c:a copy` doesn't touch audio).

- [ ] **Step 4: Second sync (bootstrap-Modify)**

```powershell
$timer = [System.Diagnostics.Stopwatch]::StartNew()
cargo run --release -- --no-tui
$timer.Stop()
"bootstrap-modify elapsed: $($timer.Elapsed.TotalSeconds.ToString('F1'))s"
```
Expected: `add=0 modify=1 metadata=0 remove=0 unchanged=1406`, total elapsed ~5-10s (one track transcoded + iPod write). The output line should include `MODIFY \\<source-host>\...`.

Confirm the manifest now has an audio_fingerprint for that entry:
```powershell
$manifest = Get-Content $env:APPDATA\ipod-sync\manifest.json | ConvertFrom-Json
$entry = $manifest.tracks | Where-Object source_path -match "PHASE3X|some-track-name"
$entry.audio_fingerprint
```
Expected: a non-empty `blake3-audio:...` value.

- [ ] **Step 5: Tag-edit the same track AGAIN**

```powershell
$src = "\\<source-host>\data\media\music\<same-artist>\<same-album>\01 <same-track>.flac"
$tmp = "$env:TEMP\phase3x-edit2.flac"
ffmpeg -loglevel error -y -i $src -c:a copy -metadata "TITLE=PHASE3X TEST EDIT 2" $tmp
Move-Item $tmp $src -Force
```

- [ ] **Step 6: Third sync — the actual gate**

```powershell
$timer = [System.Diagnostics.Stopwatch]::StartNew()
cargo run --release -- --no-tui
$timer.Stop()
"metadata-only elapsed: $($timer.Elapsed.TotalSeconds.ToString('F2'))s"
```
Expected:
- `add=0 modify=0 metadata=1 remove=0 unchanged=1406`
- Output line includes `METADATA \\<source-host>\...`
- Total elapsed should be MUCH less than Step 4's bootstrap-Modify time. Per-track work: just the ffprobe + ffmpeg art extract + libgpod tag set. Probably 1-2 seconds total (most of that is the walker stat + diff overhead from the 1,406 unchanged tracks).

If the action shows as Modify (not MetadataOnly), the gate FAILED — investigate the audio_fingerprint comparison logic. Likely causes: the audio_fingerprint actually differs (ffmpeg's `-c:a copy` did something subtle), or the manifest didn't get the audio_fingerprint written in Step 4.

- [ ] **Step 7: Physical iPod verification**

Eject cleanly, unplug, boot the iPod, navigate to the test track on Now Playing. Should display the new TITLE ("PHASE3X TEST EDIT 2") and the original album art (we didn't change art in this test).

- [ ] **Step 8: Restore the original tags** (don't leave the test edit in your library)

Find the original TITLE from `target/test-original-meta.json`, restore via ffmpeg:
```powershell
$origMeta = Get-Content F:\repos\ipod-sync\target\test-original-meta.json | ConvertFrom-Json
$origTitle = $origMeta.format.tags.TITLE
$src = "\\<source-host>\data\media\music\<same-artist>\<same-album>\01 <same-track>.flac"
$tmp = "$env:TEMP\phase3x-restore.flac"
ffmpeg -loglevel error -y -i $src -c:a copy -metadata "TITLE=$origTitle" $tmp
Move-Item $tmp $src -Force
```

Run a fourth sync — should classify as MetadataOnly again (audio still unchanged, tags reverted). Confirms the path is reusable.

- [ ] **Step 9: Record the gate result**

Append to `LEARNINGS.md`:

```markdown
## Phase 3.x gate (YYYY-MM-DD) — PASS / FAIL

- **Result:** PASS / FAIL (<reason>).
- **Baseline no-change run elapsed:** <X>s
- **Bootstrap-Modify elapsed (touched 1 file with empty audio_fingerprint):** <X>s
- **MetadataOnly elapsed (same file, second edit):** <X>s
- **Speedup vs bootstrap-Modify:** <ratio>×
- **iPod-side verification:** new TITLE shown / not shown on Now Playing
- **Restoration check (fourth sync after restoring original tags):** MetadataOnly fired again / Modify

### Observations
- (anything surprising: ffmpeg -c:a copy edge cases, audio_fingerprint stability across re-muxes, libgpod write-time noise, etc.)
```

- [ ] **Step 10: Commit + tag**

```powershell
git -C F:\repos\ipod-sync add LEARNINGS.md
git -C F:\repos\ipod-sync commit -m "docs: Phase 3.x gate result"
git -C F:\repos\ipod-sync tag -a phase-3x-complete -m "Metadata-only smart-update verified

- audio_fingerprint via hand-rolled FLAC metadata-skip parser + BLAKE3
- diff classifies MetadataOnly when file fingerprint differs but audio matches
- OwnedDb::update_track_metadata refreshes tags + thumbnails in place
  (no re-transcode, no audio file copy)
- Bootstrap path still works: Phase 2 manifest entries with empty
  audio_fingerprint fall through to Modify and accrue audio_fingerprints
  for the next cycle
- Live-verified on iPod Classic 7G: metadata edits propagate to Now
  Playing after eject"
```

---

## Self-review

**Spec coverage check (against the Phase 3.x section of `2026-05-18-post-v1-roadmap.md`):**

- "Add `audio_fingerprint: String` field to `ManifestEntry` (additive, backwards-compat via `#[serde(default)]`)" → Task 2 ✓
- "New `source::audio_fingerprint(path)` helper: parse FLAC structure, hash only audio payload" → Task 1 ✓ (hand-rolled parser instead of claxon/metaflac — no new dep)
- "Diff gains a new branch: file fingerprint differs BUT audio fingerprint matches → `Action::MetadataOnly`" → Task 3 ✓
- "New `OwnedDb::update_track_metadata(dbid, tags, art)` method" → Task 4 ✓
- "Orchestrator handles `MetadataOnly` as a fast cheap action" → Task 5 ✓
- "Migration: lazy — only files about to be Modify-ed anyway get audio-fingerprinted" → Task 3 (bootstrap-fallback branch) + Task 5 (add_one always computes audio_fingerprint on Add/Modify) ✓
- Acceptance criteria 1-6 from brainstorming → Task 6 steps 1-7 cover #1, #2 (baseline), #3 (bootstrap), #6 (manifest reload via existing tests). Acceptance #4 (iPod-side) is Task 6 Step 7. Acceptance #5 (audio change triggers Modify) is implicitly covered by Task 3 unit test `diff_classifies_modify_when_audio_actually_changed` and Task 6 Step 8 restoration check.

**Placeholder scan:** No "TBD/TODO/implement later/handle edge cases" lurking. Function bodies all show real code. Test code shows real assertions with concrete values. The few `<some-artist>` / `<some-album>` placeholders in Task 6 Steps 3/5/8 are user-input slots (the engineer running the gate picks the test track), not implementation gaps.

**Type consistency:**
- `audio_fingerprint: String` (not `Option<String>`) — used consistently in `ManifestEntry`, in `entry_from`, in the MetadataOnly arm.
- `Action::MetadataOnly { source: SourceEntry, entry: ManifestEntry }` — same shape in declaration (Task 3 Step 3), tests (Task 3 Step 1), and the apply arm (Task 5 Step 4).
- `add_one` return type `(TrackHandle, String, String)` — first String is file fingerprint, second is audio fingerprint. Consistent in caller updates.
- `entry_from(src, handle, fingerprint, audio_fingerprint)` — 4 params, consistent in declaration and both call sites.
- `compute_audio_fingerprint: impl FnMut(&Path) -> Result<String>` — same signature in diff declaration, in test callbacks `returns_audio` / `never_called_audio`, and in the orchestrator's call site (`source::audio_fingerprint`).

**Scope check:** Phase 3.x only. No Phase 3 (format pass-through, refalac) or Phase 4 (multi-iPod) work creeps in. The MetadataOnly path operates on FLAC source files only — pass-through formats (which don't transcode in the first place) aren't a concern here because they'd never have a meaningful "different file fingerprint, same audio" case (pass-through copies the file bit-for-bit; if file changes, all of it changes).

No new crate dependencies. The hand-rolled FLAC parser is ~30 lines and the format is trivially stable (FLAC spec hasn't changed since 2003).
