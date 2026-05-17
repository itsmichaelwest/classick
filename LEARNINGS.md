# Learnings — ipod-sync

Per global CLAUDE.md: record discovered conventions, gotchas, debugging insights, and useful commands here as work proceeds. One bullet per learning.

## Phase 2 Gate B (2026-05-17)

- **Result:** PASS.
- **Test subset:** `<source-library-path>\Big Wild\Superdream\` (12 FLACs).
- **First-run action plan:** Add=12, Modify=0, Remove=0, Unchanged=0.
- **First-run elapsed:** 23.7s (release build; ~2.0s per track including transcode + cp + DB add). Much faster than the plan's "a few minutes" estimate — release-mode transcode + a 12-track album fits comfortably under 30s on this hardware.
- **Second-run action plan:** Add=0, Modify=0, Remove=0, Unchanged=12.
- **Second-run elapsed:** 0.8s (walk + fingerprint + diff; no transcoding).
- **Manifest persistence:** JSON valid, all 12 entries have non-zero `ipod_dbid`, backslashed `ipod_relpath` like `iPod_Control\Music\F08\libgpod712455.m4a`, `source_known=true`, full UNC `source_path`. Round-trips cleanly across the no-op second run.
- **GLib noise observed:** `WARNING: Error parsing recent playcounts` (open) and `CRITICAL: itdb_splr_validate: assertion 'at != ITDB_SPLAT_UNKNOWN' failed` (write) — both benign and expected; Task 11 will route these through tracing.
- **ffprobe duplicate-key bug surfaced and fixed:** Picard-tagged FLACs frequently emit BOTH `TRACKTOTAL` and `TOTALTRACKS` (and `DISCTOTAL`/`TOTALDISCS`). The original `#[derive(Deserialize)]` with `#[serde(alias = ...)]` rejects this as a duplicate field. Replaced with a manual `Deserialize` for `ProbeTags` that lowercases keys and applies first-write-wins. Added regression test `probe_output_handles_duplicate_synonymous_keys`.

## Phase 2 Task 4 — source walker + BLAKE3 fingerprint (2026-05-18)

- **PID-based temp dir shared across parallel tests causes flaky failures.** The plan's `tempdir_under_target()` generates `walker-<pid>` — identical across all tests in one run. Rust test harness runs tests in parallel by default; tests clobber each other's files. Fix: add an `AtomicU32` counter to produce `walker-<pid>-<n>` (unique per test invocation). One-liner fix; zero API impact.

## Phase 0

