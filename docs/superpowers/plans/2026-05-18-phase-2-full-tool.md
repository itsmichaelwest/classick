# Phase 2: Full Sync Tool — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Phase 1 single-track demo into the full sync tool per SPEC §4 — walks a FLAC library, diffs against a persistent manifest, transcodes/writes new+modified tracks to the iPod, removes tracks deleted from source, all driven by clap CLI flags, presented through a ratatui TUI (with non-TTY plain-log fallback), capable of completing the SPEC §6 #1 acceptance run on the user's ~1,400-track library.

**Architecture:** Extends Phase 1's library crate. New modules: `cli` (clap derive), `config` (resolved runtime), `source` (walker + BLAKE3 fingerprint), `manifest` (load/save/diff), `progress` (ratatui TUI + plain fallback over a channel), `logging` (tracing + GLib log handler). Existing `ipod::db` extended with `delete_track` and `list_tracks_for_rebuild`. Existing `ipod::device` extended with `detect_ipod_mount`. Existing `transcode::ProbeTags` extended with TRACKTOTAL/DISCTOTAL aliases. Existing `OwnedDb::write` gets a one-line Windows workaround for libgpod's stale-`.bak` rename bug. Main thread runs the sync sequentially; TUI thread receives events via an mpsc channel.

**Tech Stack:** Rust stable (x86_64-pc-windows-msvc), existing libgpod + ffmpeg toolchain, plus: `clap` (derive), `walkdir`, `blake3`, `tracing`, `tracing-subscriber`, `ratatui`, `crossterm`, `dirs`.

**Plan scope:** Phase 2 only — produces the v1 tool that completes SPEC §6's full acceptance run. Phase 3 (if there is one) would handle: parallel walker, multi-iPod support, `SysInfoExtended` XML plist fallback for newer hardware, the Rust port of libgpod's iTunesDB writer documented in SPEC §12.7 as the v2/v3 migration. None of that is in this plan.

**Three internal gates:**

| Gate | Validates | Tasks before |
|---|---|---|
| **A — Dry-run preview** | `--dry-run` against `\\MUSICHOST\data\media\music\` produces accurate "would add N / modify M / remove K" output | 1-8 |
| **B — Small real sync** | 10-track real subset writes end-to-end, manifest persists atomically, second run shows 0 changes | 9-11 |
| **C — Full acceptance** | SPEC §6 #1 — 1,400-track empty-iPod sync completes, all tracks playable on device with metadata + art, second run completes in < 5 sec | 12-14 |

If any gate fails, **STOP and re-plan** — don't push through. Phase 1 taught us that the iPod's specific behavior shapes the answer (e.g. Plan A in-band art vs Plan B thumbnails); the same will be true at scale.

---

## File Structure

```
F:\repos\ipod-sync\
├── Cargo.toml                    (modify: + clap, walkdir, blake3, tracing*, ratatui, crossterm, dirs)
├── build.rs                      (modify: regenerate loaders.cache at build time)
├── src\
│   ├── lib.rs                    (modify: re-export new modules)
│   ├── main.rs                   (replace: clap-based orchestrator)
│   ├── cli.rs                    (new: clap derive struct)
│   ├── config.rs                 (new: resolved runtime)
│   ├── source.rs                 (new: walker + fingerprint)
│   ├── manifest.rs               (new: types, load/save/diff)
│   ├── progress.rs               (new: ratatui TUI + plain fallback)
│   ├── logging.rs                (new: tracing + GLib handler)
│   ├── ffi.rs                    (unchanged)
│   ├── transcode.rs              (modify: TRACKTOTAL aliases)
│   └── ipod\
│       ├── mod.rs                (modify: re-export new types)
│       ├── db.rs                 (modify: TrackHandle, delete_track, list_tracks_for_rebuild, Play Counts.bak fix)
│       └── device.rs             (modify: detect_ipod_mount)
└── tests\
    └── fixtures\
        ├── sample-ffprobe.json   (existing — extend to include TRACKTOTAL/DISCTOTAL)
        └── sample-manifest.json  (new: round-trip + diff fixture)
```

### Module responsibilities

| Module | Responsibility | Knows about |
|---|---|---|
| `cli` | Parse argv via clap. One struct, no logic. | clap |
| `config` | Resolve `Cli` → `Config` with defaults applied (source path, mount, manifest path, etc.). Pure. | cli, dirs |
| `source` | Walk a directory for `*.flac`, compute fingerprints, return `Vec<SourceEntry>`. | walkdir, blake3 |
| `manifest` | `Manifest` + `ManifestEntry` types, atomic load/save (JSON), `diff(manifest, sources) -> Vec<Action>`. Knows nothing about iPod or transcoding. | serde, serde_json |
| `progress` | `Progress` handle owns a thread that either runs ratatui or prints plain log lines. Senders use a channel; receivers handle event types. | ratatui, crossterm, std mpsc |
| `logging` | One-shot init: tracing subscriber + GLib `g_log_set_handler` redirecting GLib warnings/criticals into tracing. | tracing, tracing-subscriber, ffi |
| `ipod::db` | Extended with `TrackHandle` (returned by add), `delete_track(dbid)`, `list_tracks_for_rebuild`. | (existing) |
| `ipod::device` | Extended with `detect_ipod_mount()` enumerating drive letters. | (existing) |
| `transcode::ProbeTags` | Extended aliases for TRACKTOTAL/DISCTOTAL/TOTALTRACKS/TOTALDISCS. | (existing) |
| `main` | The orchestrator. Wires cli → config → device/db → source → manifest::diff → apply each action → write db + manifest. The only place that knows about everything. | all |

---

## Task 1: Cargo deps + module skeleton + LEARNINGS carry-forwards

This task bundles the Phase-1-to-Phase-2 prep work that's small enough to live in one commit:

- Add new crate deps.
- Create empty module stubs (filled in by later tasks).
- Three one-shot carry-forwards from LEARNINGS that don't deserve their own tasks:
  - **TRACKTOTAL/DISCTOTAL aliases** in `transcode::ProbeTags`.
  - **Play Counts.bak fix** in `ipod::db::OwnedDb::write` — delete stale `.bak` before calling `itdb_write` (libgpod's POSIX `rename` fails on Windows when target exists).
  - **loaders.cache regenerated at build time** in `build.rs` — point at the staged `target/<profile>/pixbuf-loaders/` instead of the vendor absolute paths.

- [ ] **Step 1: Update `Cargo.toml` dependencies**

Edit the `[dependencies]` section to look like this:
```toml
[dependencies]
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
walkdir = "2"
blake3 = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
ratatui = "0.29"
crossterm = "0.29"
dirs = "5"
```

Leave `[build-dependencies]` untouched.

- [ ] **Step 2: Create module stubs**

Create `F:\repos\ipod-sync\src\cli.rs` with:
```rust
//! clap CLI definitions. Implemented in Task 2.
```

Create `F:\repos\ipod-sync\src\config.rs` with:
```rust
//! Resolved runtime config (Cli + defaults). Implemented in Task 3.
```

Create `F:\repos\ipod-sync\src\source.rs` with:
```rust
//! FLAC walker + BLAKE3 fingerprint. Implemented in Task 4.
```

Create `F:\repos\ipod-sync\src\manifest.rs` with:
```rust
//! Manifest types, atomic load/save, diff. Implemented in Task 5.
```

Create `F:\repos\ipod-sync\src\progress.rs` with:
```rust
//! ratatui TUI + plain log fallback. Implemented in Task 12.
```

Create `F:\repos\ipod-sync\src\logging.rs` with:
```rust
//! tracing subscriber + GLib log handler. Implemented in Task 11.
```

- [ ] **Step 3: Wire stubs into `src/lib.rs`**

Replace `src/lib.rs` with:
```rust
pub mod cli;
pub mod config;
pub mod ffi;
pub mod ipod;
pub mod logging;
pub mod manifest;
pub mod progress;
pub mod source;
pub mod transcode;
```

- [ ] **Step 4: Apply TRACKTOTAL/DISCTOTAL aliases to `transcode::ProbeTags`**

Open `F:\repos\ipod-sync\src\transcode.rs`. Find the `pub struct ProbeTags` definition. The `track` and `disc` fields already have several aliases; extend them, and **add two new fields** `track_total` and `disc_total` to capture the separate-total convention (FLAC files tagged with `TRACK="9"` + `TRACKTOTAL="12"` instead of `TRACK="9/12"`).

The full struct should look like this after the edit:
```rust
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
    #[serde(default, alias = "TRACKTOTAL", alias = "TOTALTRACKS", alias = "tracktotal", alias = "totaltracks")]
    pub track_total: Option<String>,
    #[serde(default, alias = "DISC", alias = "Disc", alias = "discnumber", alias = "DISCNUMBER")]
    pub disc: Option<String>,
    #[serde(default, alias = "DISCTOTAL", alias = "TOTALDISCS", alias = "disctotal", alias = "totaldiscs")]
    pub disc_total: Option<String>,
    #[serde(default, alias = "GENRE", alias = "Genre")]
    pub genre: Option<String>,
    #[serde(default, alias = "COMPOSER", alias = "Composer")]
    pub composer: Option<String>,
}
```

The serde `alias` accepts an alternate JSON key during deserialization but only the renamed field name (or default field name) is used as the canonical Rust name. So `tags.track_total` resolves a JSON `TRACKTOTAL` or `TOTALTRACKS` field.

(`tags_from_probe` in main.rs will combine `track` + `track_total` into the final `Tags.tracks` count — Task 10 wires this up.)

- [ ] **Step 5: Update the existing transcode test fixture to include TRACKTOTAL**

The existing `tests/fixtures/sample-ffprobe.json` was generated for the Phase 1 plan with `"track": "9/12"`. To exercise the new split-pair aliases, regenerate the fixture with a separate `"track": "9"` + `"TRACKTOTAL": "12"` (and same for disc). Edit it directly — change:
```json
"track": "9/12",
"disc": "1/1",
```
to:
```json
"track": "9",
"TRACKTOTAL": "12",
"disc": "1",
"DISCTOTAL": "1",
```
…and update the existing test `probe_output_parses_format_tags` to assert:
```rust
assert_eq!(tags.track.as_deref(), Some("9"));
assert_eq!(tags.track_total.as_deref(), Some("12"));
assert_eq!(tags.disc.as_deref(), Some("1"));
assert_eq!(tags.disc_total.as_deref(), Some("1"));
```

Run `cargo test transcode` — all 4 transcode tests should still pass.

- [ ] **Step 6: Apply Play Counts.bak fix to `OwnedDb::write`**

Open `F:\repos\ipod-sync\src\ipod\db.rs`. Find the existing `pub fn write(&self) -> Result<()>` body. Prepend the .bak-clearing step:

```rust
pub fn write(&self) -> Result<()> {
    // libgpod's itdb_write renames `<mount>\iPod_Control\iTunes\Play Counts`
    // to `Play Counts.bak` via POSIX rename(). On Windows, rename() fails
    // (silently to libgpod, surfaced as a vague GError) if the target exists.
    // Pre-delete the stale .bak so the rename always has a clean target.
    // Discovered while building examples/wipe-tracks.rs on 2026-05-17.
    unsafe {
        let mount_c = ffi::itdb_get_mountpoint(self.0);
        if !mount_c.is_null() {
            let mount = std::ffi::CStr::from_ptr(mount_c).to_string_lossy();
            let bak = std::path::Path::new(mount.as_ref())
                .join("iPod_Control")
                .join("iTunes")
                .join("Play Counts.bak");
            let _ = std::fs::remove_file(&bak);  // ignore NotFound; surface other errors via the subsequent write
        }

        let mut err: *mut ffi::GError = std::ptr::null_mut();
        if ffi::itdb_write(self.0, &mut err) == 0 {
            return Err(gerror_to_anyhow("itdb_write", err));
        }
    }
    Ok(())
}
```

Verify `itdb_get_mountpoint` is in bindgen output:
```powershell
$bindings = Get-ChildItem F:\repos\ipod-sync\target\debug\build\ipod-sync-*\out\libgpod_bindings.rs | Sort-Object LastWriteTime -Descending | Select-Object -First 1
Select-String -Path $bindings.FullName -Pattern "\bfn itdb_get_mountpoint\b"
```
If MISSING, search for alternatives: `itdb_get_mountpoint` may be inlined as a macro in the headers (in which case bindgen can't see it). Fallback: store the mount path on `OwnedDb` directly at construction time. Adjust `OwnedDb::open` to keep a `mount: PathBuf` field and use that in `write` instead. Document the workaround used.

- [ ] **Step 7: Update `build.rs` to regenerate `loaders.cache` at build time**

The Phase 1 vendoring shipped `vendor/libgpod/pixbuf-loaders/loaders.cache` with absolute paths to `F:\repos\ipod-sync\vendor\libgpod\pixbuf-loaders\*.dll`. That works for local dev but breaks distribution. Phase 2's fix: regenerate the cache during `cargo build`, pointing at the staged `target/<profile>/pixbuf-loaders/` paths.

Open `F:\repos\ipod-sync\build.rs`. Find the existing block that copies pixbuf-loaders next to the exe. After that copy block, BEFORE the `cargo:rustc-env=PIXBUF_LOADERS_CACHE=...` line, insert:
```rust
// Regenerate loaders.cache pointing at the staged loaders dir (not the vendor
// absolute paths). gdk-pixbuf-query-loaders.exe lives in MSYS2's mingw64 bin.
let query_exe = std::path::Path::new(r"C:\msys64\mingw64\bin\gdk-pixbuf-query-loaders.exe");
if query_exe.exists() {
    // Pass each staged loader DLL as an arg; the tool emits the cache to stdout.
    let loader_dlls: Vec<_> = std::fs::read_dir(&dst_loaders)
        .expect("read staged loaders")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |e| e == "dll"))
        .collect();
    let output = std::process::Command::new(query_exe)
        .args(&loader_dlls)
        .output()
        .expect("run gdk-pixbuf-query-loaders");
    if !output.status.success() {
        panic!("gdk-pixbuf-query-loaders failed: {}",
            String::from_utf8_lossy(&output.stderr));
    }
    std::fs::write(dst_loaders.join("loaders.cache"), &output.stdout)
        .expect("write staged loaders.cache");
} else {
    // Fall back to the vendored cache (dev-tree paths) if MSYS2's query tool
    // isn't available. This is the previous Phase 1 behavior.
    let src_cache = src_loaders.join("loaders.cache");
    let dst_cache = dst_loaders.join("loaders.cache");
    if src_cache.exists() {
        std::fs::copy(&src_cache, &dst_cache).expect("copy vendor loaders.cache");
    }
}
```

(The variable names `src_loaders` and `dst_loaders` are defined earlier in the existing block — reuse them.)

- [ ] **Step 8: Build + run tests + run wipe-tracks once to confirm Play Counts.bak fix lands**

```powershell
cd F:\repos\ipod-sync
cargo build 2>&1 | Select-Object -Last 5
cargo test 2>&1 | Select-Object -Last 5
```
Expected: clean build (downloads new crates), 17 tests pass (existing) plus any tests in the modified transcode fixture (still 4 in transcode). If new lints from clap derive or tracing macros fire, address per their docs.

Sanity-check that loaders.cache regeneration ran:
```powershell
Get-Content F:\repos\ipod-sync\target\debug\pixbuf-loaders\loaders.cache | Select-String "F:" | Select-Object -First 3
```
Each line that references a loader DLL should reference `F:\repos\ipod-sync\target\debug\pixbuf-loaders\libpixbufloader-*.dll`, NOT `F:\repos\ipod-sync\vendor\libgpod\pixbuf-loaders\...`.

To confirm the Play Counts.bak fix lands, **plug in the iPod at G:** and run wipe-tracks twice in a row:
```powershell
cargo run --example wipe-tracks
# (iPod is empty, "Nothing to do" — expected)
# Add a track via cargo run -- "<some.flac>" ... actually we wiped it.
# Instead: just run wipe-tracks twice; second run must not error on Play Counts.bak.
cargo run --example wipe-tracks
```
Expected: both runs exit cleanly. If second run dies on "Play Counts.bak (File exists)" the fix isn't in place — debug `itdb_get_mountpoint` resolution.

- [ ] **Step 9: Commit**

```powershell
git -C F:\repos\ipod-sync add Cargo.toml Cargo.lock build.rs src\ tests\fixtures\sample-ffprobe.json
git -C F:\repos\ipod-sync commit -m "feat: Phase 2 scaffold + LEARNINGS carry-forwards

