# Learnings — ipod-sync

Per global CLAUDE.md: record discovered conventions, gotchas, debugging insights, and useful commands here as work proceeds. One bullet per learning.

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
