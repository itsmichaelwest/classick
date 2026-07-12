#!/usr/bin/env bash
# Build libgpod 0.8.0 (fadingred fork) from source on macOS and install it to
# a repo-local prefix so build.rs's pkg-config path can link it. Mirrors
# vendor/libgpod/BUILD-NOTES.md (the Windows/MSYS2 build) with the same source
# SHA and the same 3 patches. libgcrypt is mandatory (Classic 7G iTunesDB
# signature). See docs/superpowers/specs/2026-07-12-macos-core-enablement-design.md.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PATCHES_DIR="$REPO_ROOT/crates/classick/vendor/libgpod/patches"
PREFIX="${LIBGPOD_PREFIX:-$REPO_ROOT/crates/classick/vendor/libgpod/macos-prefix}"
SRC_SHA="4a8a33ef4bc58eee1baca6793618365f75a5c3fa"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "==> Checking Homebrew build deps"
for f in glib gdk-pixbuf libgcrypt libxml2 pkg-config autoconf automake libtool intltool gtk-doc; do
  brew list --formula "$f" >/dev/null 2>&1 || { echo "MISSING: brew install $f"; exit 1; }
done

BREW_PREFIX="$(brew --prefix)"
export PKG_CONFIG_PATH="$BREW_PREFIX/lib/pkgconfig:$BREW_PREFIX/share/pkgconfig:${PKG_CONFIG_PATH:-}"
# Homebrew keeps gettext/libxml2 keg-only; expose their pkgconfig too.
for keg in gettext libxml2 libffi; do
  [ -d "$BREW_PREFIX/opt/$keg/lib/pkgconfig" ] && PKG_CONFIG_PATH="$BREW_PREFIX/opt/$keg/lib/pkgconfig:$PKG_CONFIG_PATH"
done
export PKG_CONFIG_PATH

echo "==> Cloning fadingred/libgpod @ $SRC_SHA"
git clone https://github.com/fadingred/libgpod.git "$WORK/libgpod"
git -C "$WORK/libgpod" checkout "$SRC_SHA"

echo "==> Applying patches"
for p in "$PATCHES_DIR"/000*.patch; do
  echo "    - $(basename "$p")"
  git -C "$WORK/libgpod" apply "$p"
done

echo "==> autogen + configure (prefix: $PREFIX)"
cd "$WORK/libgpod"
# libgpod's old autogen.sh version-hunts for automake-1.4..1.9 and BSD-vs-GNU
# libtoolize. Point it at Homebrew's unversioned tools (the documented escape
# hatch) and expose Homebrew's aclocal macros (pkg-config, glib, gtk-doc,
# intltool). macOS Homebrew libtoolize is `glibtoolize`.
export AUTOMAKE=automake AUTOCONF=autoconf ACLOCAL=aclocal AUTOHEADER=autoheader
export LIBTOOLIZE=glibtoolize
export ACLOCAL_PATH="$BREW_PREFIX/share/aclocal${ACLOCAL_PATH:+:$ACLOCAL_PATH}"
export ACLOCAL_FLAGS="-I $BREW_PREFIX/share/aclocal"
NOCONFIGURE=1 ./autogen.sh
./configure --prefix="$PREFIX" --disable-static --without-hal \
            --disable-gtk-doc --disable-introspection \
            --without-python --disable-more-warnings

echo "==> make + install"
make -j"$(sysctl -n hw.ncpu)"
make install

PC="$PREFIX/lib/pkgconfig/libgpod-1.0.pc"
[ -f "$PC" ] || { echo "ERROR: $PC not produced"; exit 1; }
echo "==> Done. Add to your shell / cargo env:"
echo "    export PKG_CONFIG_PATH=\"$PREFIX/lib/pkgconfig:\$PKG_CONFIG_PATH\""