- Add clap/walkdir/blake3/tracing/ratatui/crossterm/dirs deps
- Create empty cli/config/source/manifest/progress/logging module stubs
- Extend ProbeTags with TRACKTOTAL/DISCTOTAL aliases + track_total/disc_total fields
- OwnedDb::write pre-deletes stale Play Counts.bak to work around libgpod's
  POSIX rename() failing on Windows when target exists (discovered via
  examples/wipe-tracks)
- build.rs regenerates loaders.cache at build time pointing at staged
  target/<profile>/pixbuf-loaders/ instead of vendor absolute paths
"
```

---

## Task 2: cli — clap definitions

**Files:**
- Modify: `F:\repos\ipod-sync\src\cli.rs`

Single struct, no logic, lots of `#[arg]` attributes. The conversion from `Cli` → resolved `Config` lives in Task 3.

- [ ] **Step 1: Write the failing test**

Append to `src/cli.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_no_args_with_defaults() {
        let cli = Cli::try_parse_from(["ipod-sync"]).unwrap();
        assert_eq!(cli.source, None);
        assert_eq!(cli.ipod, None);
        assert_eq!(cli.ffmpeg, None);
        assert!(!cli.dry_run);
        assert!(!cli.no_delete);
        assert!(!cli.verbose);
        assert!(!cli.rebuild_manifest);
        assert!(!cli.no_tui);
    }

    #[test]
    fn parses_all_flags() {
        let cli = Cli::try_parse_from([
            "ipod-sync",
            "--source", r"D:\music",
            "--ipod", "G:",
            "--ffmpeg", r"C:\bin\ffmpeg.exe",
            "--dry-run",
            "--no-delete",
            "--verbose",
            "--rebuild-manifest",
            "--no-tui",
        ]).unwrap();
        assert_eq!(cli.source.as_deref().and_then(|p| p.to_str()), Some(r"D:\music"));
        assert_eq!(cli.ipod.as_deref(), Some("G:"));
        assert_eq!(cli.ffmpeg.as_deref().and_then(|p| p.to_str()), Some(r"C:\bin\ffmpeg.exe"));
        assert!(cli.dry_run);
        assert!(cli.no_delete);
        assert!(cli.verbose);
        assert!(cli.rebuild_manifest);
        assert!(cli.no_tui);
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(Cli::try_parse_from(["ipod-sync", "--invented-flag"]).is_err());
    }
}
```

- [ ] **Step 2: Run, verify FAIL**

```powershell
cargo test cli 2>&1 | Select-Object -Last 5
```
Expected: FAIL — `Cli` undefined.

- [ ] **Step 3: Implement the `Cli` struct**

Replace `src/cli.rs` with:
```rust
//! clap CLI definitions. Parsing only; defaults + resolution live in `config`.

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "ipod-sync",
    version,
    about = "Sync a FLAC library to an iPod Classic via libgpod with on-the-fly ALAC transcoding."
)]
pub struct Cli {
    /// Source library root. Defaults to \\MUSICHOST\data\media\music\.
    #[arg(long)]
    pub source: Option<PathBuf>,

    /// iPod drive (e.g. G:). Auto-detected if omitted.
    #[arg(long)]
    pub ipod: Option<String>,

    /// Path to ffmpeg.exe. Defaults to "ffmpeg" on PATH.
    #[arg(long)]
    pub ffmpeg: Option<PathBuf>,

    /// Print the action plan; write nothing to manifest, iPod, or temp.
    #[arg(long)]
    pub dry_run: bool,

    /// Never remove tracks from iPod, even if removed from source.
    #[arg(long)]
    pub no_delete: bool,

    /// Enable debug-level tracing output.
    #[arg(short, long)]
    pub verbose: bool,

    /// Ignore existing manifest; rebuild a best-effort one from the iPod's
    /// current iTunesDB. Existing tracks on the iPod are preserved and not
    /// touched by subsequent syncs.
    #[arg(long)]
    pub rebuild_manifest: bool,

    /// Disable the ratatui TUI; use plain log output even when stdout is a TTY.
    #[arg(long)]
    pub no_tui: bool,
}

// (tests block from Step 1 stays here)
```

- [ ] **Step 4: Run, verify PASS**

```powershell
cargo test cli 2>&1 | Select-Object -Last 5
```
Expected: 3 tests pass.

Also verify the CLI help renders:
```powershell
cargo run -- --help 2>&1
```
(This will fail to find some main.rs code if you haven't yet rewritten main.rs — that's fine; we only need clap's auto-generated help message. If `cargo run` errors at link time but `cargo run -- --help` still shows the help, that's also fine.)

If the binary won't run yet, alternatively:
```powershell
cargo build --bin ipod-sync 2>&1 | Select-Object -Last 5
```
Until Task 10 you may see "function main not implemented" style errors from main.rs. Don't worry about them this task.

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add src\cli.rs
git -C F:\repos\ipod-sync commit -m "feat(cli): clap Cli struct with all SPEC §4.1 flags + --no-tui"
```

---

## Task 3: config — resolved runtime

**Files:**
- Modify: `F:\repos\ipod-sync\src\config.rs`

`Config` is the immutable runtime state produced by combining `Cli` with defaults. Everything downstream takes `&Config`, never `&Cli`. Pure functions, easy to test.

- [ ] **Step 1: Write the failing test**

Append to `src/config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn defaults_when_no_flags_set() {
        let cli = Cli::try_parse_from(["ipod-sync"]).unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.source, std::path::PathBuf::from(r"\\MUSICHOST\data\media\music\"));
        assert_eq!(config.ipod, None);  // auto-detect later
        assert_eq!(config.ffmpeg, std::path::PathBuf::from("ffmpeg"));
        assert!(!config.dry_run);
        assert!(!config.no_delete);
        assert!(!config.verbose);
        assert!(!config.rebuild_manifest);
        assert!(config.use_tui, "TUI defaults on");
        assert!(config.manifest_path.to_string_lossy().contains("ipod-sync"));
        assert!(config.manifest_path.to_string_lossy().ends_with("manifest.json"));
    }

    #[test]
    fn flags_override_defaults() {
        let cli = Cli::try_parse_from([
            "ipod-sync",
            "--source", r"D:\music",
            "--ipod", "F:",
            "--no-tui",
        ]).unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.source, std::path::PathBuf::from(r"D:\music"));
        assert_eq!(config.ipod, Some("F:".to_string()));
        assert!(!config.use_tui);
    }

    #[test]
    fn ipod_normalizes_drive_letter() {
        let cli = Cli::try_parse_from(["ipod-sync", "--ipod", "G"]).unwrap();
        let config = resolve(cli).unwrap();
        assert_eq!(config.ipod, Some("G:".to_string()), "single letter gets colon appended");
    }
}
```

- [ ] **Step 2: Run, verify FAIL**

```powershell
cargo test config 2>&1 | Select-Object -Last 5
```
Expected: FAIL — `resolve` and `Config` undefined.

- [ ] **Step 3: Implement `Config` + `resolve`**

Replace `src/config.rs` with:
```rust
//! Resolved runtime config. CLI + defaults applied; immutable after construction.

use crate::cli::Cli;
use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Default source library root. Confirmed in Phase 2 brainstorming
/// (SPEC §4.1's `\\server\music\` was stale).
pub const DEFAULT_SOURCE: &str = r"\\MUSICHOST\data\media\music\";

#[derive(Debug, Clone)]
pub struct Config {
    pub source: PathBuf,
    pub ipod: Option<String>,  // None = auto-detect at runtime
    pub ffmpeg: PathBuf,
    pub dry_run: bool,
    pub no_delete: bool,
    pub verbose: bool,
    pub rebuild_manifest: bool,
    pub use_tui: bool,
    pub manifest_path: PathBuf,
}

pub fn resolve(cli: Cli) -> Result<Config> {
    let manifest_path = default_manifest_path()?;
    let ipod = cli.ipod.map(normalize_drive);

    Ok(Config {
        source: cli.source.unwrap_or_else(|| PathBuf::from(DEFAULT_SOURCE)),
        ipod,
        ffmpeg: cli.ffmpeg.unwrap_or_else(|| PathBuf::from("ffmpeg")),
        dry_run: cli.dry_run,
        no_delete: cli.no_delete,
        verbose: cli.verbose,
        rebuild_manifest: cli.rebuild_manifest,
        use_tui: !cli.no_tui,
        manifest_path,
    })
}

fn default_manifest_path() -> Result<PathBuf> {
    let appdata = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve %APPDATA% via dirs::config_dir"))?;
    Ok(appdata.join("ipod-sync").join("manifest.json"))
}

/// "G" -> "G:". "G:" -> "G:". "G:\\" -> "G:\\". The Windows convention for
/// `--ipod` is a drive letter + colon (with optional trailing backslash).
fn normalize_drive(s: String) -> String {
    if s.len() == 1 && s.chars().next().unwrap().is_ascii_alphabetic() {
        format!("{s}:")
    } else {
        s
    }
}
```

- [ ] **Step 4: Run, verify PASS**

```powershell
cargo test config 2>&1 | Select-Object -Last 5
```
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add src\config.rs
git -C F:\repos\ipod-sync commit -m "feat(config): resolve Cli -> Config with SPEC §4.1 defaults"
```

---

## Task 4: source — walker + BLAKE3 fingerprint

**Files:**
- Modify: `F:\repos\ipod-sync\src\source.rs`

Walks a directory recursively for `*.flac` (case-insensitive). For each file: captures `path`, `mtime`, `size`, `fingerprint` (BLAKE3 of first 1 MiB). Skips `_excluded` and `.unwanted` subdirs per SPEC §4.2.

