#!/usr/bin/env bash
# Make Classick.app self-contained: copy the classick daemon's non-system dylib
# closure (libgpod + glib/gobject/gmodule/gdk-pixbuf/gettext/libxml2 + their
# transitive deps) into Contents/Frameworks and rewrite install-names to
# @rpath so nothing points at /opt/homebrew or the build tree. Run after
# xcodebuild + before signing.
#
# Usage: scripts/bundle-macos-libs.sh <path/to/Classick.app>
set -euo pipefail
APP="${1:?usage: bundle-macos-libs.sh <Classick.app>}"
BIN="$APP/Contents/Resources/classick"
FW="$APP/Contents/Frameworks"
[ -f "$BIN" ] || { echo "no daemon at $BIN — did bundle.sh run?"; exit 1; }
mkdir -p "$FW"

# List non-system dylib deps of a binary.
deps() {
  otool -L "$1" | tail -n +2 | awk '{print $1}' \
    | grep -vE '^/usr/lib/|^/System/|^@' || true
}

# BFS the closure, copying each dylib into Frameworks once. macOS ships bash
# 3.2 (no associative arrays), so dedup on "already copied into Frameworks?".
queue=("$BIN")
while [ ${#queue[@]} -gt 0 ]; do
  cur="${queue[0]}"; queue=("${queue[@]:1}")
  while IFS= read -r dep; do
    [ -z "$dep" ] && continue
    base="$(basename "$dep")"
    if [ ! -f "$FW/$base" ]; then
      cp -f "$dep" "$FW/$base"
      chmod u+w "$FW/$base"
      queue+=("$FW/$base")
    fi
  done < <(deps "$cur")
done

# Add an rpath to the daemon so @rpath/... resolves to Frameworks.
install_name_tool -add_rpath "@loader_path/../Frameworks" "$BIN" 2>/dev/null || true

# Rewrite each bundled dylib's own id, and every dep reference in the daemon +
# the dylibs, to @rpath/<basename>.
for f in "$FW"/*.dylib; do
  install_name_tool -id "@rpath/$(basename "$f")" "$f"
done
for f in "$BIN" "$FW"/*.dylib; do
  while IFS= read -r dep; do
    [ -z "$dep" ] && continue
    install_name_tool -change "$dep" "@rpath/$(basename "$dep")" "$f" 2>/dev/null || true
  done < <(deps "$f")
done

echo "bundled $(ls "$FW"/*.dylib 2>/dev/null | wc -l | tr -d ' ') dylibs into $FW"