- **bindgen + libclang on Windows (Task 5):** VS18 Community ships `clang-format.exe` and `clang-tidy.exe` under `VC\Tools\Llvm\x64\bin` but does NOT include `clang.exe` or `libclang.dll`. bindgen 0.72 needs `libclang.dll` to parse C headers. Install the full LLVM toolchain via `winget install --id LLVM.LLVM` (drops it at `C:\Program Files\LLVM\`). Either add `C:\Program Files\LLVM\bin` to `PATH` or set `LIBCLANG_PATH=C:\Program Files\LLVM\bin` for cargo.
- **bindgen needs GLib include paths (Task 5):** `vendor/libgpod/include/gpod/itdb.h` includes `<glib.h>` and `<glib-object.h>`. Those live under `C:/msys64/mingw64/include/glib-2.0` and `C:/msys64/mingw64/lib/glib-2.0/include` (the second has `glibconfig.h`). `build.rs` adds both via `.clang_arg("-I...")`. Without these bindgen errors out on the very first include.
- **bindgen 0.72 allowlist for the spike (Task 5):** Allowlist `itdb_.*`, `Itdb_.*`, `ITDB_.*`, `g_error_.*`, `GError`, `GList`. `GError` and `g_error_*` are pre-added so Task 6 doesn't have to revisit `build.rs`. `GList` is needed for walking the track list in Task 6.
- **`Itdb_Track` type name (Task 5):** bindgen 0.72 generates `Itdb_Track` (matching the C typedef) directly under the `ffi` module — no mangling. `size_of::<ffi::Itdb_Track>()` on x86_64-pc-windows-msvc with this libgpod build = **640 bytes**.
- **build.rs DLL copy is load-bearing for `cargo run`:** Without copying `vendor/libgpod/bin/*.dll` into `target/<profile>/` at build time, `cargo run` fails immediately with "gpod.dll was not found". The current `build.rs` copies the full closure (16 DLLs: gpod.dll + 15 MinGW/GLib runtime DLLs).
- **build.rs target dir must come from `OUT_DIR` ancestors, not `CARGO_MANIFEST_DIR/target/$PROFILE`:** `CARGO_TARGET_DIR` (or `[build] target-dir` in `.cargo/config.toml`) relocates the real target tree. Computing it from the manifest dir copies DLLs into the wrong place. `OUT_DIR = <real_target>/<profile>/build/<pkg>-<hash>/out`, so `out_dir.ancestors().nth(3)` yields `<real_target>/<profile>` reliably.
- **bindgen allowlist `allowlist_type("Itdb_.*")` covers most types but misses the smart-playlist enums (`ItdbSPLMatch`, `ItdbLimitType`, `ItdbLimitSort`, `ItdbSPLField`) because they lack the underscore after `Itdb`. If/when Phase 1+ touches smart playlists, broaden to `allowlist_type("Itdb.*")` or add explicit entries.

## libgpod acquisition research (2026-05-17)

### Searches conducted

- **MSYS2**: Not found — `packages.msys2.org/search?q=libgpod` returned zero results as of 2026-05-16. No `mingw-w64-x86_64-libgpod`, `mingw-w64-ucrt-x86_64-libgpod`, or any variant exists in the MSYS2 package database. Confirmed by checking the MSYS2 GitHub repo `msys2/MINGW-packages` via `gh api` search (no results).
- **gtkpod SourceForge**: Last libgpod source release is v0.8.3 in the `libgpod-0.8` folder (folder last modified 2013-09-04). No Windows binaries, DLLs, or installers found in any subfolder (`libgpod-0.8`, `libgpod-unstable`, `libgpod-0.7.9x`, `libgpod-0.7.2`, `libgpod-0.7.0`, `libgpod-0.6.0`). The `libgpod` root was last touched 2011-01-03.
- **GitHub (fadingred/libgpod and forks)**: `fadingred/libgpod` — no Releases, no Windows artifacts, Unix autotools only. `gtkpod/libgpod` — no Releases published. `strawberrymusicplayer/strawberry-libgpod` — has a CMakeLists.txt (added 2021-08-19) but no Releases, no Windows binaries. The CMakeLists.txt uses GCC-only flags (`-std=c99`, `-Wall`, `-Wmissing-declarations`, etc.) that are incompatible with MSVC cl.exe. `jburton/libgpod`, `hyperair/libgpod`, `gerion0/libgpod` — no Windows artifacts in any.
- **vcpkg port**: Does **not exist** — confirmed via `gh api repos/microsoft/vcpkg/contents/ports` search and `vcpkg.io/en/packages.html?query=libgpod` (no results). There is no `libgpod` port in the vcpkg curated registry as of May 2026 (2807 total ports).
- **Strawberry MSVC build chain**: `strawberrymusicplayer/strawberry-msvc-build-tools` explicitly sets `-DENABLE_GPOD=OFF` in both debug and release CMake configurations. The `strawberry-msvc-dependencies` releases (most recent: tag 3520, 2026-05-16) contain no libgpod. This is the most active Windows MSVC music-player dependency chain and it deliberately excludes libgpod.
- **Forum / contributor builds**: Strawberry forum thread about libgpod+iPod on Windows discussed only macOS/Linux. No Hydrogenaudio or other community contributor with a known-working Windows MSVC libgpod recipe found via web search for 2022–2026.

### Candidates considered

- `strawberrymusicplayer/strawberry-libgpod` (CMake fork, last commit 2021-08-19): Has a CMakeLists.txt that could theoretically be built on Windows, but uses GCC-only compiler flags, requires GLib/GModule/GObject/libplist/SQLite/zlib all pre-built for MSVC, and has never been released as a Windows binary. Would require patching the CMakeLists.txt and sourcing all transitive MSVC deps. Not viable as a prebuilt.
- Any MSYS2 MinGW build (hypothetical): Even if one were built, it would link against the MinGW runtime, not UCRT/MSVC CRT, making it incompatible for use from an MSVC-compiled Rust binary without a very careful ABI boundary analysis.
- Building from source with autotools + MSYS2/MinGW cross-toolchain: Possible but produces MinGW-linked DLLs, which introduce runtime mismatch risk with `cargo build --target x86_64-pc-windows-msvc`.

### Decision: Branch B — Build from source

- **Reason:** No prebuilt libgpod for Windows x64 exists anywhere (MSYS2, SourceForge, GitHub Releases, vcpkg) as of May 2026; even the most active Windows MSVC music-player project (Strawberry) explicitly disables libgpod support on Windows.
- **Next action:** Proceed to Task 4 — build from source. The recommended path is to use the `strawberrymusicplayer/strawberry-libgpod` CMake fork as the source base (it has already eliminated the autotools dependency), patch the CMakeLists.txt to replace GCC-only flags with MSVC-compatible equivalents, and hand-build its transitive dependencies (GLib, libplist, SQLite, zlib) either via vcpkg (all four are available vcpkg ports) or the strawberry-msvc-dependencies tarball. There is no vcpkg port for libgpod itself, so a custom CMake build step in the repo (vendored under `vendor/libgpod/`) is the cleanest path.

## Task 6 spike — open iTunesDB and list tracks (2026-05-17)

- **`itdb_parse_file` is the right symbol for a known DB file path.** bindgen 0.72 exposes both `itdb_parse(mp, error)` (takes mount path, e.g. `G:\`) and `itdb_parse_file(filename, error)` (takes the full file path to `iTunesDB`). The spike uses `itdb_parse_file` per the plan. Either would have worked on a properly-mounted iPod, but `itdb_parse_file` is the lower-friction choice when you already know the DB path.
- **FirewireGUID was NOT needed for read.** Plain `itdb_parse_file` on the iPod Classic 7G (`EXAMPLE1234`) DB returned a valid `Itdb_iTunesDB *` with `tracks` populated. The SPEC §8 row 2 risk (hashed DB signature blocking parse) did not materialize for reads. Whether it bites on *write* (Phase 1) is still unknown — verifying the hashed signature is a write-side concern in libgpod, not a read-side one. Plan for needing `itdb_device_set_sysinfo` or env-var FirewireGUID setup before the first `itdb_write` call.
- **`Itdb_Track` field names verified live.** `title`, `artist`, `album` (all `*mut gchar`) — accessed via `(*track).title` etc. in `main.rs`. Names match the C header exactly; bindgen did not mangle.
- **`Itdb_iTunesDB::tracks` is a `*mut GList`.** Walked with `node = (*node).next` and `track = (*node).data as *mut Itdb_Track`. `_GList { data, next, prev }` layout confirmed in the bindings (`prev` unused for forward iteration).
- **`g_error_free` requires a separate import lib.** It lives in `libglib-2.0-0.dll`, not `gpod.dll`. The first link attempt failed with `LNK2019: unresolved external symbol g_error_free`. Fix: generated `vendor/libgpod/lib/glib.lib` via `dumpbin /exports libglib-2.0-0.dll` + `lib /def /machine:x64` (same pattern used for `gpod.lib` in Task 3 Step 10) and added `cargo:rustc-link-lib=dylib=glib` to `build.rs`. The `.def` has 1912 exports. Other glib symbols Phase 1 may need (e.g. `g_list_*`, `g_free`) are already covered by this single import lib.
- **libgpod emits non-fatal GLib WARNING on stderr during parse.** Saw `** (process:NNNN): WARNING **: hh:mm:ss.xxx: Error parsing recent playcounts` — likely because the freshly-restored iPod has no `Play Counts` companion file yet. Parse succeeded anyway. For end-user output in Phase 2, consider installing a `g_log_set_handler` to suppress or reformat these.
- **Read-only invariant holds.** After `cargo run`, `Get-ChildItem G:\iPod_Control -Recurse -File | Where-Object LastWriteTime -gt (Get-Date).AddMinutes(-30)` returned empty. `itdb_parse_file` + walk + `itdb_free` does not touch the iPod filesystem.
- **Live spike output (1 track on device):**
  ```
  Opening iTunesDB at: G:\iPod_Control\iTunes\iTunesDB
    [1] Beck — Colors — Colors
  Total tracks: 1
  ```

## Phase 1 design notes (carried from Task 6 spike review)

- **Wrap `Itdb_iTunesDB *` in a RAII type before Phase 1 grows error paths.** The Task 6 spike used a bare pointer with manual `itdb_free` at the end. Currently safe because no `?` operators between open and free — but every error return Phase 1 adds becomes a potential leak. Pattern:
  ```rust
  struct OwnedDb(*mut ffi::Itdb_iTunesDB);
  impl Drop for OwnedDb { fn drop(&mut self) { unsafe { ffi::itdb_free(self.0) }; } }
  ```
  Apply the same pattern to `Itdb_Track *` if Phase 1 holds tracks outside libgpod's internal lists.
- **Use `itdb_tracks_number(db)` for track counts** rather than walking the GList manually. Faster, single source of truth, and avoids the spike's `count`-vs-`printed` ambiguity (the spike's null-data defensive check creates a latent inconsistency that's invisible at 1 track and would be confusing at 1,400).
- **`CString::new(path.to_str().unwrap())` is unsound for arbitrary paths.** The spike gets away with it because `IPOD_MOUNT` is a const. Phase 1 takes paths from CLI args and directory walks where non-UTF-8 is possible. Use `.to_str().ok_or_else(|| anyhow!("path contains non-UTF-8: {}", path.display()))?` instead. Same for `CString::new` itself — a path containing interior NUL bytes (impossible on Windows but reachable via crafted input) would panic on `?`.
- **Rename `cstr_or_empty` → `cstr_to_string_or_default` when promoting it to a helper module.** Current name implies empty string but it returns `"<none>"`. Phase 1 likely wants the caller to choose the default (`fn cstr_to_owned(p: *mut c_char) -> Option<String>` returning `None` on null is cleaner).

## Phase 0 gate (2026-05-17) — PASS

- **Result:** PASS — all four acceptance criteria met.
- **libgpod build provenance:** Branch B — built from source via MSYS2/MinGW from `fadingred/libgpod` commit `4a8a33ef4bc58eee1baca6793618365f75a5c3fa` with two patches (`vendor/libgpod/patches/`). libplist + SQLite/iTunesCDB path stripped per SPEC §7 (nano 5G+ out of scope). See `vendor/libgpod/BUILD-NOTES.md` for full reproduction.
- **Acceptance checks:**
  - `cargo clean && cargo build` from clean checkout: PASS (13.87s).
  - `cargo run` on iPod at `G:\`: prints `Total tracks: 1` and `[1] Beck — Colors — Colors`. Matches what was synced via iTunes 12.6.5.3.
  - Read-only invariant verified: zero files modified under `G:\iPod_Control\` after the spike ran.
  - iPod post-eject state: boots normally, plays the listed track.
- **Hashed iTunesDB signature (SPEC §8 row 2) on read:** **NOT triggered.** libgpod parses the Classic 7G's hashed DB without needing FirewireGUID setup on the read path. Risk remains open for the write path — Phase 1 must verify before the first `itdb_write` (consider `itdb_device_set_sysinfo` or env-var FirewireGUID; libgcrypt is vendored and ready to sign).
- **Phase 1 starting state:** see `## Phase 1 design notes` section above for the four carry-forward design items from the spike code review (RAII drop guard for `Itdb_iTunesDB`, `itdb_tracks_number`, CString/path safety, helper naming). The vendored libgpod + glib import libs cover the FFI surface for Phase 1 with no further `lib /def` work expected for tag/write APIs (`itdb_track_*`, `itdb_playlist_*`, `itdb_cp_track_to_ipod`, `itdb_write`).
- **ffmpeg FLAC art embedding (Task 3 smoke test):** `ffmpeg -i audio.flac -i art.png -map 0:a -map 1:v -c:a copy -c:v png -disposition:v:0 attached_pic out.flac` correctly embeds art as FLAC PICTURE block. `-attach` and `-f lavfi` one-shot approaches don't work; the two-input `-map` approach is the correct recipe. The `?` in `-map 0:v?` (in `ffmpeg_args`) correctly makes art optional — audio-only FLACs transcode cleanly without it.
- **Installed ffmpeg is Gyan.dev full build (ffmpeg 8.0.1)** — includes ALAC encoder (`alac` native), confirmed working for FLAC→ALAC transcoding into `-f ipod` container with PNG art passthrough.

## Phase 1 Task 5 — ipod::db OwnedDb / write path (2026-05-18)

- **`Itdb_Track` field names match plan exactly:** `title`, `artist`, `album`, `albumartist`, `genre`, `composer`, `year`, `track_nr`, `tracks`, `cd_nr`, `cds` — all present, all named as the C header has them. No bindgen mangling. `apply_tags` in `src/ipod/db.rs` writes them directly.
- **`g_strdup` / `g_free` ARE present in bindings as of allowlist update in Task 4 build.rs** even though they are `#define` macros in modern glib headers (`gstrfuncs.h`/`gmem.h`). bindgen 0.72 picks up the underlying function declarations declared by `GLIB_AVAILABLE_IN_ALL void (g_free)(gpointer)`. If a future glib bump drops the function form entirely, fall back to declaring `extern "C" { pub fn g_free(...); pub fn g_strdup(...); }` in `src/ffi.rs` (the DLL exports them either way — verified in `vendor/libgpod/lib/glib.def` at lines 453 and 1344).
- **Build-output staleness gotcha:** there can be multiple `target/debug/build/ipod-sync-<hash>/out/libgpod_bindings.rs` directories from prior dependency-version churn. `Get-ChildItem ... | Select-Object -First 1` picked an old one missing recent allowlist entries. `cargo build` regenerates only into the *current* hash directory, so trust `cargo build`'s redefinition errors (E0428) over manual `Select-String` checks against the wrong file.
- **gboolean check convention:** `itdb_write` and `itdb_cp_track_to_ipod` return GLib `gboolean` (bindgen `i32`). Failure is `== 0`, not Rust-style `!success`. Always pair with `gerror_to_anyhow(api, err)` to extract the GError message and free it.
- **`itdb_cp_track_to_ipod` ownership transfer is "on success only":** the call adds the track to `db.tracks` only when it returns TRUE. On failure (return == 0) we still own the freshly-`itdb_track_new`'d pointer and must `itdb_track_free` it before propagating the error — otherwise it leaks. On success, the track is owned by the DB and will be freed transitively by `itdb_free` in `OwnedDb::drop` — manual `itdb_track_free` would be a double-free.

## Phase 1 album art Plan B — pixbuf gap (2026-05-17)

- **Vendored libgpod has NO gdk-pixbuf support.** `gpod.dll` (built per `BUILD-NOTES.md`) only imports `libglib-2.0-0.dll`, `libgmodule-2.0-0.dll`, `libgobject-2.0-0.dll`, `libintl-8.dll`, `KERNEL32.dll`, `msvcrt.dll`, `libxml2-16.dll`, `zlib1.dll` — verified via `llvm-objdump -p`. No `libgdk_pixbuf-2.0-0.dll`. Confirmed because MSYS2 `mingw-w64-x86_64-gdk-pixbuf2` was not in the build dependencies and `./configure` was not given `--with-gdk-pixbuf` (or its auto-detect path).
- **Consequence:** `itdb_track_set_thumbnails_from_data`, `itdb_track_set_thumbnails_from_file` (a.k.a. `itdb_track_set_thumbnails`), and `itdb_track_set_thumbnails_from_pixbuf` are all exported as symbols but return `FALSE` (0) at runtime without setting a `GError`. The libgpod 0.8.x source conditionally compiles the body on `HAVE_GDKPIXBUF`; without it, the function is a stub.
- **Reproduction:** `cargo run -- "...City of Sound.flac"` with `art_bytes = Some(124919 bytes)` errored with `itdb_track_set_thumbnails_from_data failed`. iPod state unchanged (run aborted before `itdb_write`).
- **Two fix options for the next session:**
  1. **Rebuild libgpod with `--with-gdk-pixbuf`** and ship `libgdk_pixbuf-2.0-0.dll` plus its transitive deps (`libpng`, `libjpeg`, `libtiff`, `libwebp`, `libheif`, `libffi-7`/`libffi-8`, possibly more) in `vendor/libgpod/bin/`. Adds ~10–15 DLLs. Pixbuf needs its loaders module path set at runtime via `GDK_PIXBUF_MODULEDIR` env var — another wrinkle to handle in `build.rs` or main.
  2. **Bypass pixbuf entirely:** decode JPG in Rust (e.g. `image` crate), resize to the iPod Classic 7G's thumb sizes (200x200 + 720x720 from `ipod_artwork_capabilities` in libgpod source, or whatever `itdb_device_get_artwork_formats` reports for this device), convert to the F1024 format (RGB565 little-endian for Classic 7G's primary thumb), then construct `Itdb_Thumb_Ipod_Item` / call `itdb_artwork_set_thumbnail_from_data` after artwork allocation. This works because the no-pixbuf path can still write raw pre-decoded bytes — but only via the `itdb_artwork_*` API set, not the high-level `itdb_track_set_thumbnails_*` API. Bigger code surface, no DLL re-bundling.
- **Plumbing wired up regardless:** `src/transcode.rs::extract_cover_art` + `temp_art_path`, `src/ipod/db.rs::add_track_with_file` signature now accepts `Option<&[u8]>`, `src/main.rs` extracts art from the FLAC via ffmpeg and passes it through. The `itdb_track_set_thumbnails_from_data` call site is correct — just blocked on the lib gap. Either fix above can re-use the orchestration unchanged.

## Phase 1 Task 4 — ipod::device (2026-05-17)

- **Target iPod uses flat-text `SysInfo`, NOT `SysInfoExtended` XML.** The iPod Classic 7G (MB029, drive-modded 160 GB) has `iPod_Control\Device\SysInfo` (no extension) with line-oriented `Key: value` content, not an XML plist. `SysInfoExtended` does not exist on this device. The parser is a trivial `split_once(':')` loop — no XML, no plist. Any code path via `itdb_device_read_sysinfo_xml` would be wrong for this device.
- **`itdb_device_set_sysinfo` is the correct FFI symbol for pushing FirewireGuid.** Confirmed present in bindgen output at line 777. Signature: `fn itdb_device_set_sysinfo(device: *mut Itdb_Device, field: *const gchar, value: *const gchar)`. Called with `"FirewireGuid"` as the field key — matching case exactly as it appears in SysInfo.
- **`iPod_Control` is a hidden directory on Windows** — `Get-ChildItem` needs `-Force` to list it, but `Test-Path` and `Copy-Item` work without it.
- **SysInfo fixture committed at `tests/fixtures/sample-sysinfo.txt`.** Real hardware value: `FirewireGuid: 0x000A27002138B0A8`, `ModelNumStr: MB029`. Not a secret (hardware-bound, like a MAC address).

## Phase 1 gate (2026-05-17) — PASS

- **Result:** PASS — all five acceptance criteria met (boot, both pre-existing tracks present, new track plays, metadata correct, album art on Now Playing).
- **Test track:** Big Wild — Superdream — "City of Sound" (\MUSICHOST\data\media\music\Big Wild\Superdream\01 - City of Sound.flac, 232 sec FLAC, 28 MB, embedded 1000×1000 JPG art, rich MusicBrainz tags).
- **iPod state before Phase 1:** 1 track (Beck "Colors" from Phase 0).
- **iPod state after Phase 1:** 3 tracks (Beck "Colors", Big Wild "City of Sound" without art from first attempt, Big Wild "City of Sound" with art from Plan B retest). The duplicate is a known artifact — libgpod doesn't dedup, Phase 2 manifest will.
- **iTunesDB write (signed):** PASS — itdb_write succeeded twice; DB length grew 21046 → 22718 → 24130 bytes; LastWriteTime updated each run.
- **FirewireGuid wiring:** required and worked — read from `G:\iPod_Control\Device\SysInfo` (flat-text format, not SysInfoExtended XML) and pushed via `itdb_device_set_sysinfo`. Hashed-DB-signing risk SPEC §8 row 2 → **retired** for both read and write paths.
- **Album art Plan A (ffmpeg in-band MP4 atom):** **rejected by iPod Classic UI** — the in-band cover atom is present in the .m4a file but Classic firmware doesn't read it; ArtworkDB + ithmb blobs are the only path. SPEC §8 row 3 risk materialized as expected.
- **Album art Plan B (libgpod itdb_track_set_thumbnails_from_data):** initially failed because the Phase 0 libgpod build lacked gdk-pixbuf (functions exported but no-op). Rebuilt libgpod with gdk-pixbuf + image-format deps (libpng/libjpeg-turbo/libtiff) + vendored pixbuf loader plugins with a GDK_PIXBUF_MODULE_FILE env var wired through build.rs. Verified: 4 new .ithmb blob files (F1055/F1060/F1061/F1068 — multiple iPod display sizes) plus ArtworkDB grew by ~1KB per write. Art shows correctly on Now Playing.
- **iPod post-eject boot:** boots normally, plays all three tracks, art displays on the Plan B Big Wild track.

### Issues to address in Phase 2

- **No deduplication.** libgpod allows the same source to be added repeatedly; right now the iPod has two Big Wild "City of Sound" tracks. SPEC §4.3's manifest-diff logic will handle this — modified tracks are delete-and-add, not duplicate.
- **TRACKTOTAL/DISCTOTAL aliases not handled.** ffprobe extracts `track: "1"` (lone number, not "1/12") + separate `TRACKTOTAL: "12"`. Current `split_pair` loses the total. Add aliases for TRACKTOTAL/TOTALTRACKS/DISCTOTAL/TOTALDISCS in `ProbeTags` and fold them into `Tags.tracks` / `Tags.discs` in `tags_from_probe`.
- **`loaders.cache` contains dev-tree absolute paths.** Works on this machine; breaks for distribution and on a fresh checkout. Fix in build.rs: regenerate the cache at build time by invoking `gdk-pixbuf-query-loaders.exe` against the staged `target/<profile>/pixbuf-loaders/` directory.
- **Two benign GLib warnings during write** that are noisy but not failures:
  - `WARNING: Error parsing recent playcounts` — iPod's `PlayCounts.plist` isn't always present on freshly-restored devices.
  - `CRITICAL: itdb_splr_validate: assertion 'at != ITDB_SPLAT_UNKNOWN' failed` — libgpod's smart-playlist validator walking pre-existing empty/unrecognized rules.
  Install a `g_log_set_handler` in Phase 2 to suppress (or reformat) these so they don't clutter user output.
- **Cleanup orphan tracks if write fails mid-way.** Currently if `itdb_cp_track_to_ipod` succeeds but `itdb_write` fails, the .m4a is orphaned on the iPod. `--rebuild-manifest` recovers from this; document the failure mode in the user-facing error message.

## Phase 2 Gate A (2026-05-18)

- **Result:** PASS.
- **Source:** `<source-library-path>\`
- **FLACs found:** 1407
- **Walk elapsed (release build, end-to-end `cargo run --release -- --dry-run`):** 80.3s
- **Action plan:** Add=1407, Modify=0, Remove=0, Unchanged=0 (expected — no manifest yet).
- **Notes:** Count lines up with SPEC §11's "≈1,400" target. SMB walk + first-MiB BLAKE3 read across 1,407 files completed in 80s — comfortably inside the 30-180s window. No hangs, no errors, no warnings. `cargo build --release` from a clean release tree took 27.5s (dep graph compile); subsequent re-link inside the `cargo run` invocation was 0.16s. Bare-walk elapsed (excluding cargo's already-built check) is dominated by SMB I/O, not Rust work.

## Phase 2 §6 #2 stat-only diff fast path (2026-05-18)

- **Result:** PASS — 1,407-file no-op second run drops from 93.8s to ~0.55s (~170× speedup, ~9× under the 5s SPEC §6 #2 budget).
- **Design:** `SourceEntry` is now stat-only (path/mtime/size, no fingerprint). `manifest::diff` takes `impl FnMut(&Path) -> Result<String>` and only invokes it on the slow path — when stored (mtime, size) doesn't match. New `diff_unchanged_after_touch_but_same_content` test plus `never_called()` callback helper assert the fast path doesn't read file content.
- **Bench-diff example (`examples/bench-diff.rs`):** lets us measure walk + diff time against the real manifest without the iPod plugged in. Source = `\\HOST\data\media\music`. Reproducible target for any future I/O regression.
- **Live numbers (release, SMB):** load manifest 0.001s; walk 1407 files 0.548s; diff 0.002s with 0 fingerprint reads. Pure SMB stat alone is the floor; we're already on it.
- **Fingerprint computation moved to `add_one`:** `add_one(&db, &src) -> Result<(TrackHandle, String)>` — the orchestrator computes the fingerprint once per Add/Modify and threads it into `entry_from(&src, &handle, &fp)`. Walker never reads file content anymore.
- **mtime-touched-but-content-identical case** correctly classified as Unchanged for Phase 2 (slow path runs once, callback returns matching fp). Acceptable mild inefficiency: next run still re-fingerprints because the manifest's stored mtime is stale. Refreshing stored mtime to suppress that is Phase 3+.

## Phase 2 Task 1 — scaffold + carry-forwards (2026-05-18)

- **`itdb_get_mountpoint` IS in bindgen output** (line 722 of `libgpod_bindings.rs`): `pub fn itdb_get_mountpoint(itdb: *mut Itdb_iTunesDB) -> *const gchar`. So the Play Counts.bak fix used the FFI-based approach (read mount from the DB pointer at write time) rather than the stored-mount-path fallback. No `OwnedDb` field addition was needed.
- **`build.rs` loaders.cache regen at build time confirmed working.** `target/debug/pixbuf-loaders/loaders.cache` now references `F:/repos/ipod-sync/target/debug/pixbuf-loaders/libpixbufloader-*.dll` (staged paths) instead of vendor absolute paths. Generated via `C:\msys64\mingw64\bin\gdk-pixbuf-query-loaders.exe` passed the staged DLL list as args; tool writes a header `Created by gdk-pixbuf-query-loaders from gdk-pixbuf-2.44.6`. Fallback to vendor cache copy still in place for envs without MSYS2.

## wipe-tracks dev utility (2026-05-17)

- **`itdb_playlist_remove_track(NULL, track)` with a null playlist removes the track from every playlist** — confirmed working for the wipe case. Do not call `itdb_track_unlink` separately; `itdb_track_remove` covers the DB tracks list removal and struct free in one call.
- **`itdb_filename_on_ipod` returns a `g_strdup`'d path — must `g_free` it.** Returns `NULL` if the track has no on-disk path (can happen for tracks added without `itdb_cp_track_to_ipod`). Always null-check before use.
- **`itdb_write` on Windows fails with "Error renaming 'Play Counts' to 'Play Counts.bak' (File exists)"** when both files are present. Windows rename does not atomically replace an existing file (unlike POSIX `rename(2)`). Fix: delete `Play Counts.bak` before calling `itdb_write`. The DB track data is written BEFORE the play counts rotation, so even if the rename error is raised, the iTunesDB on disk will reflect the in-memory state. Verified: after first run (which errored on play counts rotate), second run saw 0 tracks in the DB.
- **lib.rs + bin target coexist cleanly.** Adding `src/lib.rs` with `pub mod ffi; pub mod ipod; pub mod transcode;` alongside the existing `[[bin]]` target required no Cargo.toml change (Cargo auto-detects `src/lib.rs`). Replace `mod ffi;` etc. in `main.rs` with `use ipod_sync::ffi;` etc. Tests in main.rs continue to work via `use super::*`. The library crate name matches the package name with hyphens → underscores.


## Phase 2 Gate C — full library acceptance (2026-05-17) — PASS

- **Result:** PASS. All exercised SPEC §6 acceptance criteria met.
- **Source library:** `<source-host>\data\media\music` (1,407 FLACs).
- **iPod:** Classic 7G at G:, empty going in.
- **Full sync wall-clock:** ~90 minutes (TUI-driven, transcode-bound).
- **iPod state after sync:** 1,407 m4a files in `iPod_Control\Music\F*`, iTunesDB grew from 18 KB → 2,094,506 bytes, 5 files in `iPod_Control\Artwork\` (ArtworkDB + 4 .ithmb thumbnail blobs).
- **Manifest:** 1,407 entries, valid JSON.

### SPEC §6 acceptance scorecard

- **#1** (empty iPod → full sync, playable, metadata + art): **PASS** — physical verification: iPod boots normally, Music → Songs lists ~1,407 tracks, sampled tracks play with correct metadata + art on Now Playing.
- **#2** (no changes → < 5s): **PASS** after Phase 2.1 mtime+size fast-path optimization. Actual second-run: 945 ms (PowerShell-measured command time). The original implementation was 93.8s (re-fingerprinting all files unconditionally); the fix drops `SourceEntry.fingerprint` from the walker entirely and only computes it inside the diff when mtime+size disagree with the manifest. For an all-unchanged library, zero file reads beyond stat() — ~100× speedup.
- **#3** (add 5 → only 5 processed): **NOT EXERCISED** in Gate C — same code path as the 1,400 Adds in the main run.
- **#4** (delete 5 → only 5 removed): **NOT EXERCISED** in Gate C — same code path as the manifest's Remove handling.
- **#5** (--rebuild-manifest works): **NOT EXERCISED** in Gate C — deferred to future verification.
- **#6** (--dry-run writes nothing): **PASS** — manifest LastWriteTime unchanged after dry-run invocation.

### Phase 1 carry-forwards verified at scale

- **Pixbuf-backed artwork** (Plan B from Phase 1 Task 6b): worked for all 1,407 tracks. ArtworkDB + thumbnail blobs created correctly.
- **Play Counts.bak rename fix**: never re-surfaced during the run.
- **TRACKTOTAL/DISCTOTAL alias handling**: all Picard- and Plex-tagged albums processed without serde duplicate-key errors.
- **GLib log handler**: kept stderr quiet; benign WARNING/CRITICAL noise routed through tracing.

### Observations from the full-scale run

- **Plex-written album art has bad metadata on some files.** Surfaced during physical verification — some tracks showed wrong art on the iPod's Now Playing. Root cause is Plex's media-scanner writing inconsistent cover-art bytes into FLAC tags on the server. Source-data fix, not a tool bug. The user is going to clean up Plex's tagging on the server side.
- **Walker time** is the dominant cost when nothing has changed: ~0.55s for stat()-ing 1,407 SMB files. With the fingerprint short-circuit, that's the whole runtime. Acceptable.

### Phase 3 carry-forwards

- **mtime-touched-but-content-identical files** correctly classify as Unchanged but re-fingerprint every subsequent run because the stored mtime stays stale. Phase 3+ refinement: refresh stored mtime on the slow-path-Unchanged case so the next run hits the fast path again. Tiny code change, real-world impact on libraries with `touch`-style operations.
- **Plex-bad-art investigation**: worth a small forensic pass to confirm which tracks have which issue, so the user can fix at the source.

## Phase 3.y gate (2026-05-17) — PASS

- **Result:** PASS — UX layer ships.
- **Wizard:** launches when no source set; saves to `%APPDATA%\ipod-sync\config.toml`; orchestrator continues after Enter.
- **Review state:** action plan renders correctly; `t` toggles `--no-delete` and flips the Remove count display; `d` exits cleanly with "Dry run; nothing was written"; `q` quits without changes; `a` proceeds to apply.
- **--apply flag:** skips review, applies immediately. Validated in no-change run (~1s).
- **--dry-run flag:** skips review, exits after summary.
- **--save-config:** persists effective config (tested implicitly via wizard write — explicit `--save-config` flag still standing for future ad-hoc persist cases).
- **Non-TTY rejection:** confirmed errors clearly when `--no-tui` is set without explicit `--apply` or `--dry-run`.

### Phase 3.z carry-forward

User flagged: "we might want to make the UX a bit more interactive so that all interactions are done in the TUI (even errors, etc.)" — captured as discrete roadmap item "Phase 3.z — TUI-first error UX" in `docs/superpowers/specs/2026-05-18-post-v1-roadmap.md`.