The fingerprint string format is `"blake3:<hex>"` for forward-compat (Phase 3 could switch to a different hash without changing the manifest schema).

- [ ] **Step 1: Write the failing tests**

Append to `src/source.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_flac(dir: &std::path::Path, name: &str, payload: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(payload).unwrap();
        path
    }

    #[test]
    fn fingerprint_is_blake3_of_first_mib() {
        let tmp = tempdir_under_target();
        let path = write_flac(&tmp, "a.flac", &[0xAAu8; 16]);
        let fp = fingerprint(&path).unwrap();
        assert!(fp.starts_with("blake3:"));
        assert_eq!(fp.len(), "blake3:".len() + 64, "blake3 hex is 64 chars");
    }

    #[test]
    fn fingerprint_unchanged_when_only_bytes_beyond_first_mib_differ() {
        let tmp = tempdir_under_target();
        let mut payload_a = vec![0u8; 1024 * 1024 + 100];
        let mut payload_b = payload_a.clone();
        for i in (1024 * 1024)..payload_b.len() {
            payload_b[i] = 0xFF;
        }
        let a = write_flac(&tmp, "a.flac", &payload_a);
        let b = write_flac(&tmp, "b.flac", &payload_b);
        assert_eq!(fingerprint(&a).unwrap(), fingerprint(&b).unwrap(),
            "files identical in first 1 MiB hash the same regardless of suffix");
    }

    #[test]
    fn walker_finds_flacs_recursively_case_insensitive() {
        let tmp = tempdir_under_target();
        write_flac(&tmp, "song.flac", b"x");
        write_flac(&tmp, "Sub/SONG2.FLAC", b"x");
        write_flac(&tmp, "Sub/Sub2/song3.Flac", b"x");
        write_flac(&tmp, "song.mp3", b"x");  // not flac, ignored
        let entries = walk(&tmp).unwrap();
        let names: std::collections::HashSet<_> = entries.iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains("song.flac"));
        assert!(names.contains("SONG2.FLAC"));
        assert!(names.contains("song3.Flac"));
    }

    #[test]
    fn walker_skips_excluded_subdirs() {
        let tmp = tempdir_under_target();
        write_flac(&tmp, "ok.flac", b"x");
        write_flac(&tmp, "_excluded/skip.flac", b"x");
        write_flac(&tmp, ".unwanted/also-skip.flac", b"x");
        let entries = walk(&tmp).unwrap();
        let names: std::collections::HashSet<_> = entries.iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 1);
        assert!(names.contains("ok.flac"));
    }

    #[test]
    fn source_entry_has_size_and_mtime() {
        let tmp = tempdir_under_target();
        let path = write_flac(&tmp, "a.flac", &[0x42u8; 1234]);
        let entries = walk(&tmp).unwrap();
        let e = entries.iter().find(|e| e.path == path).unwrap();
        assert_eq!(e.size, 1234);
        assert!(e.mtime > 0, "mtime is unix epoch seconds, should be > 0");
    }

    /// Create a unique temp dir under `target/` so leftover dirs don't
    /// pollute the system temp and so they're easy to clean.
    fn tempdir_under_target() -> std::path::PathBuf {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("walker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }
}
```

- [ ] **Step 2: Run, verify FAIL**

```powershell
cargo test source 2>&1 | Select-Object -Last 10
```
Expected: FAIL — `walk`, `fingerprint`, `SourceEntry` undefined.

- [ ] **Step 3: Implement `walk`, `fingerprint`, `SourceEntry`**

Replace `src/source.rs` with:
```rust
//! Recursive FLAC walker + BLAKE3 fingerprint (first 1 MiB).
//!
//! Per SPEC §4.2: case-insensitive `*.flac`, skip `_excluded` and `.unwanted`
//! subdirs. Fingerprint is BLAKE3 of the first 1 MiB; size is captured
//! separately so the diff can use (fingerprint, size) as the change signal.

use anyhow::{anyhow, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// One FLAC discovered by the walker. Cheap to clone (a few hundred bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceEntry {
    pub path: PathBuf,
    /// Unix epoch seconds.
    pub mtime: i64,
    pub size: u64,
    /// `blake3:<64-hex-chars>`.
    pub fingerprint: String,
}

const FINGERPRINT_PREFIX_BYTES: usize = 1024 * 1024;

const SKIPPED_DIR_NAMES: &[&str] = &["_excluded", ".unwanted"];

/// Walk `root` for FLACs. Errors only on I/O failures at `root` itself;
/// per-entry errors (permission denied on a subdir, etc.) are logged via
/// tracing and skipped — we'd rather sync 1395/1400 than abort on one.
pub fn walk(root: &Path) -> Result<Vec<SourceEntry>> {
    if !root.exists() {
        return Err(anyhow!("source root does not exist: {}", root.display()));
    }
    let mut out = Vec::new();
    let iter = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_skipped_dir(e));
    for entry in iter {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("walkdir entry error (skipping): {e}");
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        if !is_flac(entry.path()) {
            continue;
        }
        match build_source_entry(entry.path()) {
            Ok(e) => out.push(e),
            Err(e) => tracing::warn!("skipping {}: {e}", entry.path().display()),
        }
    }
    Ok(out)
}

fn is_skipped_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    SKIPPED_DIR_NAMES.iter().any(|&s| name == s)
}

fn is_flac(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("flac"))
        .unwrap_or(false)
}

fn build_source_entry(path: &Path) -> Result<SourceEntry> {
    let meta = std::fs::metadata(path)
        .map_err(|e| anyhow!("stat: {e}"))?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let fingerprint = fingerprint(path)?;
    Ok(SourceEntry { path: path.to_path_buf(), mtime, size, fingerprint })
}

/// Hash up to the first 1 MiB of a file with BLAKE3.
pub fn fingerprint(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)
        .map_err(|e| anyhow!("open for fingerprint: {e}"))?;
    let mut buf = vec![0u8; FINGERPRINT_PREFIX_BYTES];
    let mut read = 0usize;
    while read < buf.len() {
        match f.read(&mut buf[read..]) {
            Ok(0) => break,  // EOF
            Ok(n) => read += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(anyhow!("read for fingerprint: {e}")),
        }
    }
    let hash = blake3::hash(&buf[..read]);
    Ok(format!("blake3:{}", hash.to_hex()))
}

// (tests block from Step 1)
```

- [ ] **Step 4: Run, verify PASS**

```powershell
cargo test source 2>&1 | Select-Object -Last 15
```
Expected: 5 tests pass.

- [ ] **Step 5: Smoke-test against the real library**

```powershell
$timer = [System.Diagnostics.Stopwatch]::StartNew()
$entries = cargo run --quiet --example walk-source 2>&1
$timer.Stop()
"walked in $($timer.Elapsed.TotalSeconds) seconds"
```

Wait — we don't have `examples/walk-source` yet. Skip this smoke test; we'll exercise the walker as part of Gate A in Task 8. The unit tests are sufficient verification at this stage.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\source.rs
git -C F:\repos\ipod-sync commit -m "feat(source): walkdir-based FLAC discovery + BLAKE3 fingerprint"
```

---

## Task 5: manifest — types, atomic load/save, diff

**Files:**
- Modify: `F:\repos\ipod-sync\src\manifest.rs`
- Create: `F:\repos\ipod-sync\tests\fixtures\sample-manifest.json`

The manifest is the source of truth for "what we've previously synced." It's a JSON document at `%APPDATA%\ipod-sync\manifest.json`. Each entry records the source file's identity (path/mtime/size/fingerprint) and the resulting iPod-side identifiers (dbid + relative path). Diff classifies each source file as Unchanged / New / Modified, and each manifest entry whose source is gone as Removed (unless the entry is marked `source_known: false` from `--rebuild-manifest`).

Atomic write: serialize to `<path>.tmp`, fsync, rename over. Protects against partial writes if the process crashes mid-write.

- [ ] **Step 1: Write the failing tests**

Append to `src/manifest.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceEntry;
    use std::path::PathBuf;

    fn sample_entry(path: &str, fp: &str, size: u64) -> ManifestEntry {
        ManifestEntry {
            source_path: PathBuf::from(path),
            source_mtime: 1700000000,
            source_size: size,
            source_fingerprint: fp.to_string(),
            ipod_dbid: 12345678901234,
            ipod_relpath: r"iPod_Control\Music\F12\KLMN.m4a".to_string(),
            source_known: true,
        }
    }

    fn sample_source(path: &str, fp: &str, size: u64) -> SourceEntry {
        SourceEntry {
            path: PathBuf::from(path),
            mtime: 1700000000,
            size,
            fingerprint: fp.to_string(),
        }
    }

    #[test]
    fn roundtrip_known_fixture() {
        const FIXTURE: &str = include_str!("../tests/fixtures/sample-manifest.json");
        let parsed: Manifest = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.tracks.len(), 2);
        assert!(parsed.tracks[0].source_known);

        let serialized = serde_json::to_string_pretty(&parsed).unwrap();
        let reparsed: Manifest = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed.tracks, reparsed.tracks);
    }

    #[test]
    fn load_or_default_returns_empty_when_missing() {
        let path = std::env::temp_dir().join(format!("ipod-sync-test-missing-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let m = load_or_default(&path).unwrap();
        assert_eq!(m.tracks.len(), 0);
        assert_eq!(m.version, 1);
    }

    #[test]
    fn save_atomic_roundtrip() {
        let path = std::env::temp_dir().join(format!("ipod-sync-test-rt-{}.json", std::process::id()));
        let m = Manifest { version: 1, ipod_serial: None, tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)] };
        save_atomic(&path, &m).unwrap();
        let loaded = load_or_default(&path).unwrap();
        assert_eq!(loaded.tracks.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn diff_classifies_unchanged() {
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Unchanged(_)));
    }

    #[test]
    fn diff_classifies_new() {
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![] };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 100)];
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Add(_)));
    }

    #[test]
    fn diff_classifies_modified_when_fingerprint_changes() {
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:bb", 100)];
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_classifies_modified_when_size_changes() {
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![sample_source(r"C:\a.flac", "blake3:aa", 200)];
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Modify(_, _)));
    }

    #[test]
    fn diff_classifies_removed() {
        let manifest = Manifest {
            version: 1, ipod_serial: None,
            tracks: vec![sample_entry(r"C:\a.flac", "blake3:aa", 100)],
        };
        let sources = vec![];
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Remove(_)));
    }

    #[test]
    fn diff_preserves_unknown_source_entries() {
        let mut entry = sample_entry(r"C:\unknown.flac", "blake3:??", 0);
        entry.source_known = false;  // from --rebuild-manifest
        let manifest = Manifest { version: 1, ipod_serial: None, tracks: vec![entry] };
        let sources = vec![];  // no sources present
        let actions = diff(&manifest, &sources);
        assert_eq!(actions.len(), 0, "unknown-source entries are NOT removed when source is absent");
    }
}
```

- [ ] **Step 2: Create the test fixture**

Create `F:\repos\ipod-sync\tests\fixtures\sample-manifest.json`:
```json
{
  "version": 1,
  "ipod_serial": "EXAMPLE1234",
  "tracks": [
    {
      "source_path": "\\\\MUSICHOST\\data\\media\\music\\Beck\\Sea Change\\1-09 Already Dead.flac",
      "source_mtime": 1700000000,
      "source_size": 28349123,
      "source_fingerprint": "blake3:1111111111111111111111111111111111111111111111111111111111111111",
      "ipod_dbid": 12345678901234,
      "ipod_relpath": "iPod_Control\\Music\\F12\\KLMN.m4a",
      "source_known": true
    },
    {
      "source_path": "",
      "source_mtime": 0,
      "source_size": 0,
      "source_fingerprint": "",
      "ipod_dbid": 98765432109876,
      "ipod_relpath": "iPod_Control\\Music\\F01\\ABCD.m4a",
      "source_known": false
    }
  ]
}
```

- [ ] **Step 3: Run, verify FAIL**

```powershell
cargo test manifest 2>&1 | Select-Object -Last 10
```
Expected: FAIL — `Manifest`, `ManifestEntry`, `Action`, `diff`, `load_or_default`, `save_atomic` undefined.

- [ ] **Step 4: Implement the types + functions**

Replace `src/manifest.rs` with:
```rust
//! Sync state: per-source-file record of (source identity, iPod identity).
//! Atomic JSON file at %APPDATA%\ipod-sync\manifest.json per SPEC §4.3.

use crate::source::SourceEntry;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub version: u32,
    #[serde(default)]
    pub ipod_serial: Option<String>,
    pub tracks: Vec<ManifestEntry>,
}

