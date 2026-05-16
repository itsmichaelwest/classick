# Phase 0: libgpod Windows Spike — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove that a Rust program on Windows MSVC can link against libgpod, open a real iPod Classic 7th gen's `iTunesDB`, list its tracks, and exit cleanly — without corrupting the database.

**Architecture:** Single binary crate with a `build.rs` that links the libgpod DLL/lib placed in `vendor/libgpod/` and generates FFI bindings via `bindgen`. The spike is read-only: it parses the DB, dumps the track list, frees, exits. No writes. No ffmpeg. No CLI. No manifest. The success criterion is a printed list of tracks from the connected iPod plus the iPod still booting normally after the spike runs.

**Tech Stack:** Rust stable (`x86_64-pc-windows-msvc`), `bindgen`, `anyhow`, libgpod (Windows build — acquisition path determined by Task 2), Visual Studio Build Tools 2022, vcpkg (only if building libgpod from source).

**Plan scope:** This plan covers Phase 0 only. Phase 1 (end-to-end single track) and Phase 2 (full tool) are deferred to separate plans, written *after* this one lands. Rationale: the exact bindgen-generated Rust symbol names, the GLib lifecycle behavior on Windows, and any device-specific quirks of the real 160 GB Classic (`EXAMPLE1234`) are unknown until the spike runs. Locking those in a plan now would mean writing speculative code.

**Gate:** If this plan cannot be completed in roughly one focused day of work, escalate per SPEC §8 Risk row 1 (alternatives: native iTunesDB Rust port, or TunesReloaded WASM via wasmtime) before proceeding to Phase 1.

---

## File Structure

```
F:\repos\ipod-sync\
├── .gitignore                              (created Task 1)
├── Cargo.toml                              (created Task 1)
├── README.md                               (created Task 1, stub only)
├── SPEC.md                                 (exists)
├── docs\superpowers\plans\
│   └── 2026-05-17-phase-0-libgpod-spike.md (this file)
├── build.rs                                (created Task 5)
├── src\
│   └── main.rs                             (created Task 6, replaced Task 7)
└── vendor\
    └── libgpod\
        ├── include\gpod\itdb.h             (placed Task 3 or 4)
        ├── lib\gpod.lib                    (placed Task 3 or 4)
        └── bin\gpod.dll                    (placed Task 3 or 4)
```

`vendor/libgpod/` is the integration boundary with the C library. `build.rs` is the only Rust code that touches the vendor layout. `src/main.rs` is the spike itself — under ~100 lines, single file is appropriate at this stage.

---

## Task 1: Bootstrap the Rust project

**Files:**
- Create: `F:\repos\ipod-sync\Cargo.toml`
- Create: `F:\repos\ipod-sync\.gitignore`
- Create: `F:\repos\ipod-sync\README.md`
- Create: `F:\repos\ipod-sync\src\main.rs` (placeholder)
- Create: `F:\repos\ipod-sync\LEARNINGS.md`

- [ ] **Step 1: Initialize git**

Run from `F:\repos\ipod-sync\`:
```powershell
git init
git config user.email 19785650+itsmichaelwest@users.noreply.github.com
git config user.name "Michael West"
```
Expected: `Initialized empty Git repository in F:/repos/ipod-sync/.git/`

- [ ] **Step 2: Write `Cargo.toml`**

```toml
[package]
name = "ipod-sync"
version = "0.0.1"
edition = "2021"
description = "Windows-native FLAC-to-iPod-Classic sync with on-the-fly ALAC transcoding"
license = "MIT OR Apache-2.0"

[[bin]]
name = "ipod-sync"
path = "src/main.rs"

[dependencies]
anyhow = "1"

[build-dependencies]
bindgen = "0.69"
```

- [ ] **Step 3: Write `.gitignore`**

```gitignore
# Rust
/target
Cargo.lock

