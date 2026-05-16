# ipod-sync — Specification

A Windows-native CLI tool that syncs a FLAC music library to an iPod Classic, transcoding to Apple Lossless (ALAC) on the fly with embedded album art preserved.

This document is a handoff spec. An implementer agent should be able to build the tool from this without needing prior context from the discovery conversation.

---

## 1. Problem

The owner has a large FLAC library on a Windows file server (`\\server\music\`) and wants to sync it to an iPod Classic (7th gen, 160GB). Constraints, ranked:

1. **Windows-native** — single `.exe`, no WSL, no browser, no iTunes runtime dependency.
2. **On-the-fly transcoding** — FLAC → ALAC happens during sync; no permanent local ALAC mirror on disk.
3. **Album art preserved** — embedded artwork in source FLACs must appear on the iPod.
4. **Incremental** — re-runs only process new/changed/deleted files.
5. **Reliable** — must not corrupt the iPod's `iTunesDB`.

Existing tools that were considered and rejected during discovery (do not re-evaluate unless something below proves infeasible):

- **iTunes + MusicBee + iPod plugin chain** — works for some users; brittle in practice on Windows 11 + modern iTunes. Failed in our testing.
- **MediaMonkey** — requires iTunes for Classic sync per its own docs. Same fragility profile as MusicBee.
- **TunesReloaded** ([tunesreloaded.com](https://tunesreloaded.com/)) — closest to what we want; sidesteps iTunes via WebUSB + libgpod-WASM. Disqualifier: no album art support yet.
- **CopyTrans Manager / iMazing** — commercial; would work but the owner wants a scriptable native tool.

## 2. Tech stack

| Component | Choice | Rationale |
|---|---|---|
| Language | Rust (stable, MSVC toolchain) | Owner's preference. Gives single self-contained `.exe`. Good FFI story for the C dep. |
| iTunesDB read/write | **libgpod** via FFI | Decades-old, well-tested, handles the hashed `iTunesDB` on late-model Classics including the FirewireGUID-derived signature. Reinventing this format is a data-corruption risk we're not taking. |
| FLAC decode + ALAC encode + container muxing | **shell out to `ffmpeg.exe`** | ffmpeg already handles FLAC→ALAC with album-art passthrough cleanly. Avoids pulling FFmpeg into the build. Owner will need ffmpeg on PATH or supplied via `--ffmpeg` flag. |
| In-memory pipeline | Windows named pipe (`\\.\pipe\…`) | Stream ffmpeg's stdout to libgpod's file-open path via a named pipe. Avoids touching disk for a transcoded file. Fallback: temp file in `%TEMP%` if pipe approach proves incompatible with libgpod's expectations of seekable input. |
| CLI parsing | `clap` (derive macros) | Standard. |
| Recursive file walk | `walkdir` | Standard. |
| State manifest | `serde_json` + a single JSON file at `%APPDATA%\ipod-sync\manifest.json` | Tracks per-source-file: path, mtime, size, hash (BLAKE3), iPod track DBID, iPod relative file path. |
| Logging | `tracing` + `tracing-subscriber` | Structured logs, easy levels. |
| Progress UI | `indicatif` | Console progress bar, ETA, throughput. |
| Error handling | `anyhow` for app errors, `thiserror` for typed errors at module boundaries | Standard idiom. |

## 3. Architecture

```
                                   ┌──────────────────────────┐
                                   │ ipod-sync.exe (Rust)     │
                                   │                          │
\\server\music\**.flac ───────────►│ 1. Source walker         │
                                   │ 2. Diff vs manifest      │
                                   │ 3. For each delta:       │
                                   │    ├─ spawn ffmpeg.exe   │───►  named pipe ───►  in-memory ALAC bytes
                                   │    ├─ via libgpod-sys:   │
                                   │    │   itdb_cp_track…    │───────────────────►  G:\iPod_Control\Music\Fnn\xxxx.m4a
                                   │    │   add track to DB   │
                                   │    │   itdb_write        │───────────────────►  G:\iPod_Control\iTunes\iTunesDB (hashed)
                                   │    └─ update manifest    │
                                   │ 4. Handle deletions      │
                                   │ 5. Print summary         │
                                   └──────────────────────────┘
```

### 3.1 Modules

```
src/
├── main.rs            // CLI entry, top-level orchestration
├── cli.rs             // clap definitions
├── config.rs          // resolved runtime config (paths, options)
├── manifest.rs        // sync state: load/save/diff
├── source.rs          // FLAC source walker + change detection (mtime+size+hash)
├── ipod/
│   ├── mod.rs         // high-level: open, sync_track, remove_track, close+write_db
│   ├── ffi.rs         // raw bindgen-generated FFI to libgpod
│   ├── db.rs          // safe Rust wrappers over libgpod for iTunesDB ops
│   ├── files.rs       // iPod_Control\Music\Fnn\ path generation (4-char names, 50-folder distribution)
│   └── device.rs      // mount detection, model identification, FirewireGUID read from SysInfoExtended
├── transcode.rs       // ffmpeg invocation, named pipe wiring, art preservation flags
└── progress.rs        // indicatif wrappers
```

## 4. Key behaviors

### 4.1 CLI

```
ipod-sync [--source <path>] [--ipod <drive>] [--ffmpeg <path>]
          [--dry-run] [--no-delete] [--verbose] [--rebuild-manifest]
```

- `--source` — FLAC root. Default: `\\server\music\` (override via env var `IPOD_SYNC_SOURCE` or config file in future iteration).
- `--ipod` — iPod drive letter, e.g. `G:`. Default: auto-detect any mounted drive containing `\iPod_Control\iTunes\iTunesDB`. If multiple matches, error and require explicit flag.
- `--ffmpeg` — path to `ffmpeg.exe`. Default: `ffmpeg` (look up via PATH).
- `--dry-run` — compute and print actions, write nothing.
- `--no-delete` — never remove tracks from iPod, even if removed from source.
- `--verbose` — `tracing` level = `debug`.
- `--rebuild-manifest` — ignore existing manifest, reconcile by reading the iPod's current iTunesDB. Used when manifest is lost/stale.

### 4.2 Source walker

- Recursively walk `--source` for `*.flac` (case-insensitive).
- For each file: capture path, mtime, size, BLAKE3 of first 1 MiB + size (fast content fingerprint — full hashing 1400 files at ~30 MiB each is wasteful).
- Skip files in `.unwanted` or `_excluded` subfolders (convention for owner to mark "don't sync this").
- Build a `SourceEntry { path, mtime, size, fingerprint }` set.

### 4.3 Manifest diff

`%APPDATA%\ipod-sync\manifest.json`:

```json
{
  "version": 1,
  "ipod_serial": "EXAMPLE1234",
  "tracks": [
    {
      "source_path": "\\\\server\\music\\Beck\\Sea Change\\1-09 Already Dead.flac",
      "source_mtime": 1700000000,
      "source_size": 28349123,
      "source_fingerprint": "blake3:…",
      "ipod_dbid": 12345678901234,
      "ipod_relpath": "iPod_Control\\Music\\F12\\KLMN.m4a"
    }
  ]
}
```

Diff classifies each source file as one of:

- **Unchanged**: matches manifest by path + size + mtime + fingerprint. Skip.
- **New**: present in source, absent from manifest.
- **Modified**: same path, different fingerprint. Replace (delete + add new).
- **Removed**: present in manifest, absent from source. Delete unless `--no-delete`.

Manifest is the source of truth for what we've synced; the iPod's DB is the source of truth for what's playable. After every successful operation, manifest is rewritten atomically (write tempfile, fsync, rename).

### 4.4 Transcoding

ffmpeg invocation per track:

```
ffmpeg -loglevel error -i <source.flac> \
       -map 0:a -map 0:v? \
       -c:a alac \
       -c:v copy \
       -disposition:v attached_pic \
       -f ipod \
       \\.\pipe\ipod-sync-<uuid>
```

- `-map 0:a -map 0:v?` — copy audio + cover-art stream if present (the `?` makes the video mapping optional so files without art don't error).
- `-c:a alac` — Apple Lossless encoder, ffmpeg native.
- `-c:v copy -disposition:v attached_pic` — pass embedded artwork through as MP4 cover atom.
- `-f ipod` — forces MP4/M4A container with iPod-compatible muxing constraints.

**Critical**: MP4 container requires `moov` atom placement. ffmpeg's default writes `moov` at end of file, which needs seekable output. A named pipe is *not* seekable. Add `-movflags +empty_moov+frag_keyframe` to write streaming-friendly fragmented MP4, OR fall back to a temp file in `%TEMP%` per track.

**Recommendation for v1**: use temp file in `%TEMP%\ipod-sync\` per track, processed one at a time. Delete immediately after libgpod has copied it to the iPod. This is "in-memory enough" — no permanent staging, no full mirror — while sidestepping the pipe-seekability problem. If perf measurement shows temp-file IO is a bottleneck, optimize to named pipe + fragmented MP4 in v2.

### 4.5 iPod file & DB operations (via libgpod)

For each new/modified track, in order:

1. `Itdb_Track *track = itdb_track_new()` then populate metadata from ffprobe output of the source FLAC (title, artist, album, year, track number, etc.) — ffmpeg's `-loglevel quiet -i input.flac` writes metadata to stderr; easier to invoke `ffprobe -of json -show_format -show_streams input.flac` and parse.
2. `itdb_cp_track_to_ipod(track, "<temp_alac.m4a>", &error)` — libgpod handles file copy into `iPod_Control\Music\Fnn\` with correct 4-char name + 50-folder distribution.
3. `itdb_playlist_add_track(itdb_playlist_mpl(itdb), track, -1)` — add to master playlist.
4. Album art: read embedded picture from source FLAC (use `metaflac.exe`, ffprobe, or parse the METADATA_BLOCK_PICTURE ourselves), then `itdb_track_set_thumbnails_from_data(track, data, len)`.
5. Record `track->dbid` and the relative file path in the manifest entry.

After all per-track operations, call `itdb_write(itdb, &error)` ONCE at the end to flush the database, hash signing included.

For removals: `itdb_playlist_remove_track`, `itdb_track_remove`, `itdb_cp_track_from_ipod`-equivalent deletion of the on-device file (delete the file at `track->ipod_path` translated to Windows path).

### 4.6 Mount detection

- Enumerate drive letters (`A:` through `Z:`).
- For each drive, test existence of `<drive>:\iPod_Control\iTunes\iTunesDB`.
- If found, confirm it's an iPod Classic by reading `<drive>:\iPod_Control\Device\SysInfoExtended` and checking the model number.
- Extract `FirewireGuid` from `SysInfoExtended` (XML plist) — libgpod needs this to compute the hash signature on write for late-model Classics.

## 5. Build setup

### 5.1 Toolchain

- Rust stable, `x86_64-pc-windows-msvc` target.
- Visual Studio Build Tools 2022 (for MSVC linker + Windows SDK).
- vcpkg (for GLib).

### 5.2 libgpod build (one-time)

libgpod sources: https://sourceforge.net/projects/gtkpod/files/libgpod/ (or its git mirror). Active mirror: https://github.com/fadingred/libgpod.

Steps for the implementer:
1. `vcpkg install glib:x64-windows`
2. Clone libgpod, apply any Windows-specific patches found in the gtkpod community (search the gtkpod-devel archives if build fails).
3. Build with meson (libgpod supports meson as of recent versions) targeting MSVC, producing `libgpod.dll` + `libgpod.lib` + headers.
4. Place artifacts in `vendor/libgpod/{include,lib,bin}` within this project.

`build.rs` instructs cargo to:
- `println!("cargo:rustc-link-search=native=vendor/libgpod/lib")`
- `println!("cargo:rustc-link-lib=dylib=gpod")`
- Generate FFI bindings via `bindgen` from `vendor/libgpod/include/gpod/itdb.h`.
- Emit instruction to copy `libgpod.dll` to the output directory next to `ipod-sync.exe` for distribution.

### 5.3 Open question — prebuilt libgpod for Windows

The implementer should first search for an existing Windows build of libgpod before building from source. Candidates:
- gtkpod's old Windows installers (may include libgpod.dll usable standalone)
- MSYS2 packages (`mingw-w64-x86_64-libgpod` if it exists)
- Builds from contributors on Linux audio forums or the Hydrogenaudio community

If a usable prebuilt exists, skip §5.2.

## 6. Acceptance criteria

The tool is "v1 done" when:

1. Run against an empty (freshly restored) iPod Classic with a 1,400-track FLAC library at `\\server\music\`, the tool:
   - Completes without errors.
   - Populates the iPod with all tracks playable on the device's native menu.
   - Each track shows correct title/artist/album/year metadata.
   - Each track shows embedded album art on the iPod's "Now Playing" screen.
   - The iPod still boots normally and plays after the sync.

2. Run again with no source changes: completes in < 5 seconds with "0 changes" output.

3. Add 5 new FLACs to source, re-run: only those 5 are processed.

4. Delete 5 FLACs from source, re-run: those 5 disappear from the iPod's library and their files are removed from `iPod_Control\Music\Fnn\`.

5. Re-run with `--rebuild-manifest`: reads the iPod's current DB, reconciles to a fresh manifest matching reality, then a normal run produces "0 changes."

6. Run with `--dry-run`: prints the action plan, writes nothing to manifest, iPod, or temp.

## 7. Out of scope (v1)

- Playlist sync (smart or static).
- Two-way sync of ratings, play counts, skip counts.
- Podcast / audiobook / video sync.
- File-watcher daemon (auto-trigger on source change). Future.
- GUI. CLI only.
- Other iPod models (Touch, Shuffle, iPhone). The libgpod calls used are Classic-friendly; behavior on other models is untested and unsupported in v1.
- Non-FLAC sources (MP3/AAC/etc. pass-through). Source must be FLAC.

## 8. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Prebuilt libgpod.dll for modern Windows doesn't exist; building from source has unforeseen friction | Medium | High (blocks the project) | Spend a max of 1 day on the build before falling back: write the iTunesDB Rust port from scratch (significant scope expansion — would need its own spec) OR use TunesReloaded's WASM via wasmtime. |
| iPod Classic 7th gen 160GB hashed iTunesDB signature is wrong → iPod refuses to recognize tracks | Medium | High | libgpod is supposed to handle this when given the FirewireGUID. Verify with a small "write one track, eject, plug back in" test before processing the full library. |
| ffmpeg cover-art passthrough produces an MP4 container that iPod rejects | Low | Medium | Test with a single track first; if artwork breaks playback, switch to writing art via libgpod's `itdb_track_set_thumbnails_from_data` instead of in-band MP4 atom. |
| Owner's network share latency over SMB makes the source walk slow on first run | Low | Low | Acceptable; show progress. Future optimization: parallel hashing. |
| Concurrent device access — owner unplugs mid-sync | Low | High | Wrap every write op in a fail-safe: log the action, attempt the op, on I/O error abort, do NOT update manifest. Next run picks up where it left off. |

## 9. Testing

- **Unit tests** for the manifest diff logic, path/name generation for `Fnn` folders, ffmpeg command construction, ffprobe metadata parsing.
- **Integration test** behind `#[cfg(feature = "ipod-integration")]` that runs against a real iPod plugged in at a configured drive letter. Operator-run, not CI.
- **Golden file test** for iTunesDB writes: write a minimal DB with 3 known tracks, hex-dump it, compare against a known-good reference DB created by iTunes for the same iPod. Catches hash/format regressions.

## 10. Repository layout

```
F:\ipod-sync\
├── SPEC.md                 (this file)
├── README.md               (user-facing: install + use)
├── Cargo.toml
├── build.rs
├── src/                    (per §3.1)
├── vendor/
│   └── libgpod/            (built artifacts; gitignored)
└── tests/
    ├── manifest_diff.rs
    ├── ffmpeg_cmd.rs
    └── golden_itunesdb.rs
```

## 11. Handoff notes for the implementer

- Discovery context: the owner went through ~half a day of fighting iTunes 12.13 / MusicBee plugins / driver hell on Windows 11 before landing on "build it from scratch, native, Rust." Don't suggest going back to those tools.
- Owner is technical, comfortable with Rust, runs Windows 11 on AMD hardware (AMD USB 3.10 controllers). Test target: iPod Classic 7th gen 160GB, serial `EXAMPLE1234`, currently formatted via iTunes 12.6.5.3 (Windows-formatted FAT32, freshly restored, empty).
- ffmpeg is presumed installed (verify with `ffmpeg -version` on PATH before first use; emit a helpful error if missing).
- Source library is ~1,400 albums of FLAC at `\\server\music\` over SMB. ~58 GB worth on the iPod after ALAC conversion (per a MusicBee sync preview that did make it that far).
- The `--rebuild-manifest` flow is critical if the owner ever loses `%APPDATA%\ipod-sync\manifest.json` — it's the recovery path that avoids re-syncing the whole library.

## 12. Implementation decisions & sequencing (2026-05-17 addendum)

Decisions made during brainstorming, layered on top of §1–§11:

1. **Transcoding output**: temp file in `%TEMP%\ipod-sync\` per track, one at a time, deleted immediately after `itdb_cp_track_to_ipod`. Named-pipe + fragmented MP4 is explicitly out of scope for v1 (revisit only if profiling shows temp-file IO as a real bottleneck).
2. **Metadata extraction**: standardize on `ffprobe -of json -show_format -show_streams` for tags. Single tool, structured output, parseable with `serde_json`. Do not also wire `metaflac.exe`.
3. **Album art path**: primary path is ffmpeg in-band passthrough (`-c:v copy -disposition:v attached_pic` inside `-f ipod`). Fallback to `itdb_track_set_thumbnails_from_data` *only* if device testing shows the in-band MP4 atom breaks playback. Build one path, hold the fallback in reserve — do not pre-implement both.
5. **Transcoder choice**: ffmpeg is the primary. Reasons: single process per track (decode + encode + mux + art passthrough in one invocation), already a stated dependency. **refalac64** (from the qaac distribution — wraps Apple's reference ALAC encoder) is the documented fallback if Phase 1 device testing reveals the iPod Classic rejecting ffmpeg's ALAC output. The refalac path costs more orchestration: separate libFLAC dep, separate art-extraction step (`--artwork <file>`), two-step pipeline per track. Do not pre-implement both — switch only on evidence.
6. **UI library — supersedes SPEC §2 table row "Progress UI"**: use **`ratatui`** + `crossterm` for an interactive TUI in Phase 2 (overall progress, current track, recent errors, log tail, throughput). The `indicatif` row in §2 is retired. Constraint: detect non-TTY stdout (e.g. `IsTerminal` from `std::io`) and fall back to plain `tracing` log output — never enter the alternate screen buffer when piped, redirected, or run under CI. A `--no-tui` flag should also force the plain path. TUI is irrelevant to Phase 0 (no UI) and likely irrelevant to Phase 1 (single track end-to-end); it lands in Phase 2.
4. **Repo path**: actual location is `F:\repos\ipod-sync\` (SPEC §10 said `F:\ipod-sync\`). Treat §10 layout as relative to the repo root.

### 12.1 Build sequence

Phase 0 and Phase 1 are gates — if Phase 0 fails, the project's foundation is wrong and we re-plan before writing more code.

- **Phase 0 — libgpod spike (gate)**: obtain a usable Windows `libgpod` (DLL + import lib + headers), either prebuilt or built from source per §5.2. Write a minimal Rust binary that opens the connected iPod's `iTunesDB`, lists tracks, and exits cleanly. No ffmpeg, no manifest, no CLI polish. If this can't be made to work within ~1 day of focused effort, escalate per Risk §8 row 1 before sinking further time.
- **Phase 1 — end-to-end single track (gate)**: hard-coded source FLAC → ffmpeg → temp ALAC → `itdb_cp_track_to_ipod` → metadata populated → art attached → `itdb_write` → eject → physically verify on the device that the track plays with correct tags and art shown on Now Playing. Validates the entire write path against the real hashed DB on real hardware.
- **Phase 2 — full tool**: source walker, manifest diff, deletions, `--dry-run`, `--no-delete`, `--rebuild-manifest`, mount auto-detect, progress UI, structured logs, error handling, full 1,400-track acceptance run per §6.