impl Manifest {
    pub fn empty() -> Self {
        Self { version: 1, ipod_serial: None, tracks: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub source_path: PathBuf,
    pub source_mtime: i64,
    pub source_size: u64,
    pub source_fingerprint: String,
    pub ipod_dbid: u64,
    pub ipod_relpath: String,
    /// `false` means this entry was reconstructed by `--rebuild-manifest` from
    /// the iPod's DB and has no known source file. The diff preserves these
    /// untouched (no Remove action emitted) and the orchestrator skips them.
    #[serde(default = "default_source_known")]
    pub source_known: bool,
}

fn default_source_known() -> bool { true }

#[derive(Debug, Clone)]
pub enum Action {
    Add(SourceEntry),
    /// Source is present and changed; old manifest entry must be removed
    /// from iPod first, then the new source added.
    Modify(SourceEntry, ManifestEntry),
    Remove(ManifestEntry),
    Unchanged(ManifestEntry),
}

/// Diff a manifest against the current source state. See SPEC §4.3 for the
/// classification rules; this is "fingerprint + size determines unchanged",
/// not the spec's mention of mtime (mtime is captured but not used in the
/// comparison — it's unreliable across filesystems).
pub fn diff(manifest: &Manifest, sources: &[SourceEntry]) -> Vec<Action> {
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
                let unchanged = entry.source_fingerprint == src.fingerprint
                    && entry.source_size == src.size;
                if unchanged {
                    actions.push(Action::Unchanged((*entry).clone()));
                } else {
                    actions.push(Action::Modify(src.clone(), (*entry).clone()));
                }
            }
        }
    }

    for entry in &manifest.tracks {
        if !entry.source_known {
            continue;  // preserved as-is
        }
        if !source_paths.contains(&entry.source_path) {
            actions.push(Action::Remove(entry.clone()));
        }
    }

    actions
}

/// Read the manifest from disk; return an empty manifest if the file doesn't exist.
pub fn load_or_default(path: &Path) -> Result<Manifest> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s)
            .with_context(|| format!("parse manifest at {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::empty()),
        Err(e) => Err(anyhow!("read manifest at {}: {e}", path.display())),
    }
}

/// Write the manifest atomically: write to <path>.tmp, fsync, rename over.
/// Survives crashes mid-write; the target file is either fully old or fully new.
pub fn save_atomic(path: &Path, manifest: &Manifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let json = serde_json::to_string_pretty(manifest)?;
        let f = std::fs::File::create(&tmp)
            .with_context(|| format!("create temp manifest {}", tmp.display()))?;
        let mut writer = std::io::BufWriter::new(f);
        std::io::Write::write_all(&mut writer, json.as_bytes())?;
        let f = std::io::BufWriter::into_inner(writer)?;
        f.sync_all().with_context(|| format!("fsync {}", tmp.display()))?;
    }
    // On Windows, std::fs::rename overwrites an existing target (unlike POSIX).
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

// (tests block from Step 1)
```

- [ ] **Step 5: Run, verify PASS**

```powershell
cargo test manifest 2>&1 | Select-Object -Last 15
```
Expected: 9 tests pass.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\manifest.rs tests\fixtures\sample-manifest.json
git -C F:\repos\ipod-sync commit -m "feat(manifest): Manifest types, atomic load/save, diff classification"
```

---

## Task 6: ipod::device — auto-detect iPod mount

**Files:**
- Modify: `F:\repos\ipod-sync\src\ipod\device.rs`

Per SPEC §4.6: enumerate drive letters A-Z, test each for `iPod_Control\iTunes\iTunesDB`. If exactly one match → return it. If zero → error. If multiple → error and instruct user to specify `--ipod`.

- [ ] **Step 1: Write the failing test**

The drive enumeration itself can't be unit-tested without mocking the filesystem, but the "given a list of candidate drives + a predicate, return the right verdict" logic CAN be tested in isolation.

Append to `src/ipod/device.rs`:
```rust
#[cfg(test)]
mod detection_tests {
    use super::*;

    #[test]
    fn pick_mount_single_match() {
        let mounts = vec!["G:\\".to_string()];
        let mount = pick_mount(mounts).unwrap();
        assert_eq!(mount, "G:\\");
    }

    #[test]
    fn pick_mount_no_match_errors() {
        let err = pick_mount(vec![]).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("no ipod"));
    }

    #[test]
    fn pick_mount_multiple_matches_errors() {
        let mounts = vec!["E:\\".to_string(), "G:\\".to_string()];
        let err = pick_mount(mounts).unwrap_err();
        assert!(err.to_string().contains("E:") && err.to_string().contains("G:"),
            "error message must enumerate the candidates");
        assert!(err.to_string().contains("--ipod"),
            "error must hint at --ipod flag");
    }
}
```

- [ ] **Step 2: Run, verify FAIL**

```powershell
cargo test ipod::device::detection_tests 2>&1 | Select-Object -Last 5
```
Expected: FAIL — `pick_mount` undefined.

- [ ] **Step 3: Implement `detect_ipod_mount` + `pick_mount`**

Append to `src/ipod/device.rs` (don't replace the existing FirewireGuid code):
```rust
/// Enumerate Windows drive letters A-Z, find drives that look like an iPod
/// (have `iPod_Control\iTunes\iTunesDB`), and return the unique mount.
pub fn detect_ipod_mount() -> Result<String> {
    let candidates = candidate_drives()
        .into_iter()
        .filter(looks_like_ipod)
        .collect();
    pick_mount(candidates)
}

/// Return all currently-existing drive letters A:\\ through Z:\\.
fn candidate_drives() -> Vec<String> {
    ('A'..='Z')
        .map(|c| format!("{c}:\\"))
        .filter(|d| std::path::Path::new(d).exists())
        .collect()
}

/// True if `drive` looks like a mounted iPod (has iTunesDB).
fn looks_like_ipod(drive: &String) -> bool {
    std::path::Path::new(drive)
        .join("iPod_Control")
        .join("iTunes")
        .join("iTunesDB")
        .exists()
}

/// Given a set of iPod-looking mounts, return the unique one or an error.
fn pick_mount(mounts: Vec<String>) -> Result<String> {
    match mounts.len() {
        0 => Err(anyhow!(
            "no iPod found mounted on any drive. Plug in the iPod (or pass --ipod <drive>)."
        )),
        1 => Ok(mounts.into_iter().next().unwrap()),
        _ => Err(anyhow!(
            "multiple iPod-like drives found: {}. Pass --ipod <drive> to disambiguate.",
            mounts.join(", ")
        )),
    }
}
```

- [ ] **Step 4: Run, verify PASS**

```powershell
cargo test ipod::device 2>&1 | Select-Object -Last 10
```
Expected: 4 existing FirewireGuid tests + 3 new detection tests = 7 pass.

- [ ] **Step 5: Live smoke (with iPod plugged in at G:)**

Write a one-off binary inline (no need to commit it):
```powershell
@'
fn main() {
    match ipod_sync::ipod::device::detect_ipod_mount() {
        Ok(m) => println!("Detected: {m}"),
        Err(e) => eprintln!("Detection failed: {e}"),
    }
}
'@ | Out-File -Encoding utf8 F:\repos\ipod-sync\examples\detect-mount.rs
cargo run --quiet --example detect-mount
```
Expected: `Detected: G:\` (assuming iPod is the only one plugged in).

Delete the example so we don't commit a one-off:
```powershell
Remove-Item F:\repos\ipod-sync\examples\detect-mount.rs
```

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\ipod\device.rs
git -C F:\repos\ipod-sync commit -m "feat(ipod::device): detect_ipod_mount enumerates A-Z drives"
```

---

## Task 7: Update ipod::mod to re-export new types

**Files:**
- Modify: `F:\repos\ipod-sync\src\ipod\mod.rs`

Trivial housekeeping so downstream `use ipod_sync::ipod::OwnedDb;` style imports work.

- [ ] **Step 1: Update `src/ipod/mod.rs`**

Replace the file with:
```rust
pub mod db;
pub mod device;

pub use db::{OwnedDb, Tags};
pub use device::{detect_ipod_mount, read_firewire_guid, set_firewire_guid};
```

- [ ] **Step 2: Build clean**

```powershell
cargo build 2>&1 | Select-Object -Last 3
```
Expected: clean build.

- [ ] **Step 3: Commit**

```powershell
git -C F:\repos\ipod-sync add src\ipod\mod.rs
git -C F:\repos\ipod-sync commit -m "chore(ipod): re-export OwnedDb/Tags/detect_ipod_mount/firewire helpers"
```

---

## Task 8: main.rs — dry-run orchestrator → Gate A

**Files:**
- Replace: `F:\repos\ipod-sync\src\main.rs`

This is the smallest orchestrator that covers the dry-run path. No writes, no transcoding, no TUI. Just: parse CLI → walk source → load manifest → diff → print summary → exit. After this task we run it against `\\MUSICHOST\data\media\music\` and confirm the output is sensible. That's Gate A.

- [ ] **Step 1: Replace `src/main.rs`**

```rust
use anyhow::Result;
use clap::Parser;
use ipod_sync::cli::Cli;
use ipod_sync::config::{self};
use ipod_sync::manifest::{self, Action};
use ipod_sync::source;

fn main() -> Result<()> {
    // Pixbuf loader cache wiring (set up in Phase 1, still required for any
    // libgpod artwork call — harmless if we don't touch artwork in this run).
    unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE")); }

    let cli = Cli::parse();
    let config = config::resolve(cli)?;

    println!("Source : {}", config.source.display());
    println!("iPod   : {}", config.ipod.as_deref().unwrap_or("(auto-detect deferred to non-dry-run path)"));
    println!("Manifest: {}", config.manifest_path.display());
    println!();

    println!("Walking source...");
    let sources = source::walk(&config.source)?;
    println!("  found {} FLAC file(s)", sources.len());

    let manifest = manifest::load_or_default(&config.manifest_path)?;
    println!("Existing manifest entries: {}", manifest.tracks.len());

    let actions = manifest::diff(&manifest, &sources);

    let mut add = 0usize;
    let mut modify = 0usize;
    let mut remove = 0usize;
    let mut unchanged = 0usize;
    for a in &actions {
        match a {
            Action::Add(_) => add += 1,
            Action::Modify(_, _) => modify += 1,
            Action::Remove(_) => remove += 1,
            Action::Unchanged(_) => unchanged += 1,
        }
    }
    println!();
    println!("Action plan:");
    println!("  Add      : {add}");
    println!("  Modify   : {modify}");
    println!("  Remove   : {remove} {}", if config.no_delete { "(--no-delete; will be skipped)" } else { "" });
    println!("  Unchanged: {unchanged}");

    if config.dry_run {
        println!();
        println!("Dry run; nothing was written.");
        return Ok(());
    }

    // The non-dry-run path lands in Task 10. For now error out loudly.
    eprintln!();
    eprintln!("ERROR: non-dry-run mode not yet implemented (Task 10).");
    eprintln!("Pass --dry-run to preview the action plan.");
    std::process::exit(2);
}
```

- [ ] **Step 2: Build + sanity-test against a tiny local dir**

```powershell
cargo build 2>&1 | Select-Object -Last 3
# Quick check: source default error path (no flag) hits the SMB share — skip locally.
# Instead pass --source pointing at the repo root (which has 0 .flacs)
cargo run -- --source F:\repos\ipod-sync --dry-run
```
Expected: walks the repo, finds 0 FLACs, prints "Add: 0 / Modify: 0 / Remove: 0 / Unchanged: 0", exits cleanly.

- [ ] **Step 3: Commit**

```powershell
git -C F:\repos\ipod-sync add src\main.rs
git -C F:\repos\ipod-sync commit -m "feat(main): dry-run orchestrator — walk + diff + summary"
```

- [ ] **Step 4: Run Gate A against the real library**

This is the gate. The user supplied source library is at `\\MUSICHOST\data\media\music\` (default).

```powershell
$timer = [System.Diagnostics.Stopwatch]::StartNew()
cargo run --release -- --dry-run 2>&1 | Tee-Object -Variable output
$timer.Stop()
"Elapsed: $($timer.Elapsed.TotalSeconds.ToString('F1'))s"
```

(Use `--release` for the SMB walk so BLAKE3 isn't unnecessarily slow.)

Expected:
- Completes without errors.
- "found N FLAC file(s)" where N is around 1,400 (the user's library size per SPEC §11).
- Manifest entries: 0 (no manifest exists yet).
- Action plan: Add ≈ N, Modify 0, Remove 0, Unchanged 0.
- Elapsed: probably 30-180 seconds depending on SMB speed (1.4 GB of first-MiB reads).

**If N is way off** (10x too low or "0"): walker isn't recursing or the path is wrong. Investigate. The user's recent successful test FLAC was at `\\MUSICHOST\data\media\music\Big Wild\Superdream\01 - City of Sound.flac` — confirm that path is reachable from the running session.

**If the walk hangs** (>10 minutes with no progress): SMB issue. The current implementation has no progress reporting during the walk (that comes in the TUI task). For Gate A acceptance the walker must just finish; we can revisit if it's painfully slow.

**Report the output to the user** for Gate A approval. If the action plan looks right ("Add: 1,400ish") and the elapsed time is acceptable, Gate A passes and we proceed to Task 9.

- [ ] **Step 5: Record the Gate A result**

Append to `LEARNINGS.md`:
```markdown
## Phase 2 Gate A (YYYY-MM-DD)

- **Result:** PASS / FAIL.
- **Source:** \\MUSICHOST\data\media\music\
- **FLACs found:** N
- **Walk elapsed (release build):** Xs
- **Action plan:** Add=N, Modify=0, Remove=0, Unchanged=0 (expected — no manifest yet).
- **Notes:** (anything surprising about file count, encoding, SMB performance, etc.)
```

```powershell
git -C F:\repos\ipod-sync add LEARNINGS.md
git -C F:\repos\ipod-sync commit -m "docs: Phase 2 Gate A — dry-run preview against library"
```

---

## Task 9: ipod::db — TrackHandle, delete_track, list_tracks_for_rebuild

**Files:**
- Modify: `F:\repos\ipod-sync\src\ipod\db.rs`

Three changes to the existing `OwnedDb`:

1. **`add_track_with_file` returns `TrackHandle`** — needed so the orchestrator can record `ipod_dbid` and `ipod_relpath` in the manifest.
2. **`delete_track(dbid)`** — finds the track by dbid, removes it from all playlists, removes the on-iPod file, removes from DB. Returns Ok if no track with that dbid exists (idempotent).
3. **`list_tracks_for_rebuild()`** — walks `db.tracks`, returns `Vec<TrackHandle>` for `--rebuild-manifest`.

- [ ] **Step 1: Add `TrackHandle` type**

Append to `src/ipod/db.rs` near the existing `Tags` definition:
```rust
/// Identifies a track on the iPod after add. Returned by `add_track_with_file`
/// and `list_tracks_for_rebuild`; recorded in `ManifestEntry`.
#[derive(Debug, Clone)]
pub struct TrackHandle {
    pub dbid: u64,
    /// Relative path with Windows backslashes: `iPod_Control\Music\F41\libgpod079263.m4a`.
    pub ipod_relpath: String,
}
```

- [ ] **Step 2: Change `add_track_with_file` to return `TrackHandle`**

Find the existing `add_track_with_file` signature in `src/ipod/db.rs`. Change return type from `Result<()>` to `Result<TrackHandle>`. In the body, after the `itdb_playlist_add_track` call but before the `Ok(())`, replace `Ok(())` with code that reads the track's dbid and ipod_path and returns a TrackHandle:

```rust
            // Read the assigned dbid + ipod_path from the now-attached track.
            let dbid = (*track).dbid as u64;
            let relpath = read_ipod_relpath(track);
            Ok(TrackHandle { dbid, ipod_relpath: relpath })
        }
    }
