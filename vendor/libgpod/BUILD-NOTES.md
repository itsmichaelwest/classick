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

- 244 symbols exported from `bin/gpod.dll`
- All four required spike symbols present: `itdb_parse`, `itdb_parse_file`, `itdb_free`, `itdb_track_new`, plus `itdb_write` and `itdb_write_file`
- `lib/gpod.lib` machine type: `8664 (x64)`

## Smoke check (rerun before committing if anything changes)

From a Developer PowerShell:
```powershell
dumpbin /exports vendor\libgpod\bin\gpod.dll | Select-String "itdb_parse|itdb_write|itdb_free|itdb_track_new"
dumpbin /headers vendor\libgpod\lib\gpod.lib | Select-String "Machine"
```