# libgpod binaries are platform-specific and large
/vendor/libgpod/bin/*.dll
/vendor/libgpod/bin/*.pdb

# Build artifacts
*.exe
*.pdb

# Editor
.vscode/
.idea/

# OS
Thumbs.db
.DS_Store

# Project state (manifest lives in %APPDATA%, but be safe)
/manifest.json
```

Note: header (`vendor/libgpod/include/`) and import lib (`vendor/libgpod/lib/gpod.lib`) ARE checked in — they're small and reproducibility-critical. Only the DLL/PDB are excluded.

- [ ] **Step 4: Write `README.md` stub**

```markdown
# ipod-sync

Windows-native CLI to sync a FLAC library to an iPod Classic, transcoding to ALAC on the fly.

Status: Phase 0 (libgpod spike). See `SPEC.md` for the full design and `docs/superpowers/plans/` for the implementation plan.
```

- [ ] **Step 5: Write `src/main.rs` placeholder**

```rust
fn main() {
    println!("ipod-sync — Phase 0 spike (not yet implemented)");
}
```

- [ ] **Step 6: Write `LEARNINGS.md`**

```markdown
# Learnings — ipod-sync

Per global CLAUDE.md: record discovered conventions, gotchas, debugging insights, and useful commands here as work proceeds. One bullet per learning.

## Phase 0

- (entries added during implementation)
```

- [ ] **Step 7: Verify the crate builds**

Run from `F:\repos\ipod-sync\`:
```powershell
cargo build
```
Expected: builds successfully, produces `target\debug\ipod-sync.exe`. If `cargo` is missing, install Rust via https://rustup.rs/ with the MSVC toolchain (`x86_64-pc-windows-msvc`) and re-run.

- [ ] **Step 8: Run the placeholder**

Run:
```powershell
.\target\debug\ipod-sync.exe
```
Expected output: `ipod-sync — Phase 0 spike (not yet implemented)`

- [ ] **Step 9: Commit**

```powershell
git add Cargo.toml .gitignore README.md src\main.rs LEARNINGS.md SPEC.md docs\
git commit -m "chore: bootstrap ipod-sync Rust crate"
```

---

## Task 2: Research libgpod Windows acquisition path (decision)

This task is **research**, not code. Its output is a written decision recorded in `LEARNINGS.md` that selects either Task 3 (prebuilt) or Task 4 (build from source). Spend at most 60 minutes on this task before falling back to Task 4.

**Files:**
- Modify: `F:\repos\ipod-sync\LEARNINGS.md` (record findings)

- [ ] **Step 1: Check MSYS2 for `mingw-w64-x86_64-libgpod`**

Search the MSYS2 package database online: https://packages.msys2.org/?q=libgpod
- If a package exists for `mingw-w64-x86_64-libgpod` (or `ucrt-x86_64`), note the version and download URL.
- MSYS2 packages link against UCRT or MinGW runtimes — usable from MSVC Rust *only if* the DLL has a clean C ABI and no MinGW-specific runtime deps. Verify with `dumpbin /dependents` on the DLL after extracting it.

- [ ] **Step 2: Check gtkpod's historical Windows installers**

Browse https://sourceforge.net/projects/gtkpod/files/ — look for any Windows installer in the last 10 years that bundles `libgpod-*.dll`. Note version and URL if found.

- [ ] **Step 3: Check Hydrogenaudio + gtkpod-devel mailing list archives**

Search "libgpod windows msvc build" — note any contributor with a known-working Windows build.

- [ ] **Step 4: Try a quick link test (if a candidate DLL was found)**

In a scratch directory, place the candidate DLL + headers and run:
```powershell
dumpbin /exports <candidate>\bin\libgpod.dll | Select-String "itdb_parse"
dumpbin /dependents <candidate>\bin\libgpod.dll
```
Expected: `itdb_parse` (or `itdb_parse_file`) appears in exports; dependents list does not include unusual MinGW-only runtimes (e.g. `libgcc_s_seh-1.dll`) — or if it does, those DLLs are bundled alongside.

- [ ] **Step 5: Record the decision in `LEARNINGS.md`**

Append a section like:
```markdown
## libgpod acquisition (2026-05-17)

- Searched: MSYS2 (result: <found/not found, version>), gtkpod SourceForge (result: <version + url or "stale, last release YYYY">), Hydrogenaudio (result: <link or none).
- Decision: <Branch A — use prebuilt from <source>> OR <Branch B — build from source>.
- Reason: <one sentence>.
```

- [ ] **Step 6: Commit**

```powershell
git add LEARNINGS.md
git commit -m "docs: record libgpod Windows acquisition decision"
```

**Branch:** if Branch A, proceed to Task 3 and skip Task 4. If Branch B, skip Task 3 and proceed to Task 4.

---

## Task 3: Vendor a prebuilt libgpod (Branch A only)

Skip this task if Task 2 selected Branch B.

**Files:**
- Create: `F:\repos\ipod-sync\vendor\libgpod\include\gpod\itdb.h` (and any sibling headers it includes)
- Create: `F:\repos\ipod-sync\vendor\libgpod\lib\gpod.lib`
- Create: `F:\repos\ipod-sync\vendor\libgpod\bin\gpod.dll`
- Create: `F:\repos\ipod-sync\vendor\libgpod\README.md` (provenance)

- [ ] **Step 1: Create the vendor directory layout**

```powershell
New-Item -ItemType Directory -Force -Path F:\repos\ipod-sync\vendor\libgpod\include\gpod, F:\repos\ipod-sync\vendor\libgpod\lib, F:\repos\ipod-sync\vendor\libgpod\bin | Out-Null
```

- [ ] **Step 2: Extract the prebuilt package**

From the candidate identified in Task 2: copy `libgpod.dll` → `vendor\libgpod\bin\gpod.dll`, the import lib → `vendor\libgpod\lib\gpod.lib`, and all `gpod/*.h` headers → `vendor\libgpod\include\gpod\`.

If the prebuilt provides only `libgpod.dll` and no `.lib`, generate one via:
```powershell
dumpbin /exports vendor\libgpod\bin\gpod.dll > exports.txt
# extract the function names into a .def file, then:
lib /def:gpod.def /machine:x64 /out:vendor\libgpod\lib\gpod.lib
```
(Record the steps actually used in `LEARNINGS.md`.)

- [ ] **Step 3: Copy GLib dependency DLLs if required**

`dumpbin /dependents vendor\libgpod\bin\gpod.dll` will list its runtime deps. Likely candidates: `libglib-2.0-0.dll`, `libgobject-2.0-0.dll`, `libintl-8.dll`, `libiconv-2.dll`, `zlib1.dll`. Copy each into `vendor\libgpod\bin\` from the same source package.

- [ ] **Step 4: Write `vendor\libgpod\README.md`**

```markdown
# libgpod (vendored)

Source: <URL / package name / version from Task 2>
Date acquired: 2026-05-17
ABI: x86_64 Windows

Files:
- `include/gpod/itdb.h` (+ siblings) — public headers
- `lib/gpod.lib` — MSVC import library
- `bin/gpod.dll` — runtime library
- `bin/lib*.dll` — GLib runtime dependencies (see `dumpbin /dependents`)

To rebuild from source, see SPEC §5.2.
```

- [ ] **Step 5: Commit headers + import lib (DLL is gitignored)**

```powershell
git add vendor\libgpod\include vendor\libgpod\lib vendor\libgpod\README.md
git commit -m "build: vendor prebuilt libgpod headers and import lib"
```

(DLLs are gitignored per Task 1 step 3. They live on disk locally but won't be committed. Distribution will copy them next to the exe at install time — out of scope for Phase 0.)

---

## Task 4: Build libgpod from source (Branch B only)

Skip this task if Task 2 selected Branch A.

This task may take several hours and has the highest unknown-unknowns of any task in the plan. If it hasn't produced a linkable DLL after one focused day, treat that as Phase 0 failure and escalate per the gate criterion at the top of this document.

**Files:**
- Create: `F:\repos\ipod-sync\vendor\libgpod\include\gpod\itdb.h` (and siblings)
- Create: `F:\repos\ipod-sync\vendor\libgpod\lib\gpod.lib`
- Create: `F:\repos\ipod-sync\vendor\libgpod\bin\gpod.dll` (+ GLib deps)
- Create: `F:\repos\ipod-sync\vendor\libgpod\BUILD-NOTES.md`

- [ ] **Step 1: Install build prerequisites**

If not already present:
- Visual Studio Build Tools 2022 with the "Desktop development with C++" workload (includes MSVC, Windows SDK, CMake).
- Python 3 (for meson).
- vcpkg: `git clone https://github.com/microsoft/vcpkg.git C:\vcpkg && C:\vcpkg\bootstrap-vcpkg.bat`.

Verify:
```powershell
cl.exe /?    # MSVC compiler
python --version
C:\vcpkg\vcpkg.exe version
```

- [ ] **Step 2: Install GLib via vcpkg**

```powershell
C:\vcpkg\vcpkg.exe install glib:x64-windows
```
Expected: completes successfully. The build artifacts live under `C:\vcpkg\installed\x64-windows\`.

- [ ] **Step 3: Install meson and ninja**

```powershell
pip install meson ninja
meson --version
ninja --version
```

- [ ] **Step 4: Clone libgpod**

```powershell
git clone https://github.com/fadingred/libgpod.git C:\src\libgpod
```
(If that mirror is gone, fall back to the SourceForge tarball under https://sourceforge.net/projects/gtkpod/files/libgpod/ — extract to `C:\src\libgpod`.)

- [ ] **Step 5: Configure with meson against vcpkg GLib**

Open the **x64 Native Tools Command Prompt for VS 2022**, then:
```cmd
set PKG_CONFIG_PATH=C:\vcpkg\installed\x64-windows\lib\pkgconfig
cd C:\src\libgpod
meson setup build --buildtype=release --default-library=shared -Dwith-gtk=false -Dwith-python=false
```

Expected: configuration succeeds. If it fails because of missing dependencies (libxml2, sqlite3, etc.), `vcpkg install <pkg>:x64-windows` and re-run. Record each dep added in `vendor\libgpod\BUILD-NOTES.md`.

If configuration fails for code reasons (MSVC vs GCC syntax), search the gtkpod-devel archives for a Windows patch; apply and document.

- [ ] **Step 6: Build**

```cmd
meson compile -C build
```
Expected: produces `build\src\libgpod-1.0-8.dll` (or similar) plus an import lib.

- [ ] **Step 7: Copy artifacts to `vendor/libgpod/`**

```powershell
New-Item -ItemType Directory -Force -Path F:\repos\ipod-sync\vendor\libgpod\include\gpod, F:\repos\ipod-sync\vendor\libgpod\lib, F:\repos\ipod-sync\vendor\libgpod\bin | Out-Null
Copy-Item C:\src\libgpod\src\itdb.h F:\repos\ipod-sync\vendor\libgpod\include\gpod\
# Copy any other public headers referenced by itdb.h (e.g. itdb_device.h)
Copy-Item C:\src\libgpod\build\src\*.dll F:\repos\ipod-sync\vendor\libgpod\bin\
Copy-Item C:\src\libgpod\build\src\*.lib F:\repos\ipod-sync\vendor\libgpod\lib\
# Copy GLib runtime DLLs from vcpkg
Copy-Item C:\vcpkg\installed\x64-windows\bin\glib-*.dll F:\repos\ipod-sync\vendor\libgpod\bin\
Copy-Item C:\vcpkg\installed\x64-windows\bin\gobject-*.dll F:\repos\ipod-sync\vendor\libgpod\bin\
Copy-Item C:\vcpkg\installed\x64-windows\bin\intl-*.dll F:\repos\ipod-sync\vendor\libgpod\bin\
Copy-Item C:\vcpkg\installed\x64-windows\bin\iconv-*.dll F:\repos\ipod-sync\vendor\libgpod\bin\
Copy-Item C:\vcpkg\installed\x64-windows\bin\zlib1.dll F:\repos\ipod-sync\vendor\libgpod\bin\
```

Rename the libgpod artifacts to `gpod.dll` / `gpod.lib` for predictable `build.rs` linkage:
```powershell
Get-ChildItem F:\repos\ipod-sync\vendor\libgpod\bin\libgpod*.dll | Rename-Item -NewName "gpod.dll"
Get-ChildItem F:\repos\ipod-sync\vendor\libgpod\lib\libgpod*.lib | Rename-Item -NewName "gpod.lib"
```

- [ ] **Step 8: Verify exports**

```powershell
dumpbin /exports F:\repos\ipod-sync\vendor\libgpod\bin\gpod.dll | Select-String "itdb_parse"
```
Expected: shows `itdb_parse` and/or `itdb_parse_file`.

- [ ] **Step 9: Write `vendor\libgpod\BUILD-NOTES.md`**

```markdown
# libgpod Windows build notes (2026-05-17)

Source: <git repo + commit SHA>
Build host: Windows 11 + VS 2022 Build Tools + vcpkg + meson <version>

## vcpkg packages installed
- glib:x64-windows
- (others as needed)

## Patches applied
- (list any Windows compat patches and their source)

## Reproduction steps
(paste the exact meson commands used)
```

- [ ] **Step 10: Commit**

```powershell
git add vendor\libgpod\include vendor\libgpod\lib vendor\libgpod\BUILD-NOTES.md
git commit -m "build: vendor libgpod headers and import lib (built from source)"
```

---

## Task 5: Wire `build.rs` for linking and bindgen

**Files:**
- Create: `F:\repos\ipod-sync\build.rs`
- Modify: `F:\repos\ipod-sync\Cargo.toml` (add build script declaration if needed — it's auto-detected when `build.rs` exists at the crate root, no Cargo.toml change required)
- Create: `F:\repos\ipod-sync\src\ffi.rs` (generated bindings re-exported; the actual generation goes to `OUT_DIR`)

- [ ] **Step 1: Verify `bindgen` prerequisites**

`bindgen` requires `libclang.dll` on PATH. Install LLVM for Windows from https://github.com/llvm/llvm-project/releases (pick the latest `LLVM-*-win64.exe`) and ensure `C:\Program Files\LLVM\bin` is on `PATH`. Then:
```powershell
clang --version
```
Expected: prints a version.

- [ ] **Step 2: Write `build.rs`**

```rust
use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor = manifest_dir.join("vendor").join("libgpod");

    // Link the import library
    println!(
        "cargo:rustc-link-search=native={}",
        vendor.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=gpod");

    // Re-run if the header changes
    let header = vendor.join("include").join("gpod").join("itdb.h");
    println!("cargo:rerun-if-changed={}", header.display());

    // Generate Rust bindings
    let bindings = bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .clang_arg(format!("-I{}", vendor.join("include").display()))
        // GLib types come in through itdb.h — let bindgen ingest them
        .allowlist_function("itdb_.*")
        .allowlist_type("Itdb_.*")
        .allowlist_var("ITDB_.*")
        .layout_tests(false)
        .generate()
        .expect("bindgen failed to generate libgpod bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("libgpod_bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("failed to write bindings");

    // Ensure the DLL is alongside the exe for `cargo run`
    let dll_src = vendor.join("bin").join("gpod.dll");
    let target_dir = manifest_dir
        .join("target")
        .join(env::var("PROFILE").unwrap());
    if dll_src.exists() {
        let _ = std::fs::create_dir_all(&target_dir);
        let _ = std::fs::copy(&dll_src, target_dir.join("gpod.dll"));
        // Copy GLib runtime deps too
        if let Ok(entries) = std::fs::read_dir(vendor.join("bin")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("dll") {
                    let _ = std::fs::copy(&path, target_dir.join(path.file_name().unwrap()));
                }
            }
        }
    }
}
```

The DLL copy at the bottom is a convenience so `cargo run` works without manually staging DLLs. If `bindgen` fails because GLib's transitively-included headers aren't found, add their include directory to `.clang_arg("-I...")` — vcpkg's GLib installs headers under `C:\vcpkg\installed\x64-windows\include\glib-2.0\`.

- [ ] **Step 3: Write `src/ffi.rs`**

```rust
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]

include!(concat!(env!("OUT_DIR"), "/libgpod_bindings.rs"));
```

- [ ] **Step 4: Reference the FFI module from `main.rs`**

Replace `src/main.rs`:
```rust
mod ffi;

fn main() {
    // Sanity check that bindings compiled.
    // ITDB_FILETYPE_MASK is a representative #define from itdb.h; if bindgen
    // generated a different constant name, substitute any allowlisted item.
    println!("ipod-sync build.rs + bindgen wired up");
    println!("size of Itdb_Track: {}", std::mem::size_of::<ffi::Itdb_Track>());
}
```

If `ffi::Itdb_Track` doesn't resolve, list what bindgen produced:
```powershell
Get-Content "$((cargo metadata --format-version=1 | ConvertFrom-Json).target_directory)\debug\build\ipod-sync-*\out\libgpod_bindings.rs" | Select-Object -First 200
```
and substitute a type that *does* appear (typically lowercase `_Itdb_Track` or similar — bindgen mirrors the C struct tag).

- [ ] **Step 5: Build**

```powershell
cargo build
```
Expected: builds successfully. On first build, expect output like `Compiling bindgen`, `Compiling ipod-sync`. If link fails with `LNK2019 unresolved external symbol`, the import lib's name doesn't match — verify `gpod.lib` is in `vendor\libgpod\lib\` and exports the symbol referenced (`dumpbin /headers vendor\libgpod\lib\gpod.lib | Select-String "Name "`).

- [ ] **Step 6: Run**

```powershell
.\target\debug\ipod-sync.exe
```
Expected output:
```
ipod-sync build.rs + bindgen wired up
size of Itdb_Track: <some number, typically 200-600>
```

- [ ] **Step 7: Commit**

```powershell
git add build.rs src\ffi.rs src\main.rs Cargo.toml
git commit -m "build: wire bindgen and link libgpod"
```

---

## Task 6: Write the spike — open iPod DB and list tracks

**Files:**
- Modify: `F:\repos\ipod-sync\src\main.rs`

The spike is invoked with a hardcoded path to the iPod's `iTunesDB`. It parses, prints a summary plus the first few tracks, frees, exits. Read-only.

- [ ] **Step 1: Identify the iPod's drive letter**

Plug the iPod in. In PowerShell:
```powershell
Get-Volume | Where-Object FileSystemLabel -match "iPod|IPOD"
```
Or browse `This PC` for the drive that contains `iPod_Control\iTunes\iTunesDB`. Record the drive letter (likely `D:`, `E:`, or `G:`). For the remainder of this task assume `G:`.

- [ ] **Step 2: Verify the DB file exists and is readable**

```powershell
Test-Path G:\iPod_Control\iTunes\iTunesDB
Get-Item G:\iPod_Control\iTunes\iTunesDB | Select-Object Length, LastWriteTime
```
Expected: `True`, with a non-zero `Length`. If false, the iPod isn't mounted as MSC (Mass Storage Class) — restore via iTunes per SPEC §11 handoff notes before continuing.

- [ ] **Step 3: Write the spike**

Replace `src/main.rs`:
```rust
mod ffi;

use anyhow::{anyhow, Result};
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;

/// Hardcoded for the Phase 0 spike. Phase 1 will accept this via CLI.
const IPOD_MOUNT: &str = "G:\\";

fn main() -> Result<()> {
    let mount_path = Path::new(IPOD_MOUNT);
    let db_path = mount_path.join("iPod_Control").join("iTunes").join("iTunesDB");
    if !db_path.exists() {
        return Err(anyhow!(
            "iTunesDB not found at {} — is the iPod mounted at {}?",
            db_path.display(),
            IPOD_MOUNT
        ));
    }

    println!("Opening iTunesDB at: {}", db_path.display());

    // SAFETY: itdb_parse_file allocates an Itdb_iTunesDB on success or returns
    // NULL and sets *error on failure. We must call itdb_free on success.
    let db = unsafe {
        let path_c = CString::new(db_path.to_str().unwrap())?;
        let mut err: *mut ffi::GError = ptr::null_mut();
        let db = ffi::itdb_parse_file(path_c.as_ptr(), &mut err);
        if db.is_null() {
            let msg = if err.is_null() {
                "itdb_parse_file returned NULL with no error".to_string()
            } else {
                let m = CStr::from_ptr((*err).message).to_string_lossy().into_owned();
                ffi::g_error_free(err);
                m
            };
            return Err(anyhow!("itdb_parse_file failed: {}", msg));
        }
        db
    };

    // Walk the track list (GList *)
    let mut count: usize = 0;
    let mut node = unsafe { (*db).tracks };
    let mut printed = 0usize;
    while !node.is_null() {
        let track = unsafe { (*node).data as *mut ffi::Itdb_Track };
        if printed < 5 && !track.is_null() {
            let title = unsafe { cstr_or_empty((*track).title) };
            let artist = unsafe { cstr_or_empty((*track).artist) };
            let album = unsafe { cstr_or_empty((*track).album) };
            println!("  [{}] {} — {} — {}", printed + 1, artist, album, title);
            printed += 1;
        }
        count += 1;
        node = unsafe { (*node).next };
    }

    println!("Total tracks: {}", count);

    // Free
    unsafe { ffi::itdb_free(db) };

    Ok(())
}

/// Convert a possibly-null C string from libgpod into a Rust String,
/// returning "<none>" if NULL.
unsafe fn cstr_or_empty(p: *mut std::os::raw::c_char) -> String {
    if p.is_null() {
        return "<none>".to_string();
    }
    CStr::from_ptr(p).to_string_lossy().into_owned()
}
```

Notes for the implementer:
- The exact bindgen-generated names may vary. If `ffi::itdb_parse_file` doesn't exist, search the generated bindings (`Get-ChildItem $env:OUT_DIR -Recurse libgpod_bindings.rs` to locate, then grep for `itdb_parse`) — there may be `itdb_parse` (which takes a mount path, not a file path) instead. If only `itdb_parse` exists, pass `IPOD_MOUNT` (the drive root) rather than `db_path`. Document the choice in `LEARNINGS.md`.
- `GError` and `g_error_free` come from GLib via libgpod's headers. If they're allowlisted out, add `.allowlist_function("g_error_.*").allowlist_type("GError")` to `build.rs` and rebuild.
- `Itdb_Track`'s field names (`title`, `artist`, `album`) match the libgpod 0.8+ C struct. If bindgen mangled them, use the names from the generated file verbatim.

- [ ] **Step 4: If empty iPod — populate first**

The test target iPod (`EXAMPLE1234`) was described as freshly restored / empty. Listing zero tracks is a degenerate test. Before running the spike, use iTunes 12.6.5.3 to sync 2-3 known tracks onto the iPod manually so the spike has something to print. Eject cleanly afterward and re-mount.

If iTunes is not available, this step is optional — the spike will still validate parse + free with zero tracks, but won't validate field access.

- [ ] **Step 5: Run the spike**

```powershell
cargo run
```
Expected output (with at least one track on the iPod):
```
Opening iTunesDB at: G:\iPod_Control\iTunes\iTunesDB
  [1] <artist> — <album> — <title>
  ...
Total tracks: N
```

If the program crashes or returns an error, capture the error and investigate. Common failure modes:
- `itdb_parse_file failed: ...checksum...` → libgpod can't verify the hashed DB signature. Some libgpod versions need an explicit FirewireGUID set via env var `IPOD_FIREWIRE_GUID` or via an additional API call. Read `SysInfoExtended` from the iPod, extract the `FirewireGuid` value, and try again with `$env:IPOD_FIREWIRE_GUID = "<guid>"; cargo run`.
- Access violation / segfault → likely a layout mismatch between `itdb.h` and the linked DLL. Confirm both came from the same libgpod version.

- [ ] **Step 6: Eject the iPod cleanly and verify it still boots**

In Windows, right-click the iPod's drive → Eject. Wait for the "Safe to remove" message. Unplug. The iPod should display its normal menu and remain playable.

This is the read-side acceptance criterion for Phase 0.

- [ ] **Step 7: Record observations in `LEARNINGS.md`**

Append a section noting:
- libgpod version used.
- Which `itdb_parse*` symbol was correct.
- Whether FirewireGUID setup was needed.
- Exact field names used from `Itdb_Track`.
- Any unexpected behavior.

- [ ] **Step 8: Commit**

```powershell
git add src\main.rs LEARNINGS.md
git commit -m "feat: Phase 0 spike opens iTunesDB and lists tracks"
```

---

## Task 7: Phase 0 gate review

**Files:**
- Modify: `F:\repos\ipod-sync\LEARNINGS.md` (final gate record)

- [ ] **Step 1: Run the acceptance checklist**

Confirm each of the following:
- [ ] `cargo build` from a clean checkout (`cargo clean && cargo build`) succeeds.
- [ ] `cargo run` with the iPod mounted prints "Total tracks: N" where N matches what iTunes/Finder showed before the spike ran.
- [ ] The iPod boots normally after ejection and plays one of the tracks listed by the spike.
- [ ] No files were created or modified anywhere under `<iPod>:\iPod_Control\` by the spike. Verify with:
  ```powershell
  Get-ChildItem G:\iPod_Control -Recurse -File | Where-Object LastWriteTime -gt (Get-Date).AddMinutes(-30)
  ```
  Expected: empty.

- [ ] **Step 2: Record the gate result in `LEARNINGS.md`**

Append:
```markdown
## Phase 0 gate (2026-05-17)

- Result: PASS / FAIL (<reason if fail>)
- libgpod build provenance: <Branch A source / Branch B build SHA>
- Tracks listed: <N>
- iPod post-eject state: boots, plays / does not boot (<details>)
- Open issues to address in Phase 1: <list, or "none">
```

- [ ] **Step 3: Tag the milestone**

```powershell
git add LEARNINGS.md
git commit -m "docs: Phase 0 gate result"
git tag -a phase-0-complete -m "libgpod Windows spike complete"
```

- [ ] **Step 4: Hand off to Phase 1 planning**

If the gate passed: write the Phase 1 plan (`docs/superpowers/plans/YYYY-MM-DD-phase-1-single-track.md`) using the same brainstorming → writing-plans flow, informed by the LEARNINGS recorded in Task 6 (which `itdb_*` symbols exist, whether FirewireGUID setup is needed, etc.).

If the gate failed: escalate per the SPEC §8 Risk row 1 mitigation. Do not proceed to Phase 1.

---

## Self-review

**Spec coverage (Phase 0 scope only):** SPEC §12.1 Phase 0 = "obtain libgpod, write minimal Rust that opens DB, lists tracks, exits cleanly, no ffmpeg/manifest/CLI." Tasks 2/3/4 cover acquisition. Task 5 covers FFI wiring. Task 6 covers the spike itself including the list-tracks acceptance behavior. Task 7 covers the gate. SPEC §1 constraint 5 ("must not corrupt the iPod's iTunesDB") is covered by Task 7 step 1 read-only verification. SPEC §5.2/§5.3 (libgpod build vs prebuilt) is covered by the Task 2 branch.

Out of Phase 0 scope (deferred to later plans by design): ffmpeg, manifest, source walker, CLI flags, transcoding, writes to iPod, album art, mount auto-detect, all of SPEC §4 except DB-open and DB-free.

**Placeholder scan:** No "TBD/TODO". One "fill in based on findings" pattern appears in Task 2 step 5 (LEARNINGS template), Task 3 step 4 (vendor README), and Task 4 step 9 (BUILD-NOTES) — these are intentional: the engineer fills in factual values they discover, not implementation logic. That's appropriate. No vague "add error handling" steps. No "similar to Task N" references — each step shows full code.

**Type consistency:** `ffi::itdb_parse_file`, `ffi::itdb_free`, `ffi::Itdb_Track`, `ffi::GError`, `ffi::g_error_free` are referenced. The Task 6 notes explicitly flag that exact names depend on bindgen output and instruct the engineer how to verify and adjust. Field accesses (`tracks`, `next`, `data`, `title`, `artist`, `album`) are consistent across the single code block where they appear.

**Scope check:** Phase 0 is a focused single-day spike. The plan does not creep into Phase 1 work (no transcoding, no manifest, no write paths). Confirmed appropriate scope for one plan.