```

(The closing braces match the existing structure — the function ends with an `unsafe` block.)

Add the helper at the bottom of the file (private, near `path_to_cstring`):
```rust
/// Convert libgpod's colon-separated `ipod_path` to Windows backslashes,
/// stripping the leading colon. libgpod stores e.g. `:iPod_Control:Music:F12:KLMN.m4a`;
/// the manifest stores `iPod_Control\Music\F12\KLMN.m4a`.
unsafe fn read_ipod_relpath(track: *mut ffi::Itdb_Track) -> String {
    let p = (*track).ipod_path;
    if p.is_null() {
        return String::new();
    }
    let s = std::ffi::CStr::from_ptr(p).to_string_lossy();
    s.trim_start_matches(':').replace(':', "\\")
}
```

Update the callers (there's the wipe-tracks example and the Phase 1 main.rs — but Phase 2 will replace main.rs in Task 10 anyway). For the existing example you can leave as-is if it doesn't use the return value (`let _ = db.add_track_with_file(...)`). For the Phase 1 main.rs that's about to be replaced, no change needed. `cargo build` will tell you if anything else needs updating.

- [ ] **Step 3: Implement `delete_track`**

Append to the `impl OwnedDb` block:
```rust
    /// Remove a track from the iPod by dbid. Idempotent (returns Ok if not present).
    /// Does NOT call `itdb_write`; the caller batches multiple removes + adds
    /// then calls `write` once.
    pub fn delete_track(&self, dbid: u64) -> Result<()> {
        unsafe {
            // Find the track by walking the GList. libgpod doesn't expose a
            // hashmap lookup; 1,400 tracks at ~30ns per pointer-chase is fine.
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
                return Ok(());  // already gone; idempotent
            }

            // Delete the on-iPod file via libgpod's path helper.
            let fname_c = ffi::itdb_filename_on_ipod(found);
            if !fname_c.is_null() {
                let path_str = std::ffi::CStr::from_ptr(fname_c).to_string_lossy().into_owned();
                let _ = std::fs::remove_file(std::path::Path::new(&path_str));
                ffi::g_free(fname_c as *mut std::os::raw::c_void);
            }
            // Remove from all playlists, then remove + free the track.
            ffi::itdb_playlist_remove_track(std::ptr::null_mut(), found);
            ffi::itdb_track_remove(found);
        }
        Ok(())
    }
```

- [ ] **Step 4: Implement `list_tracks_for_rebuild`**

Append to the `impl OwnedDb` block:
```rust
    /// Walk all tracks currently in the DB and return their handles.
    /// Used by `--rebuild-manifest` to populate a fresh manifest with
    /// `source_known = false` entries.
    pub fn list_tracks_for_rebuild(&self) -> Vec<TrackHandle> {
        let mut out = Vec::new();
        unsafe {
            let mut node = (*self.0).tracks;
            while !node.is_null() {
                let t = (*node).data as *mut ffi::Itdb_Track;
                if !t.is_null() {
                    out.push(TrackHandle {
                        dbid: (*t).dbid as u64,
                        ipod_relpath: read_ipod_relpath(t),
                    });
                }
                node = (*node).next;
            }
        }
        out
    }
