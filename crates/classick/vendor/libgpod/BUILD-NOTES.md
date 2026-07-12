# libgpod Windows build notes (2026-05-17)

Built via MSYS2 / MinGW-w64 autotools per SPEC §12.7. Rust links against
an MSVC import library (`lib/gpod.lib`) generated from the MinGW DLL's
export table; at runtime it loads the MinGW-flavored `bin/gpod.dll`
alongside the MinGW C/GLib runtime DLLs in the same directory.

## Source

- Repo: `https://github.com/fadingred/libgpod.git`
- Commit SHA: `4a8a33ef4bc58eee1baca6793618365f75a5c3fa` ("Merge branch '0.8.0+'")
- Upstream version: libgpod 0.8.0

## Build host

- Windows 11 Pro 10.0.26200
- MSYS2 runtime: `msys2-runtime 3.6.9-1`
- MinGW64 toolchain: `mingw-w64-x86_64-gcc 16.1.0-4`

## MSYS2 packages installed (build deps)

Listed in the order they were installed.

- `base-devel` (MSYS) — make, m4, bison, flex, patch, texinfo, etc.
- `mingw-w64-x86_64-toolchain` — gcc, binutils, crt, headers, winpthreads, gdb, pkgconf, make
- `mingw-w64-x86_64-glib2 2.88.1-1` — load-bearing runtime + dev headers
- `mingw-w64-x86_64-libxml2 2.15.3-1`
- `mingw-w64-x86_64-libplist 2.7.0-4` — installed but NOT linked (see Patches)
- `mingw-w64-x86_64-sqlite3 3.53.1-1` — installed but NOT linked (see Patches)
- `mingw-w64-x86_64-zlib 1.3.2-2`
- `mingw-w64-x86_64-libgcrypt 1.12.2-2` — load-bearing for late-model Classic iTunesDB signature
- `mingw-w64-x86_64-pkgconf 1~2.5.1-1`
- `mingw-w64-x86_64-gettext-tools 1.0-1`
- `intltool 0.51.0-4` (MSYS)
- `git`, `autoconf-wrapper`, `automake-wrapper`, `libtool` (MSYS) — required by `./autogen.sh` and to clone the repo; not listed in the plan's Step 3 but needed in practice
- `mingw-w64-x86_64-gtk-doc 1.34.0-3` — `autogen.sh` runs `gtkdocize` regardless of `--disable-gtk-doc` later in `./configure`

## Environment quirks

`./autogen.sh` runs MSYS's `aclocal` which doesn't search the mingw64
prefix by default, so the required m4 macros (`glib-gettext.m4`,
`pkg.m4`, `gtk-doc.m4`) aren't found. Workaround: export
`ACLOCAL_FLAGS='-I /mingw64/share/aclocal'` before each `autogen.sh`
invocation. Without this, autogen fails with "some autoconf macros
required to build libgpod were not found in your aclocal path".

## configure options

```
./configure \
    --prefix=/mingw64 \
    --disable-static \
    --without-hal \
    --disable-gtk-doc \
    --disable-introspection \
    --without-python \
    --disable-more-warnings
```

`--disable-more-warnings` is added beyond the plan's listed flags. It
suppresses libgpod's developer-mode `-Werror` block in `configure.ac`
(`AC_ARG_ENABLE(more-warnings, ...)`, defaults to "yes" when
`autogen.sh` is present). Without it, the build fails because GCC 16.1
and GLib 2.88 emit deprecation warnings (`g_get_current_time`,
`GTimeVal`, `g_value_array_*`) and unused-variable warnings on libgpod
0.8.0 code that was clean against older toolchains. The warnings are
benign for our use case — libgpod's behavior on the affected code paths
is unchanged.

## Patches applied

Stored in `patches/` next to this file. Apply with `git apply` from
the libgpod source root.

### `0001-drop-libplist-sqlite3-add-gmodule.patch`

Strips the `libplist` and `sqlite3` dependencies from libgpod entirely
and rewires `gmodule-2.0` (which the source uses but the original
PKG_CHECK_MODULES omitted on the fadingred branch). Rationale:

- libplist 1.x is unavailable in modern MSYS2; only libplist 2.x ships,
  and the 2.x API renamed/removed the `plist_dict_insert_item`,
  `plist_dict_new_iter`, etc. that libgpod 0.8.0 uses. Porting those
  call sites is out of scope.
