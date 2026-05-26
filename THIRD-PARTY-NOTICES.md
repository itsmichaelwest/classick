# Third-Party Notices

Classick is distributed under the [MIT License](LICENSE). It bundles or links
against the following third-party components. Their full license texts are
preserved under [`licenses/`](licenses/) and their copyrights remain with the
respective authors.

This software is based in part on the work of the Independent JPEG Group.

---

## Vendored native libraries (shipped as DLLs alongside `classick.exe`)

These ride along in the `target/<profile>/` output and inside any packaged
build. All are dynamically linked and replaceable on disk.

### LGPL-2.1-or-later

These libraries are LGPL-2.1-or-later. We satisfy the LGPL by:

- Dynamic linking (LGPL §6(b)) — each library ships as a separate DLL the
  user can replace with an interface-compatible version
- Preserving the libraries' identity and license below
- Including the full LGPL-2.1 text at [`licenses/LGPL-2.1.txt`](licenses/LGPL-2.1.txt)
- The source-code offer immediately below this section

| Library | DLL(s) | Source | License text |
|---|---|---|---|
| libgpod 0.8.0 (vendored fork) | `gpod.dll` | https://github.com/fadingred/libgpod | [`licenses/libgpod.txt`](licenses/libgpod.txt) (mirrors LGPL-2.1) |
| GLib 2.x | `libglib-2.0-0.dll`, `libgobject-2.0-0.dll`, `libgmodule-2.0-0.dll`, `libgthread-2.0-0.dll`, `libgio-2.0-0.dll` | https://gitlab.gnome.org/GNOME/glib | LGPL-2.1 (see [`licenses/LGPL-2.1.txt`](licenses/LGPL-2.1.txt)) |
| gdk-pixbuf 2.x | `libgdk_pixbuf-2.0-0.dll` + loader DLLs in `pixbuf-loaders/` | https://gitlab.gnome.org/GNOME/gdk-pixbuf | LGPL-2.1 |
| libgcrypt | `libgcrypt-20.dll` | https://www.gnupg.org/software/libgcrypt/ | LGPL-2.1 |
| libgpg-error | `libgpg-error-0.dll` | https://www.gnupg.org/software/libgpg-error/ | LGPL-2.1 |
| gettext runtime | `libintl-8.dll` | https://www.gnu.org/software/gettext/ | LGPL-2.1 |
| libiconv | `libiconv-2.dll` | https://www.gnu.org/software/libiconv/ | LGPL-2.1 |

Local source modifications to libgpod (patches we wrote) live in
`crates/classick/vendor/libgpod/patches/`. The remaining LGPL libraries
above are unmodified upstream MSYS2 builds.

### Written offer for LGPL source code

For three years from the date you obtained a binary distribution of Classick,
the copyright holder named in [LICENSE](LICENSE) will, on request, provide
the complete machine-readable source code for any LGPL-2.1-or-later library
shipped with that binary, for no more than the cost of physically performing
the distribution (i.e. media + postage, or free if delivered electronically).
Contact: open an issue on the GitHub repository, or email the address on the
maintainer's GitHub profile.

This satisfies LGPL-2.1 §6(c).

### Permissive licenses (MIT / BSD / libtiff / libpng / IJG / zlib / 0BSD / Apache-2.0)

| Library | DLL(s) | License | License text |
|---|---|---|---|
| libffi | `libffi-8.dll` | MIT | [`licenses/libffi.txt`](licenses/libffi.txt) |
| libxml2 | `libxml2-16.dll` | MIT | [`licenses/libxml2.txt`](licenses/libxml2.txt) |
| libdeflate | `libdeflate.dll` | MIT | [`licenses/libdeflate.txt`](licenses/libdeflate.txt) |
| PCRE2 | `libpcre2-8-0.dll` | BSD-3-Clause | [`licenses/libpcre2.txt`](licenses/libpcre2.txt) |
| libwebp + libsharpyuv | `libwebp-7.dll`, `libsharpyuv-0.dll` | BSD-3-Clause | [`licenses/libwebp.txt`](licenses/libwebp.txt) |
| Zstandard | `libzstd.dll` | BSD-3-Clause (selected over the dual GPL-2.0 option) | [`licenses/libzstd.txt`](licenses/libzstd.txt) |
| libtiff (rebuilt locally with `-Djbig=OFF`) | `libtiff-6.dll` | libtiff (BSD-style) | [`licenses/libtiff.txt`](licenses/libtiff.txt) |
| libpng | `libpng16-16.dll` | libpng License | [`licenses/libpng.txt`](licenses/libpng.txt) |
| libjpeg-turbo | `libjpeg-8.dll` | IJG + Modified BSD | [`licenses/libjpeg-turbo.txt`](licenses/libjpeg-turbo.txt) |
| LERC | `libLerc.dll` | Apache-2.0 | [`licenses/LERC.txt`](licenses/LERC.txt) |
| zlib | `zlib1.dll` | zlib License | [`licenses/zlib.txt`](licenses/zlib.txt) |
| XZ Utils (liblzma) | `liblzma-5.dll` | 0BSD (liblzma proper) — see file for per-component breakdown | [`licenses/xz-utils.txt`](licenses/xz-utils.txt) |
| mingw-w64 winpthreads | `libwinpthread-1.dll` | MIT / Public Domain / ZPL-2.1 (mixed permissive) | [`licenses/mingw-winpthreads.txt`](licenses/mingw-winpthreads.txt) |

### GCC runtime libraries

`libgcc_s_seh-1.dll` and `libstdc++-6.dll` are GPL-3.0 WITH GCC-Runtime-Library-Exception-3.1. The exception permits redistribution alongside any binary regardless of the binary's license. Exception text: [`licenses/gcc-runtime-exception.txt`](licenses/gcc-runtime-exception.txt).

---

## Rust crate dependencies

All Rust dependencies declared in `crates/classick/Cargo.toml` are permissively
licensed (MIT, Apache-2.0, BSD-3-Clause, Unlicense, or CC0). Run
`cargo tree --format '{p} {l}'` for the full transitive picture.

## .NET package dependencies

All NuGet packages referenced from `ui/windows/*/*.csproj` are MIT-licensed
(Microsoft.WindowsAppSDK, CommunityToolkit.Mvvm, H.NotifyIcon.WinUI, etc.).
Test-only packages (xUnit, Microsoft.NET.Test.Sdk, coverlet.collector) are not
redistributed with the application.

## Subprocess dependencies (not bundled)

Classick spawns these tools at runtime when the user provides them on PATH or
via `--ffmpeg` / `--refalac-path`. They are not redistributed with the binary,
so their licenses do not apply to our distribution.

- **ffmpeg** — LGPL-2.1-or-later (default upstream build) or GPL depending on
  build flags. The user supplies their own.
- **refalac** (qaac) — proprietary Apple CoreAudio + permissive wrapper.
  Optional, opt-in via `--encoder refalac`.