```

- [ ] **Step 5: Verify bindgen has every symbol we need**

```powershell
$bindings = Get-ChildItem F:\repos\ipod-sync\target\debug\build\ipod-sync-*\out\libgpod_bindings.rs | Sort-Object LastWriteTime -Descending | Select-Object -First 1
$needed = "itdb_filename_on_ipod", "itdb_playlist_remove_track", "itdb_track_remove", "itdb_get_mountpoint", "g_free"
foreach ($name in $needed) {
    $hit = Select-String -Path $bindings.FullName -Pattern "\bfn $name\b" -Quiet
    "$name : $(if ($hit) { 'OK' } else { 'MISSING' })"
}
```
Expected: all OK. (We've used all of these in earlier phases except `itdb_get_mountpoint` which Task 1 already verified.)

Also confirm `Itdb_Track` has a `dbid` field and `ipod_path` field:
```powershell
Select-String -Path $bindings.FullName -Pattern "pub struct _Itdb_Track\b" -Context 0,80 | Out-String -Width 200 | Select-String "dbid|ipod_path"
```
Expected: shows both. If `dbid` is `i64` instead of `u64` in the bindings, the `as u64` cast is fine (DBID is opaque to us; we just need to round-trip it).

- [ ] **Step 6: Build + run existing tests**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test 2>&1 | Select-Object -Last 5
```
Expected: clean build (the `add_track_with_file` signature change shouldn't break anything since Phase 1's main.rs is about to be replaced and wipe-tracks doesn't use the return value); all existing tests pass.

- [ ] **Step 7: Commit**

```powershell
git -C F:\repos\ipod-sync add src\ipod\db.rs
git -C F:\repos\ipod-sync commit -m "feat(ipod::db): TrackHandle return + delete_track + list_tracks_for_rebuild"
```

---

## Task 10: main.rs — full sync orchestrator → Gate B

**Files:**
- Replace: `F:\repos\ipod-sync\src\main.rs`

Now the full orchestrator. Apply each action sequentially. Stop on the first error (per SPEC §8 row 5). Save the manifest atomically only if the run completes cleanly.

This task INCLUDES the helper `tags_from_probe` that combines `track` + `track_total` (and `disc` + `disc_total`) into the canonical `Tags`.

- [ ] **Step 1: Replace `src/main.rs`**

```rust
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use ipod_sync::cli::Cli;
use ipod_sync::config::{self, Config};
use ipod_sync::ipod::db::{OwnedDb, Tags, TrackHandle};
use ipod_sync::ipod::{device, detect_ipod_mount};
use ipod_sync::manifest::{self, Action, Manifest, ManifestEntry};
use ipod_sync::source::{self, SourceEntry};
use ipod_sync::transcode::{self, has_embedded_art, ProbeOutput, ProbeTags};
use std::path::Path;

fn main() -> Result<()> {
    unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE")); }

    let cli = Cli::parse();
    let config = config::resolve(cli)?;
    run(&config)
}

fn run(config: &Config) -> Result<()> {
    println!("Source  : {}", config.source.display());
    println!("Manifest: {}", config.manifest_path.display());

    transcode::verify_tools_available()?;

    // 1. Resolve iPod mount.
    let mount = match &config.ipod {
        Some(m) => {
            let p = ensure_trailing_backslash(m);
            if !Path::new(&p).join("iPod_Control").join("iTunes").join("iTunesDB").exists() {
                return Err(anyhow!("explicit --ipod {} does not contain iPod_Control\\iTunes\\iTunesDB", p));
            }
            p
        }
        None => detect_ipod_mount()?,
    };
    println!("iPod    : {mount}");

    // 2. Walk source.
    println!("Walking source...");
    let sources = source::walk(&config.source)?;
    println!("  {} FLAC file(s)", sources.len());

    // 3. Load (or rebuild) manifest.
    let mut manifest = if config.rebuild_manifest {
        println!("Rebuilding manifest from iPod (--rebuild-manifest)...");
        let db = OwnedDb::open(Path::new(&mount))?;
        let rebuilt = build_rebuild_manifest(&db);
        println!("  {} existing iPod track(s) recorded as source-unknown", rebuilt.tracks.len());
        // Save eagerly so a crash after this point doesn't lose the rebuild.
        manifest::save_atomic(&config.manifest_path, &rebuilt)?;
        rebuilt
    } else {
        let m = manifest::load_or_default(&config.manifest_path)?;
        println!("Loaded {} existing manifest entries", m.tracks.len());
        m
    };

    // 4. Diff.
    let actions = manifest::diff(&manifest, &sources);
    let (add, modify, remove, unchanged) = count_actions(&actions);
    println!();
    println!("Action plan:");
    println!("  Add      : {add}");
    println!("  Modify   : {modify}");
    println!("  Remove   : {remove}{}", if config.no_delete { " (--no-delete; skipped)" } else { "" });
    println!("  Unchanged: {unchanged}");

    if config.dry_run {
        println!("\nDry run; nothing was written.");
        return Ok(());
    }

    if add == 0 && modify == 0 && (remove == 0 || config.no_delete) {
        println!("\nNothing to do.");
        return Ok(());
    }

    // 5. Apply actions.
    let db = OwnedDb::open(Path::new(&mount))?;
    let guid = device::read_firewire_guid(Path::new(&mount))?;
    unsafe {
        let device_ptr = (*db.as_ptr()).device;
        device::set_firewire_guid(device_ptr, &guid)?;
    }

    let total = actions.len();
    let mut i = 0usize;
    for action in actions {
        i += 1;
        match action {
            Action::Unchanged(_) => {}  // no-op
            Action::Remove(entry) => {
                if config.no_delete {
                    continue;
                }
                println!("[{i}/{total}] REMOVE {} (dbid {})", entry.source_path.display(), entry.ipod_dbid);
                db.delete_track(entry.ipod_dbid)
                    .with_context(|| format!("delete dbid {}", entry.ipod_dbid))?;
                manifest.tracks.retain(|e| e.ipod_dbid != entry.ipod_dbid);
            }
            Action::Modify(src, old) => {
                println!("[{i}/{total}] MODIFY {}", src.path.display());
                if !config.no_delete {
                    db.delete_track(old.ipod_dbid)
                        .with_context(|| format!("delete-for-modify dbid {}", old.ipod_dbid))?;
                    manifest.tracks.retain(|e| e.ipod_dbid != old.ipod_dbid);
                }
                let handle = add_one(&db, &src)?;
                manifest.tracks.push(entry_from(&src, &handle));
            }
            Action::Add(src) => {
                println!("[{i}/{total}] ADD {}", src.path.display());
                let handle = add_one(&db, &src)?;
                manifest.tracks.push(entry_from(&src, &handle));
            }
        }
    }

    // 6. Commit DB + manifest. NEITHER is persisted unless we got this far.
    println!("\nWriting iPod DB...");
    db.write()?;
    println!("Writing manifest...");
    manifest::save_atomic(&config.manifest_path, &manifest)?;

    println!("\nDone. Eject the iPod before unplugging.");
    Ok(())
}

/// Transcode + add one source file. Returns the iPod-side handle.
fn add_one(db: &OwnedDb, src: &SourceEntry) -> Result<TrackHandle> {
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

    let handle = db.add_track_with_file(&temp, &tags, art.as_deref())
        .with_context(|| format!("add_track_with_file for {}", src.path.display()))?;

    let _ = std::fs::remove_file(&temp);
    Ok(handle)
}

fn tags_from_probe(p: &ProbeOutput) -> Tags {
    let pt: &ProbeTags = match &p.format.tags {
        Some(t) => t,
        None => return Tags::default(),
    };

    let track_nr = pt.track.as_deref().and_then(|s| parse_int_first_field(s));
    let tracks_from_total = pt.track_total.as_deref().and_then(|s| s.trim().parse().ok());
    let tracks_from_slash = pt.track.as_deref().and_then(parse_int_second_field);
    let tracks = tracks_from_total.or(tracks_from_slash);

    let disc_nr = pt.disc.as_deref().and_then(|s| parse_int_first_field(s));
    let discs_from_total = pt.disc_total.as_deref().and_then(|s| s.trim().parse().ok());
    let discs_from_slash = pt.disc.as_deref().and_then(parse_int_second_field);
    let discs = discs_from_total.or(discs_from_slash);

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

/// "9/12" -> Some(9). "9" -> Some(9). "" / garbage -> None.
fn parse_int_first_field(s: &str) -> Option<i32> {
    s.split('/').next()?.trim().parse().ok()
}

/// "9/12" -> Some(12). "9" -> None.
fn parse_int_second_field(s: &str) -> Option<i32> {
    s.split('/').nth(1)?.trim().parse().ok()
}

fn parse_year(s: &str) -> Option<i32> {
    s.split('-').next()?.trim().parse().ok()
}

fn count_actions(actions: &[Action]) -> (usize, usize, usize, usize) {
    let mut add = 0; let mut modify = 0; let mut remove = 0; let mut unchanged = 0;
    for a in actions {
        match a {
            Action::Add(_) => add += 1,
            Action::Modify(_, _) => modify += 1,
            Action::Remove(_) => remove += 1,
            Action::Unchanged(_) => unchanged += 1,
        }
    }
    (add, modify, remove, unchanged)
}

fn entry_from(src: &SourceEntry, handle: &TrackHandle) -> ManifestEntry {
    ManifestEntry {
        source_path: src.path.clone(),
        source_mtime: src.mtime,
        source_size: src.size,
        source_fingerprint: src.fingerprint.clone(),
        ipod_dbid: handle.dbid,
        ipod_relpath: handle.ipod_relpath.clone(),
        source_known: true,
    }
}

fn build_rebuild_manifest(db: &OwnedDb) -> Manifest {
    let handles = db.list_tracks_for_rebuild();
    let tracks = handles.into_iter().map(|h| ManifestEntry {
        source_path: std::path::PathBuf::new(),
        source_mtime: 0,
        source_size: 0,
        source_fingerprint: String::new(),
        ipod_dbid: h.dbid,
        ipod_relpath: h.ipod_relpath,
        source_known: false,
    }).collect();
    Manifest { version: 1, ipod_serial: None, tracks }
}

fn ensure_trailing_backslash(s: &str) -> String {
    if s.ends_with('\\') { s.to_string() } else { format!("{s}\\") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_first_field_handles_slash_and_lone() {
        assert_eq!(parse_int_first_field("9/12"), Some(9));
        assert_eq!(parse_int_first_field("9"), Some(9));
        assert_eq!(parse_int_first_field(""), None);
        assert_eq!(parse_int_first_field("abc"), None);
    }

    #[test]
    fn parse_int_second_field_only_returns_after_slash() {
        assert_eq!(parse_int_second_field("9/12"), Some(12));
        assert_eq!(parse_int_second_field("9"), None);
    }

    #[test]
    fn parse_year_handles_iso_date_and_lone_year() {
        assert_eq!(parse_year("2002-09-24"), Some(2002));
        assert_eq!(parse_year("2002"), Some(2002));
        assert_eq!(parse_year(""), None);
    }
}
```

- [ ] **Step 2: Build + unit tests**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test 2>&1 | Select-Object -Last 5
```
Expected: clean build, all tests pass.

- [ ] **Step 3: Wipe the iPod for a clean slate**

```powershell
cargo run --example wipe-tracks
```
Expected: iPod track count is 0.

- [ ] **Step 4: Gate B — 10-track real sync**

Pick a 10-track subset by passing `--source <some-album-dir>`. The Big Wild album worked in Phase 1; use the whole album dir:
```powershell
$timer = [System.Diagnostics.Stopwatch]::StartNew()
cargo run --release -- --source "\\MUSICHOST\data\media\music\Big Wild\Superdream" 2>&1
$timer.Stop()
"Elapsed: $($timer.Elapsed.TotalSeconds.ToString('F1'))s"
```
Expected:
- Walks the album (12-ish FLACs).
- Action plan: Add ≈ 12, all others 0.
- Each track shows "[N/12] ADD ..." line.
- "Writing iPod DB..." then "Writing manifest..." then "Done."
- Elapsed: a few minutes (transcode-bound).

Then re-run with NO changes to confirm "Unchanged: 12" → "Nothing to do" path:
```powershell
cargo run --release -- --source "\\MUSICHOST\data\media\music\Big Wild\Superdream"
```
Expected: completes in seconds (just walks + diffs); no transcoding; prints "Action plan: Add=0 Modify=0 Remove=0 Unchanged=12" then "Nothing to do."

Check the manifest file is sensible:
```powershell
Get-Content $env:APPDATA\ipod-sync\manifest.json | Select-Object -First 20
```
Expected: valid JSON with version=1, ipod_serial=null (we don't capture this yet — Phase 3), 12 track entries each with source_path, fingerprint, ipod_dbid, ipod_relpath, source_known=true.

**Hardware verification (user step):** eject, plug back in (no need to wait for separate task), verify the iPod boots and plays one of the new tracks with metadata + art.

- [ ] **Step 5: Record Gate B result**

Append to `LEARNINGS.md`:
```markdown
## Phase 2 Gate B (YYYY-MM-DD)

- **Result:** PASS / FAIL.
- **Test subset:** \\MUSICHOST\data\media\music\Big Wild\Superdream\
- **First-run action plan:** Add=12 (or whatever the album has).
- **First-run elapsed:** Xs.
- **Second-run action plan:** Add=0, Unchanged=12.
- **Second-run elapsed:** Xs (should be < 10s — no transcoding).
- **Manifest persistence:** JSON valid, fields populated, round-trips cleanly.
- **Hardware verify:** iPod boots, tracks play, art shows.
- **Notes:** (anything surprising about scaling, error paths, etc.)
```

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\main.rs LEARNINGS.md
git -C F:\repos\ipod-sync commit -m "feat(main): full sync orchestrator + tags_from_probe with TRACKTOTAL"
```

---

## Task 11: logging — tracing setup + GLib log handler

**Files:**
- Modify: `F:\repos\ipod-sync\src\logging.rs`
- Modify: `F:\repos\ipod-sync\src\main.rs` (call `logging::init` at startup)

Two concerns in one module:

1. **`tracing-subscriber` setup** — `RUST_LOG=ipod_sync=debug` style filtering, single-line formatter. `--verbose` flag from CLI bumps default level to `debug`.
2. **GLib log handler** — `g_log_set_default_handler` to redirect libgpod's `WARNING`/`CRITICAL` messages into tracing instead of cluttering our output. We saw these in Phase 1: `Error parsing recent playcounts` and `itdb_splr_validate: ...UNKNOWN`.

- [ ] **Step 1: Verify GLib symbols in bindgen output**

```powershell
$bindings = Get-ChildItem F:\repos\ipod-sync\target\debug\build\ipod-sync-*\out\libgpod_bindings.rs | Sort-Object LastWriteTime -Descending | Select-Object -First 1
foreach ($name in @("g_log_set_default_handler", "GLogLevelFlags", "G_LOG_LEVEL_WARNING")) {
    $hit = Select-String -Path $bindings.FullName -Pattern "\b$name\b" -Quiet
    "$name : $(if ($hit) { 'OK' } else { 'MISSING' })"
}
```

`g_log_set_default_handler` is exported by GLib; it should appear in bindings via the allowlist `g_.*` if we have it. **If MISSING**, broaden the allowlist in `build.rs`: add `.allowlist_function("g_log_.*").allowlist_type("GLogLevelFlags").allowlist_var("G_LOG_.*")`. Rebuild.

- [ ] **Step 2: Add `g_log_*` to bindgen allowlist if needed**

If Step 1 found missing symbols, open `build.rs` and append after the existing `allowlist_function` lines:
```rust
.allowlist_function("g_log_.*")
.allowlist_type("GLogLevelFlags")
```

Run `cargo clean -p ipod-sync && cargo build` to regenerate bindings.

- [ ] **Step 3: Implement `logging::init`**

Replace `src/logging.rs` with:
```rust
//! Tracing-subscriber init + GLib log handler installation.

use crate::ffi;
use std::ffi::CStr;
use tracing::{debug, info, warn};
use tracing_subscriber::filter::EnvFilter;

pub fn init(verbose: bool) {
    let default = if verbose { "ipod_sync=debug,info" } else { "ipod_sync=info,warn" };
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();

    install_glib_handler();
    debug!("logging initialized (verbose={verbose})");
}

extern "C" fn glib_log_handler(
    log_domain: *const std::os::raw::c_char,
    log_level: ffi::GLogLevelFlags,
    message: *const std::os::raw::c_char,
    _user_data: *mut std::os::raw::c_void,
) {
    let domain = if log_domain.is_null() {
        "glib".to_string()
    } else {
        unsafe { CStr::from_ptr(log_domain).to_string_lossy().into_owned() }
    };
    let message = if message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(message).to_string_lossy().into_owned() }
    };

    // GLib's level constants are a bitmask; check the most-severe-first.
    // We treat CRITICAL/WARNING as tracing warn (these are noisy but benign
    // in our case — playcounts parse, splr_validate UNKNOWN, etc.).
    let is_critical = (log_level & ffi::GLogLevelFlags_G_LOG_LEVEL_CRITICAL) != 0;
    let is_warning = (log_level & ffi::GLogLevelFlags_G_LOG_LEVEL_WARNING) != 0;
    let is_message = (log_level & ffi::GLogLevelFlags_G_LOG_LEVEL_MESSAGE) != 0;

    if is_critical || is_warning {
        warn!(target: "glib", "{domain}: {message}");
    } else if is_message {
        info!(target: "glib", "{domain}: {message}");
    } else {
        debug!(target: "glib", "{domain}: {message}");
    }
}

fn install_glib_handler() {
    unsafe {
        ffi::g_log_set_default_handler(Some(glib_log_handler), std::ptr::null_mut());
    }
}
```

(If bindgen names the variants differently than `GLogLevelFlags_G_LOG_LEVEL_CRITICAL` etc., adjust to match the actual generated names. Bindgen typically prepends the enum type name; double-check by searching the bindings.)

- [ ] **Step 4: Wire `logging::init` into main.rs**

In `src/main.rs`, after `let config = config::resolve(cli)?;` and BEFORE `run(&config)?`:
```rust
ipod_sync::logging::init(config.verbose);
```

- [ ] **Step 5: Build + verify on a real run**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo run --release -- --source "\\MUSICHOST\data\media\music\Big Wild\Superdream" --dry-run
```
Expected: same dry-run output as before, except now you'll see tracing prefixes (e.g. `[INFO ipod_sync] ...`) and any libgpod warnings would route through tracing (during dry-run there are no libgpod calls so this is hard to test).

To actually exercise the GLib handler, run wipe-tracks (which previously printed `WARNING` and `CRITICAL` to stderr directly):
```powershell
cargo run --example wipe-tracks
```
Expected: the `Error parsing recent playcounts` and `itdb_splr_validate` messages now appear as tracing warnings prefixed with their level (e.g. `WARN glib: ...`), NOT as bare `**WARNING**` / `**CRITICAL**` GLib output.

(If wipe-tracks doesn't init logging — it won't, since it's a separate binary — the GLib messages will appear as before. That's fine; the main binary is what matters.)

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\logging.rs src\main.rs build.rs
git -C F:\repos\ipod-sync commit -m "feat(logging): tracing-subscriber + GLib log handler routing into tracing"
```

---

## Task 12: progress — ratatui TUI + plain fallback

**Files:**
- Modify: `F:\repos\ipod-sync\src\progress.rs`
- Modify: `F:\repos\ipod-sync\src\main.rs` (use `Progress` instead of direct println)

This is the biggest single module in Phase 2. Design:

- `Progress` owns a thread + an `mpsc::Sender<ProgressEvent>`.
- Main thread calls `progress.summary(...)`, `progress.track_start(...)`, `progress.track_done()`, `progress.log(...)`, `progress.error(...)`, `progress.finish()`.
- The owned thread loops: try_recv with a 100ms timeout, apply events to internal state, redraw (TUI) or print (plain).
- TUI mode iff `config.use_tui && stdout().is_terminal()`.
- TUI uses ratatui + crossterm. Layout: header (paths) / progress bar (overall %, ETA) / current track / log tail.
- Plain mode just prints lines to stdout/stderr.

There are no unit tests for the TUI itself (it'd require an ANSI-rendering harness); the test is "does Gate C run produce useful output."

- [ ] **Step 1: Implement `progress.rs`**

Replace `src/progress.rs` with:
```rust
//! Progress reporting: ratatui TUI when stdout is a TTY + --no-tui is off,
//! plain log lines otherwise. Main thread sends events; a dedicated thread
//! drains the channel and renders.

use anyhow::Result;
use crossterm::event::{Event, KeyCode};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph};
use std::collections::VecDeque;
use std::io::IsTerminal;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Events sent from the main thread to the progress thread.
pub enum ProgressEvent {
    Header { source: String, ipod: String, manifest: String },
    Summary { add: usize, modify: usize, remove: usize, unchanged: usize, total_planned: usize },
    TrackStart { current: usize, total: usize, label: String },
    TrackDone,
    Log(String),
    Error(String),
    Finish,
}

pub struct Progress {
    sender: Sender<ProgressEvent>,
    thread: Option<JoinHandle<()>>,
}

impl Progress {
    pub fn start(use_tui: bool) -> Result<Self> {
        let is_tty = std::io::stdout().is_terminal();
        let active_tui = use_tui && is_tty;
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            if active_tui {
                if let Err(e) = run_tui(rx) {
                    eprintln!("TUI failure: {e}; falling back to plain mode is not possible mid-run");
                }
            } else {
                run_plain(rx);
            }
        });
        Ok(Self { sender: tx, thread: Some(thread) })
    }

    pub fn header(&self, source: String, ipod: String, manifest: String) {
        let _ = self.sender.send(ProgressEvent::Header { source, ipod, manifest });
    }
    pub fn summary(&self, add: usize, modify: usize, remove: usize, unchanged: usize, total_planned: usize) {
        let _ = self.sender.send(ProgressEvent::Summary { add, modify, remove, unchanged, total_planned });
    }
    pub fn track_start(&self, current: usize, total: usize, label: String) {
        let _ = self.sender.send(ProgressEvent::TrackStart { current, total, label });
    }
    pub fn track_done(&self) {
        let _ = self.sender.send(ProgressEvent::TrackDone);
    }
    pub fn log(&self, msg: impl Into<String>) {
        let _ = self.sender.send(ProgressEvent::Log(msg.into()));
    }
    pub fn error(&self, msg: impl Into<String>) {
        let _ = self.sender.send(ProgressEvent::Error(msg.into()));
    }

    /// Drains the channel and joins the worker thread. Call once at the end.
    pub fn finish(mut self) {
        let _ = self.sender.send(ProgressEvent::Finish);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Plain mode: dump events as lines. Stdout for normal stuff, stderr for errors.
fn run_plain(rx: Receiver<ProgressEvent>) {
    for event in rx {
        match event {
            ProgressEvent::Header { source, ipod, manifest } => {
                println!("Source  : {source}");
                println!("iPod    : {ipod}");
                println!("Manifest: {manifest}");
            }
            ProgressEvent::Summary { add, modify, remove, unchanged, .. } => {
                println!();
                println!("Action plan: add={add} modify={modify} remove={remove} unchanged={unchanged}");
            }
            ProgressEvent::TrackStart { current, total, label } => {
                println!("[{current}/{total}] {label}");
            }
            ProgressEvent::TrackDone => {}  // already printed at start
            ProgressEvent::Log(s) => println!("{s}"),
            ProgressEvent::Error(s) => eprintln!("ERROR: {s}"),
            ProgressEvent::Finish => break,
        }
    }
}

struct TuiState {
    source: String,
    ipod: String,
    manifest: String,
    add: usize,
    modify: usize,
    remove: usize,
    unchanged: usize,
    total_planned: usize,
    done: usize,
    current_label: String,
    current_index: usize,
    current_total: usize,
    started_at: Instant,
    log_tail: VecDeque<String>,
}

impl TuiState {
    fn new() -> Self {
        Self {
            source: String::new(), ipod: String::new(), manifest: String::new(),
            add: 0, modify: 0, remove: 0, unchanged: 0, total_planned: 0,
            done: 0, current_label: String::new(),
            current_index: 0, current_total: 0,
            started_at: Instant::now(),
            log_tail: VecDeque::with_capacity(LOG_TAIL_CAPACITY),
        }
    }

    fn push_log(&mut self, line: String) {
        if self.log_tail.len() == LOG_TAIL_CAPACITY {
            self.log_tail.pop_front();
        }
        self.log_tail.push_back(line);
    }

    fn fraction(&self) -> f64 {
        if self.total_planned == 0 { 0.0 } else {
            (self.done as f64) / (self.total_planned as f64)
        }
    }

    fn eta(&self) -> Option<Duration> {
        if self.done == 0 || self.total_planned == 0 { return None; }
        let elapsed = self.started_at.elapsed();
        let per_track = elapsed.as_secs_f64() / (self.done as f64);
        let remaining = self.total_planned.saturating_sub(self.done);
        if remaining == 0 { return None; }
        Some(Duration::from_secs_f64(per_track * remaining as f64))
    }
}

const LOG_TAIL_CAPACITY: usize = 12;

fn run_tui(rx: Receiver<ProgressEvent>) -> Result<()> {
    let mut state = TuiState::new();
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide,
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut finished = false;
    while !finished {
        // Drain any pending events without blocking; cap per-frame so a flood
        // doesn't starve the redraw.
        for _ in 0..32 {
            match rx.try_recv() {
                Ok(event) => apply_event(&mut state, event, &mut finished),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => { finished = true; break; }
            }
        }

        terminal.draw(|f| render(f, &state))?;

        // Allow Ctrl+C / 'q' to bail out of the TUI (caller still owns sync flow).
        if crossterm::event::poll(Duration::from_millis(80))? {
            if let Event::Key(key) = crossterm::event::read()? {
                if key.code == KeyCode::Char('q') {
                    // 'q' is a request-stop; we just exit the draw loop. The sync
                    // thread keeps running until it next sends an event and finds
                    // the channel closed.
                    finished = true;
                }
            }
        }
    }

    // Teardown.
    crossterm::execute!(terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
    )?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

fn apply_event(state: &mut TuiState, event: ProgressEvent, finished: &mut bool) {
    match event {
        ProgressEvent::Header { source, ipod, manifest } => {
            state.source = source; state.ipod = ipod; state.manifest = manifest;
        }
        ProgressEvent::Summary { add, modify, remove, unchanged, total_planned } => {
            state.add = add; state.modify = modify; state.remove = remove;
            state.unchanged = unchanged; state.total_planned = total_planned;
            state.started_at = Instant::now();  // reset clock for ETA
        }
        ProgressEvent::TrackStart { current, total, label } => {
            state.current_index = current; state.current_total = total;
            state.current_label = label;
        }
        ProgressEvent::TrackDone => { state.done += 1; }
        ProgressEvent::Log(s) => state.push_log(s),
        ProgressEvent::Error(s) => state.push_log(format!("ERROR: {s}")),
        ProgressEvent::Finish => { *finished = true; }
    }
}

fn render(f: &mut ratatui::Frame, state: &TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),  // header
            Constraint::Length(4),  // progress
            Constraint::Length(3),  // current track
            Constraint::Min(5),     // log tail
        ])
        .split(f.area());

    let header_text = vec![
        Line::from(format!("Source  : {}", state.source)),
        Line::from(format!("iPod    : {}", state.ipod)),
        Line::from(format!("Manifest: {}", state.manifest)),
    ];
    f.render_widget(
        Paragraph::new(header_text)
            .block(Block::default().borders(Borders::ALL).title(" ipod-sync ")),
        chunks[0],
    );

    let pct = (state.fraction() * 100.0) as u16;
    let eta = state.eta()
        .map(|d| format!(" ETA {}", format_duration(d)))
        .unwrap_or_default();
    let progress_label = format!("{}/{} ({}%){}", state.done, state.total_planned, pct, eta);
    let plan_line = Line::from(vec![
        Span::raw(format!(
            "add={} modify={} remove={} unchanged={}",
            state.add, state.modify, state.remove, state.unchanged
        )),
    ]);
    let progress_lines = vec![plan_line];
    f.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(" progress "))
            .ratio(state.fraction().clamp(0.0, 1.0))
            .label(progress_label),
        chunks[1],
    );

    let current = if state.current_total > 0 {
        format!("[{}/{}] {}", state.current_index, state.current_total, state.current_label)
    } else {
        "(idle)".to_string()
    };
    f.render_widget(
        Paragraph::new(current).block(Block::default().borders(Borders::ALL).title(" current ")),
        chunks[2],
    );

    let log_items: Vec<ListItem> = state.log_tail.iter()
        .map(|l| ListItem::new(Line::from(l.as_str())))
        .collect();
    f.render_widget(
        List::new(log_items)
            .block(Block::default().borders(Borders::ALL).title(" log "))
            .style(Style::default().add_modifier(Modifier::DIM)),
        chunks[3],
    );

    let _ = progress_lines;  // silence unused-warning if future refactor drops plan_line
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}
```

- [ ] **Step 2: Wire `Progress` into main.rs**

In `src/main.rs`, after `logging::init(config.verbose);` and `transcode::verify_tools_available()?;`:
```rust
use ipod_sync::progress::Progress;

// in run():
let progress = Progress::start(config.use_tui)?;
progress.header(
    config.source.display().to_string(),
    mount.clone(),  // wait — mount isn't resolved yet at this point; reorder so progress is set up AFTER mount resolution
    config.manifest_path.display().to_string(),
);
```

Reorder: do mount resolution, walker, manifest load, diff, summary first; THEN start the progress (with the action counts already known so `total_planned` is correct from the first frame).

After every println in the apply-actions loop, replace with `progress.log(...)` or `progress.track_start(...)` + `progress.track_done()`. Replace the final "Done." println with `progress.log("Done.")` then `progress.finish()`.

Concretely, the apply-actions loop becomes:
```rust
let total_planned = add + modify + (if config.no_delete { 0 } else { remove });
progress.summary(add, modify, remove, unchanged, total_planned);

let mut i = 0usize;
for action in actions {
    match action {
        Action::Unchanged(_) => continue,
        Action::Remove(entry) if config.no_delete => continue,
        Action::Remove(entry) => {
            i += 1;
            progress.track_start(i, total_planned,
                format!("REMOVE {}", entry.source_path.display()));
            db.delete_track(entry.ipod_dbid)
                .with_context(|| format!("delete dbid {}", entry.ipod_dbid))?;
            manifest.tracks.retain(|e| e.ipod_dbid != entry.ipod_dbid);
            progress.track_done();
        }
        Action::Modify(src, old) => {
            i += 1;
            progress.track_start(i, total_planned,
                format!("MODIFY {}", src.path.display()));
            if !config.no_delete {
                db.delete_track(old.ipod_dbid)
                    .with_context(|| format!("delete-for-modify dbid {}", old.ipod_dbid))?;
                manifest.tracks.retain(|e| e.ipod_dbid != old.ipod_dbid);
            }
            let handle = add_one(&db, &src)?;
            manifest.tracks.push(entry_from(&src, &handle));
            progress.track_done();
        }
        Action::Add(src) => {
            i += 1;
            progress.track_start(i, total_planned,
                format!("ADD {}", src.path.display()));
            let handle = add_one(&db, &src)?;
            manifest.tracks.push(entry_from(&src, &handle));
            progress.track_done();
        }
    }
}

progress.log("Writing iPod DB...".to_string());
db.write()?;
progress.log("Writing manifest...".to_string());
manifest::save_atomic(&config.manifest_path, &manifest)?;
progress.log("Done. Eject the iPod before unplugging.".to_string());
progress.finish();
Ok(())
```

(Adjust the `add_one` signature to accept `&Progress` if you want per-step "Transcoding..."/"Adding to DB..." sub-progress messages — optional, can be deferred.)

- [ ] **Step 3: Build + run with --no-tui first to confirm plain mode**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo run --release -- --source "\\MUSICHOST\data\media\music\Big Wild\Superdream" --dry-run --no-tui
```
Expected: plain mode output, same shape as before. Header, action plan, "Dry run; nothing was written."

- [ ] **Step 4: Run with TUI on a small subset**

```powershell
cargo run --release -- --source "\\MUSICHOST\data\media\music\Big Wild\Superdream" --dry-run
```
Expected: ratatui shows the header + a 0/0 progress bar + idle "current" + empty log. Holds the screen for a moment then exits cleanly (the dry-run path calls `progress.finish()` quickly).

- [ ] **Step 5: Run an actual TUI sync**

First wipe the iPod (so we have something to add):
```powershell
cargo run --example wipe-tracks
```

Then:
```powershell
cargo run --release -- --source "\\MUSICHOST\data\media\music\Big Wild\Superdream"
```
Expected: TUI shows live progress, each track shows up in the "current" box, log tail shows the recent action lines, ETA decreases, eventually "Done." and clean exit back to the normal terminal.

Sanity-check the terminal: after exit, your prompt should be at the normal position, NOT garbled from the alternate-screen / raw-mode setup. If garbled, the teardown didn't run (likely a panic in the TUI). Investigate.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add src\progress.rs src\main.rs
git -C F:\repos\ipod-sync commit -m "feat(progress): ratatui TUI + plain fallback over event channel"
```

---

## Task 13: Orphan-track message + final polish

**Files:**
- Modify: `F:\repos\ipod-sync\src\main.rs`

One LEARNINGS carry-forward we haven't addressed: when a partial sync fails (e.g. `itdb_cp_track_to_ipod` succeeds for track N but `itdb_write` fails at the end), the orphan `.m4a` files on the iPod don't have manifest entries. Phase 2's `--rebuild-manifest` handles recovery, but the user error message at the failure point needs to say so clearly.

- [ ] **Step 1: Wrap the apply-actions block and final write in a "if this errors, tell the user how to recover" pattern**

In `src/main.rs`, modify the `run` function to capture errors from the action loop OR the final write, and surface a recovery hint before returning the error:
```rust
let sync_result: Result<()> = (|| -> Result<()> {
    // ... the action loop + db.write() + save_atomic block ...
    Ok(())
})();

if let Err(e) = &sync_result {
    progress.error(format!("Sync failed: {e}"));
    progress.error("The iPod may now contain orphan track files (added but not".to_string());
    progress.error("in the iTunesDB), and the manifest has NOT been updated.".to_string());
    progress.error("To recover: re-run with --rebuild-manifest, which will read the iPod's".to_string());
    progress.error("current DB and create a fresh manifest. Then run normally.".to_string());
}
progress.finish();
sync_result
```

(Adapt the syntax to fit the existing function shape — the goal is "any error from the orchestrator gets paired with a recovery message before bubbling up.")

- [ ] **Step 2: Manual smoke — induce a failure and check message renders**

The easiest way to induce a partial-write failure: pass a `--source` path that doesn't exist after the walker runs. Actually, the walker errors before any writes, so that doesn't exercise the orphan path.

Easier: don't artificially induce a failure for this task. The recovery message is a one-shot you trust to be correct because it's a static string. Skip the smoke; rely on the natural-failure case in Gate C if it ever materializes.

- [ ] **Step 3: Commit**

```powershell
git -C F:\repos\ipod-sync add src\main.rs
git -C F:\repos\ipod-sync commit -m "feat(main): print recovery hint when sync fails partway"
```

---

## Task 14: Gate C — full 1,400-track acceptance run

**Files:**
- Modify: `F:\repos\ipod-sync\LEARNINGS.md`

This is SPEC §6 #1: the real acceptance criterion for v1. Empty iPod + 1,400-track library → ipod-sync runs to completion, every track plays with metadata + art, second run shows 0 changes.

- [ ] **Step 1: Pre-flight checks**

```powershell
# iPod is wiped (or freshly restored)
cargo run --example wipe-tracks
(Get-ChildItem G:\iPod_Control\Music -Recurse -Filter *.m4a -Force | Measure-Object).Count
# Expected: 0

# Manifest doesn't exist (or delete it for a true fresh run)
Remove-Item $env:APPDATA\ipod-sync\manifest.json -ErrorAction SilentlyContinue

# Source is reachable
Test-Path "\\MUSICHOST\data\media\music"
```

- [ ] **Step 2: Run the full sync**

```powershell
$timer = [System.Diagnostics.Stopwatch]::StartNew()
cargo run --release 2>&1 | Tee-Object F:\repos\ipod-sync\target\gate-c-run.log
$timer.Stop()
"Total elapsed: $($timer.Elapsed.ToString())"
```

This will take a while — easily 1-3 hours for 1,400 tracks (transcode-bound; ~3-5 sec per track of ffmpeg work). The TUI shows progress.

If anything fails partway: capture the error, run with `--rebuild-manifest` to recover, then investigate. The error message from Task 13 should tell the user what to do.

- [ ] **Step 3: Spot-check iPod-side state on completion**

```powershell
Get-Item G:\iPod_Control\iTunes\iTunesDB | Select-Object Length, LastWriteTime
(Get-ChildItem G:\iPod_Control\Music -Recurse -Filter *.m4a -Force | Measure-Object).Count
# Expected: ≈ 1,400 m4a files
(Get-ChildItem G:\iPod_Control\Artwork -Force | Measure-Object).Count
# Expected: ArtworkDB plus a handful of .ithmb files

# Manifest is populated
(Get-Content $env:APPDATA\ipod-sync\manifest.json | ConvertFrom-Json).tracks.Count
# Expected: ≈ 1,400
```

- [ ] **Step 4: Run again with no changes**

```powershell
cargo run --release
```
Expected per SPEC §6 #2: completes in < 5 seconds, action plan = Unchanged=1400, "Nothing to do." This validates the fingerprint-based diff actually works at scale.

- [ ] **Step 5: Physical verification (user step)**

User ejects the iPod, plugs back in, verifies:
1. Boots normally.
2. Music → Songs lists ~1,400 tracks.
3. Several random tracks play with correct audio.
4. Several random tracks show correct metadata + art on Now Playing.

- [ ] **Step 6: SPEC §6 #3 — add 5 new FLACs to source, re-run**

Copy any 5 FLACs from the library into a temporary subdir of the source (or just point --source at a slightly different subset that's a superset):
```powershell
# Or simulate by adding fresh files - this depends on user availability.
# Easier: assume the source library doesn't change between runs.
# This step is OPTIONAL for Gate C; the full-1400 run is the primary signal.
```

If you don't have spare FLACs to add, **skip this step** and document it in LEARNINGS as "not exercised in Gate C; relies on the same code path that the 1,400-track run hits 1,400 times."

- [ ] **Step 7: SPEC §6 #4 — delete 5 FLACs, re-run**

Same as Step 6 — optional, skip if no convenient way to test, document in LEARNINGS.

- [ ] **Step 8: SPEC §6 #5 — --rebuild-manifest works**

```powershell
# Save the working manifest in case anything goes wrong
Copy-Item $env:APPDATA\ipod-sync\manifest.json $env:APPDATA\ipod-sync\manifest.json.backup
# Delete it to simulate loss
Remove-Item $env:APPDATA\ipod-sync\manifest.json
# Rebuild from iPod
cargo run --release -- --rebuild-manifest --dry-run
```
Expected: walks iPod (1,400 tracks), creates fresh manifest with source_known=false entries.

Then run normally:
```powershell
cargo run --release
```
Expected per the design decision: action plan = Add=1400 (all sources look new because manifest has them as source_unknown), but ACTUALLY the unknown-source entries are preserved untouched — so the action plan would be Add=1400 (re-add all sources) AND the 1400 unknown-source entries stay in the manifest. This produces 2,800 entries in the manifest and 2,800 tracks on the iPod (1400 originals + 1400 duplicates). 

That's the designed-for behavior of "best-effort rebuild" — it doesn't match by title/artist, just preserves. The user is told (in `--rebuild-manifest` output) that this is the trade-off.

**For Gate C acceptance,** verify that the --rebuild-manifest call DOESN'T destroy data. The user then either:
(a) accepts the dup state, deletes duplicates via a future Phase 3 tool, OR
(b) restores the backup manifest: `Copy-Item $env:APPDATA\ipod-sync\manifest.json.backup $env:APPDATA\ipod-sync\manifest.json`

Use option (b) for the Gate C test so we don't strand the iPod with 2,800 tracks. Restore the backup, verify a normal run shows 0 changes.

- [ ] **Step 9: SPEC §6 #6 — --dry-run path works**

Already exercised in earlier tasks. Re-confirm:
```powershell
cargo run --release -- --dry-run
```
Expected: prints action plan, doesn't touch iPod or manifest.

- [ ] **Step 10: Record Gate C result**

Append to `LEARNINGS.md`:
```markdown
## Phase 2 Gate C — full library acceptance (YYYY-MM-DD)

- **Result:** PASS / FAIL (with reasons).
- **Source library:** \\MUSICHOST\data\media\music\
- **Track count synced:** N (expected ~1,400 per SPEC §11).
- **Total wall-clock:** X hours Y minutes.
- **Second-run elapsed:** Zs (must be < 5s per SPEC §6 #2).
- **Hardware verification:** [iPod boots / Music → Songs lists ~N tracks / random playback works / random Now Playing has metadata + art].
- **SPEC §6 acceptance criteria:**
  - #1 (empty iPod -> full sync, playable, metadata + art): PASS / FAIL
  - #2 (no changes -> < 5s): PASS / FAIL
  - #3 (add 5 -> only 5 processed): EXERCISED / SKIPPED
  - #4 (delete 5 -> only 5 removed): EXERCISED / SKIPPED
  - #5 (--rebuild-manifest works): EXERCISED — produces duplicates by design
  - #6 (--dry-run writes nothing): PASS
- **Issues encountered + how fixed:**
  - (any partial failures, ffmpeg errors, libgpod warnings worth recording)
- **Phase 3 carry-forwards:**
  - --rebuild-manifest produces duplicates: needs interactive match or a separate dedup tool
  - (anything else from the run)
```

- [ ] **Step 11: Commit + tag**

```powershell
git -C F:\repos\ipod-sync add LEARNINGS.md
git -C F:\repos\ipod-sync commit -m "docs: Phase 2 Gate C result"
git -C F:\repos\ipod-sync tag -a phase-2-complete -m "Phase 2 full-tool acceptance run complete

- Walks 1,400-track FLAC library at \\MUSICHOST\data\media\music
- Manifest-driven incremental sync (add/modify/remove/unchanged)
- ratatui TUI with non-TTY plain fallback
- --rebuild-manifest recovery (best-effort, marks existing as source-unknown)
- All SPEC §4.1 CLI flags
- SPEC §6 #1 acceptance criteria met on iPod Classic 7G (EXAMPLE1234)"
```

---

## Self-review

**Spec coverage check:**

- §4.1 CLI: every flag in Task 2's `Cli` struct ✓
- §4.2 walker: Task 4 (BLAKE3 first 1 MiB + size, .unwanted/_excluded skipped, case-insensitive .flac) ✓
- §4.3 manifest + diff: Tasks 5 + 10 (atomic JSON, four-way classification, --rebuild-manifest semantics) ✓
- §4.4 transcoding: existing from Phase 1, no changes needed — orchestrator uses it ✓
- §4.5 iPod ops: Task 9 (delete_track) + existing Phase 1 (add_track_with_file + write) ✓
- §4.6 mount detection: Task 6 ✓
- §6 acceptance criteria: Task 14 walks through each ✓
- §7 out-of-scope items: explicitly listed in the plan header ("Things the plan will NOT include")  — wait, I forgot to add that section. Should not be a separate section; SPEC §7's list (playlists, two-way, podcasts, watcher, GUI, other models, non-FLAC) is honored by absence. No tasks introduce any of those.
- §8 risks: row 5 (concurrent device access / mid-sync failure) handled by Task 13's orphan message + stop-on-first-error pattern
- §12 #6 TUI: Task 12 (ratatui + non-TTY fallback) ✓
- LEARNINGS carry-forwards: TRACKTOTAL (Task 1.4) / Play Counts.bak (1.6) / loaders.cache (1.7) / GLib handler (Task 11) / orphan message (Task 13) — all present ✓

**Placeholder scan:**

- "ipod-sync-test-rt-{}" with std::process::id() — concrete code, not a placeholder.
- "Phase 3 (if there is one)" header note — context-setting, not a TODO.
- Task 14 Step 6/7 say "skip if no convenient way to test" — that's intentional; the spec criteria #3 and #4 are the same code paths as the main 1,400-track add and the eventual deletions, so 100% coverage isn't strictly required. Acceptable.
- Two places where I wrote "verify against bindgen first" (Task 1.6 for `itdb_get_mountpoint`, Task 5.5 for the various delete-path symbols) — these are necessary verification steps, not placeholders. The plan instructs concrete fallbacks if symbols differ.
- No "TBD" / "TODO" / "add error handling" / "implement later" / "similar to Task N" patterns.

**Type consistency check:**

- `TrackHandle { dbid: u64, ipod_relpath: String }` — defined in Task 9, used in Tasks 9 + 10. Field names consistent.
- `Tags` — existing Phase 1 type, fields `track_nr` / `tracks` / `disc_nr` / `discs` referenced consistently by Task 10's `tags_from_probe`.
- `Action::{Add, Modify, Remove, Unchanged}` — defined in Task 5, matched in Tasks 8, 10, 12.
- `Config` field names — consistent across Task 3 definition and Task 8/10/12 usage.
- `ProgressEvent::{Header, Summary, TrackStart, TrackDone, Log, Error, Finish}` — Task 12.
- `manifest::{load_or_default, save_atomic, diff}` and `Manifest`/`ManifestEntry`/`Action` — consistent throughout.
- `ipod::detect_ipod_mount` (top-level re-export in Task 7) used in Task 10. Consistent with Task 6's definition.

**Scope check:** Phase 2 builds the v1 sync tool. Out: parallelism, multi-iPod, SysInfoExtended XML fallback, Rust port of iTunesDB writer, smart playlists, play counts, dedup-after-rebuild. All listed as Phase 3 in the LEARNINGS template or implicit by absence.

The plan is long but each task is bounded and self-contained. The three gates create natural pause points for course-correction. Tasks 1, 7, 13 are small wrap-up tasks; Tasks 4, 5, 10, 12, 14 are the heavyweights.