- The only consumer of libplist + sqlite3 in libgpod is
  `src/itdb_sqlite.c`, which generates the iTunesCDB used by iPod
  nano 5G, iPod Touch, iPhone, etc.
- SPEC §7 declares those devices out of scope for v1 — we target the
  iPod Classic 7G, which uses the original iTunesDB format handled
  entirely by `itdb_itunesdb.c`. No capability lost.

Files modified:
- `configure.ac` — `PKG_CHECK_MODULES(LIBGPOD, glib-2.0 >= 2.8.0 gobject-2.0 sqlite3 libplist >= 1.0)` → `PKG_CHECK_MODULES(LIBGPOD, glib-2.0 >= 2.8.0 gobject-2.0 gmodule-2.0)`
- `Makefile.am` — `SUBDIRS=src tools tests po m4 docs bindings` → `SUBDIRS=src tests po m4 docs bindings` (drops the `tools/` directory which contains the optional `ipod-read-sysinfo-extended` binary and `ipod-lockdown.c`; we don't ship tools).
- `src/Makefile.am` — drop `itdb_sqlite_queries.h` from `noinst_HEADERS` (the corresponding `itdb_sqlite.c` source file is kept in `libgpod_la_SOURCES`, but its contents are replaced — see below).
- `src/itdb_sqlite.c` — replaced with a stub translation unit that defines `itdb_sqlite_generate_itdbs(FExport *fexp)` as a no-op returning 0, so `itdb_itunesdb.c`'s call to it from `itdb_write` continues to link and succeed. The stub includes a comment explaining how to revert if iPod nano 5G / Touch / iPhone support is restored later.

### `0002-glib-2.88-gstatbuf-fix.patch`

Two call sites use `struct stat` to receive a `g_stat()` result, which
in modern GLib (2.88) is typed as `GStatBuf*` (= `struct _stat64*` on
MinGW). Fix: change the local declaration from `struct stat stat_buf;`
to `GStatBuf stat_buf;`. The pointer signature now matches and
`stat_buf.st_size` still resolves to the right field.

Files modified:
- `src/db-parse-context.c` — line 183
- `src/itdb_tzinfo.c` — line 246

## Build, install, vendor

```bash
# Source root: /c/src/libgpod (cloned in MSYS2)
NOCONFIGURE=1 ./autogen.sh                   # with ACLOCAL_FLAGS set
./configure <flags above>
make -j
make install                                  # to /mingw64
```

Produces `/mingw64/bin/libgpod-4.dll` (the libtool-versioned DLL),
`/mingw64/lib/libgpod.dll.a` (MinGW GNU-format import lib, unused by
MSVC), and `/mingw64/include/gpod-1.0/gpod/itdb.h`.

Vendored layout (under `F:\repos\ipod-sync\vendor\libgpod\`):

- `include/gpod/itdb.h` — copied verbatim from `/mingw64/include/gpod-1.0/gpod/`
- `bin/gpod.dll` — `/mingw64/bin/libgpod-4.dll` renamed to drop the libtool version suffix (so `build.rs` can `cargo:rustc-link-lib=dylib=gpod`)
- `bin/*.dll` — MinGW + GLib runtime closure (see "Runtime DLLs vendored" below)
- `lib/gpod.lib` — MSVC-format import lib (see "MSVC import library" below)
- `lib/gpod.def` — the .def file fed to `lib /def`, kept for reproducibility and diff-ability of the exported symbol set (the binary `.exp` intermediate that `lib.exe` also emits is not committed; it's regenerable from `.def` + `.dll`)

## Runtime DLLs vendored (in `vendor/libgpod/bin/`)

16 DLLs total. Transitive closure verified with `dumpbin /dependents`
recursively — everything outside the Windows system DLL set
(`KERNEL32`, `MSVCRT`, `ADVAPI32`, etc., plus `api-ms-*`) is present
locally.

```
gpod.dll                  (was libgpod-4.dll)
libgcc_s_seh-1.dll        MinGW C runtime
libgcrypt-20.dll          load-bearing for iTunesDB hash signature on Classic 7G
libglib-2.0-0.dll
libgmodule-2.0-0.dll      pulled in by itdb_hashAB.c (dynamic-loading hash lib)
libgobject-2.0-0.dll
libgpg-error-0.dll        libgcrypt transitive dep
libgthread-2.0-0.dll
libffi-8.dll              libgobject transitive dep (only discovered in Step 9 closure check)
libiconv-2.dll            libintl transitive dep
libintl-8.dll
libpcre2-8-0.dll          libglib transitive dep
libstdc++-6.dll           MinGW C++ runtime (kept for safety even though libgpod is pure C)
libwinpthread-1.dll       MinGW pthreads
libxml2-16.dll
zlib1.dll
```

DLLs from the plan's list that were intentionally NOT vendored:
`libplist-*.dll`, `libsqlite3-0.dll` — no longer dependencies after
patches.

## MSVC import library

Generated via `dumpbin /exports` + `lib /def` per Step 10 of Task 3,
using VS Community 18 (Build Tools 14.51) at
`C:\Program Files\Microsoft Visual Studio\18\Community`. Source the
matching `vcvars64.bat` first so `dumpbin` and `lib` are on PATH.

Note on the "18": Visual Studio 18 is the successor to VS 2022
(internal version 17); it ships the MSVC 14.5x toolchain (vs 14.3x
in VS 17). The `.lib` archive format produced by `lib /def` is
unchanged from 14.3x, so this import library is compatible with any
MSVC 14.x consumer -- a Rust crate built with the
`x86_64-pc-windows-msvc` target on either VS 2022 or VS 18 will link
against it without rebuilding.

- 244 symbols exported from `bin/gpod.dll`
- All four required spike symbols present: `itdb_parse`, `itdb_parse_file`, `itdb_free`, `itdb_track_new`, plus `itdb_write` and `itdb_write_file`
- `lib/gpod.lib` machine type: `8664 (x64)`

## Smoke check (rerun before committing if anything changes)

From a Developer PowerShell:
```powershell
dumpbin /exports vendor\libgpod\bin\gpod.dll | Select-String "itdb_parse|itdb_write|itdb_free|itdb_track_new"
dumpbin /headers vendor\libgpod\lib\gpod.lib | Select-String "Machine"
```

---

## Phase 1 rebuild: gdk-pixbuf added (2026-05-17)

Phase 1's `itdb_track_set_thumbnails_from_data` was a silent no-op
because the Phase 0 libgpod was built without gdk-pixbuf available --
`itdb_artwork.c`, `itdb_photoalbum.c`, and `ithumb-writer.c` are
gated on `HAVE_GDKPIXBUF` in configure.ac, so they were compiled out.
The exported symbols still existed (the public function signatures
live in `itdb.h`) but their bodies returned FALSE/0 without setting
GError.

Fix: install gdk-pixbuf2 + image-format deps in MSYS2 and rebuild.
configure auto-detects via pkg-config -- no flag changes needed. The
new configure summary shows `Artwork support ..........: yes` (was
`no`) and the linker line gains `-lgdk_pixbuf-2.0`.

### New MSYS2 packages installed

- `mingw-w64-x86_64-gdk-pixbuf2 2.44.6-1` (load-bearing)
- `mingw-w64-x86_64-libpng 1.6.58-1`
- `mingw-w64-x86_64-libjpeg-turbo 3.1.4.1-3`
- `mingw-w64-x86_64-libtiff 4.7.1-1`

Pulled in transitively: `mingw-w64-x86_64-libwebp 1.6.0-1`,
`mingw-w64-x86_64-lerc 4.1.0-1`, `mingw-w64-x86_64-giflib 6.1.3-1`,
`mingw-w64-x86_64-jbigkit 2.1-5`, `mingw-w64-x86_64-libdeflate 1.25-1`.

### Extra source patches for the pixbuf code paths

These translation units were not compiled in Phase 0 (so were not in
the original patches), but failed against modern GLib / GCC when
enabled. All are mechanical and follow the same patterns as the
Phase 0 patches.

- `src/itdb_artwork.c` (line 155): `struct stat statbuf;` -> `GStatBuf statbuf;` (same g_stat signature change as Phase 0 patch 2).
- `src/itdb_photoalbum.c` (line 419): same fix, tab-indented.
- `src/ithumb-writer.c` (line 1212): same fix.
- `src/ithumb-writer.c` (lines 731, 971): wrap `g_object_ref (G_OBJECT (...))` calls with `GDK_PIXBUF (...)`. Modern GLib types `g_object_ref` to return `GObject*` (was `gpointer` in older versions); assigning to a `GdkPixbuf*` local and returning from a `GdkPixbuf*`-returning function now requires the cast.

These are committed as a separate patch file (see `patches/0003-pixbuf-codepaths-glib-2.88-fixes.patch`).

`tests/test-photos.c` also fails to build (a `mkdir(path, mode)` call;
MinGW's mkdir takes one arg). We `make install` from the `src/`
subdirectory only to skip the tests build. The library itself is
fine.

### Configure (unchanged from Phase 0)

```bash
./configure --prefix=/mingw64 --disable-static --without-hal \
            --disable-gtk-doc --disable-introspection \
            --without-python --disable-more-warnings
```

pkg-config now finds gdk-pixbuf-2.0; no explicit flag needed.

### New / updated vendored DLLs (vendor/libgpod/bin/)

`gpod.dll` itself grew from 1,112,684 to 1,495,283 bytes as the
artwork code is now linked in.

New runtime DLLs added (12 total):
```
libgdk_pixbuf-2.0-0.dll
libpng16-16.dll
libjpeg-8.dll
libtiff-6.dll
libwebp-7.dll
libsharpyuv-0.dll
liblzma-5.dll
libzstd.dll
libdeflate.dll
libgio-2.0-0.dll        (pixbuf init pulls GIO for module loading)
libjbig-0.dll           (libtiff transitive)
libLerc.dll             (libtiff transitive)
```

Total vendored DLL count: 16 (Phase 0) -> 28.

### gdk-pixbuf loader plugin bundle

gdk-pixbuf uses a plugin architecture: each image format is a separate
DLL under `<prefix>/lib/gdk-pixbuf-2.0/2.10.0/loaders/`, and pixbuf
finds them via a text manifest (`loaders.cache`) referenced by the
`GDK_PIXBUF_MODULE_FILE` env var.

We vendor the full set of 13 loader DLLs under
`vendor/libgpod/pixbuf-loaders/` along with a `loaders.cache` file
generated by `gdk-pixbuf-query-loaders.exe`:

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
$loaders = Get-ChildItem F:\repos\ipod-sync\vendor\libgpod\pixbuf-loaders\*.dll
& "C:\msys64\mingw64\bin\gdk-pixbuf-query-loaders.exe" $loaders `
    > F:\repos\ipod-sync\vendor\libgpod\pixbuf-loaders\loaders.cache
```

`build.rs` copies the `pixbuf-loaders/` directory next to the exe at
build time and exports `PIXBUF_LOADERS_CACHE` as a compile-time env
holding the absolute path to `loaders.cache`. `main.rs` reads that
via `env!()` and `std::env::set_var("GDK_PIXBUF_MODULE_FILE", ...)`
before any libgpod call.

**Distribution caveat:** `loaders.cache` contains absolute paths to
`F:\repos\ipod-sync\vendor\libgpod\pixbuf-loaders\*.dll` -- it's a
local-dev artifact. For real distribution we'd ship the loaders DLLs
alongside the exe and regenerate the cache at install time (or use
GDK_PIXBUF_MODULEDIR + an empty cache, which makes pixbuf scan the
dir at startup; slower but no-cache-file). Out of scope for Phase 1.

### Acceptance check (Phase 1 spike)

```
.\target\debug\ipod-sync.exe "<some.flac>"
```
Expected: track count increments AND `G:\iPod_Control\Artwork\`
contains new `F*_*.ithmb` thumbnail blob files (= libgpod successfully
decoded the JPEG cover and rendered iPod-format thumbnails). The
`** (process:N): CRITICAL **: itdb_splr_validate: ...` warning is
benign (smart-playlist validation on an empty smart-playlist list).

## libtiff rebuild: dropped GPL-licensed libjbig dep (2026-05-26)

`mingw-w64-x86_64-libtiff` from MSYS2 is built with JBIG support and pulls in
`libjbig-0.dll` (jbigkit), which is **GPL-2.0-only**. Unlike LGPL, GPL has no
dynamic-linking safe harbour — shipping libjbig forces the entire project
under GPL. libgpod's artwork paths write JPEGs and never touch JBIG2-compressed
TIFFs, so libjbig is dead weight here.

Fixed by rebuilding libtiff 4.7.1 from upstream source with JBIG disabled,
then swapping the vendored DLL.

### Procedure

```
# In an MSYS2 MinGW64 shell:
pacman -S --needed mingw-w64-x86_64-cmake
curl -LO https://download.osgeo.org/libtiff/tiff-4.7.1.tar.gz
tar xf tiff-4.7.1.tar.gz
cd tiff-4.7.1
cmake -B out -G "MinGW Makefiles" \
    -DCMAKE_BUILD_TYPE=Release \
    -Djbig=OFF \
    -DBUILD_SHARED_LIBS=ON
cmake --build out -j
cp out/libtiff/libtiff.dll <repo>/crates/classick/vendor/libgpod/bin/libtiff-6.dll
rm <repo>/crates/classick/vendor/libgpod/bin/libjbig-0.dll
```

The output `libtiff.dll` is renamed to `libtiff-6.dll` because gdk-pixbuf
imports libtiff by that exact filename (encoded in its import table). The
DLL's own internal export name is unaffected — Windows resolves imports by
the on-disk filename.

### Verification

`objdump -p libtiff-6.dll | grep "DLL Name"` should not list `libjbig`. The
only DLL in `vendor/libgpod/bin/` that imported libtiff was
`libgdk_pixbuf-2.0-0.dll`; the only DLL that imported libjbig was libtiff
itself, so removing libjbig is safe once libtiff no longer references it.

---

## macOS build (2026-07-12)

Built from source on macOS via `scripts/build-libgpod-macos.sh`. No Homebrew
`libgpod` formula exists — the script clones the fadingred fork, applies the
same three Windows patches unchanged (no macOS-specific patch was required),
and installs to a repo-local prefix for pkg-config discovery.

### Source and patches

- Repo: `https://github.com/fadingred/libgpod.git`
- Commit SHA: `4a8a33ef4bc58eee1baca6793618365f75a5c3fa` (0.8.0)
- Patches: applied from `vendor/libgpod/patches/0001..0003` cleanly with no macOS-specific changes

### Build dependencies (Homebrew)

```
glib gdk-pixbuf libgcrypt libxml2 pkg-config autoconf automake libtool intltool gtk-doc
```

`libgcrypt` is mandatory — it provides the hash signatures required for
iPod Classic 7G iTunesDB signing. `ffmpeg` is a runtime dependency (not build-time).

### Autotools configuration

macOS autotools required environment variables not present in Homebrew's defaults
because libgpod's old `autogen.sh` version-hunts for unversioned automake (finds 1.17
vs the sought 1.4–1.9). Fixed in the build script by exporting:

```bash
export AUTOMAKE=automake AUTOCONF=autoconf ACLOCAL=aclocal AUTOHEADER=autoheader LIBTOOLIZE=glibtoolize
export ACLOCAL_PATH="$(brew --prefix)/share/aclocal"
export ACLOCAL_FLAGS="-I $(brew --prefix)/share/aclocal"
```

(On macOS, Homebrew's libtool is named `glibtool`, and we export `LIBTOOLIZE=glibtoolize`
to match.)

### Configure flags

Identical to Windows, no macOS-specific flags:

```
./configure --prefix=<repo-local-prefix> --disable-static --without-hal \
            --disable-gtk-doc --disable-introspection \
            --without-python --disable-more-warnings
```

Result: `configure` reports `Artwork support ..........: yes` (confirming gdk-pixbuf
was found).

### Installation and linking

Installs to `crates/classick/vendor/libgpod/macos-prefix/` (gitignored). The prefix
is gitignored; it's regenerated by the build script on-demand.

`build.rs` exposes the prefix via `PKG_CONFIG_PATH` so `pkg-config libgpod-1.0` resolves
library + include paths. A gitignored `.cargo/config.toml` sets `PKG_CONFIG_PATH` for
development builds.

macOS **links `libgpod.dylib` directly** — no import-library (`.lib`) or `.def` dance,
and no vendored runtime-DLL closure (that was a Windows MinGW/MSVC concern unique to
static-linking and distributed .NET bundles). Dev builds link against Homebrew's
`glib`, `gdk-pixbuf`, and transitive dylib dependencies; no vendoring needed.

### Hash algorithm notes

iPod Classic uses `hash58` (compiled into libgpod), so Classic signing works without
external blobs. The `hashAB` algorithm (used only for nano 4G / 5G) dynamically loads
an external blob and is out of scope for this project.
