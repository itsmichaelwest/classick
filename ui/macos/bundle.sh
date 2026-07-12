#!/usr/bin/env bash
# Build the Classick executable via SwiftPM and assemble Classick.app around it
# (LSUIElement menu-bar agent). Ad-hoc signed for dev; real Developer ID
# signing + notarization is SP3. See ui/macos/README.md.
set -euo pipefail
cd "$(dirname "$0")"

CONFIG="${1:-release}"
echo "==> swift build -c $CONFIG"
swift build -c "$CONFIG"

APP="Classick.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp Info.plist "$APP/Contents/Info.plist"
cp ".build/$CONFIG/Classick" "$APP/Contents/MacOS/Classick"

# Dev convenience: embed the freshly built daemon binary so the app can spawn
# it from Contents/Resources. In SP3 this (plus the libgpod dylib closure) is
# done properly with signing.
if [ -f ../../target/release/classick ]; then
  cp ../../target/release/classick "$APP/Contents/Resources/classick"
else
  echo "warn: ../../target/release/classick not found (run: cargo build --release)"
fi

# Ad-hoc sign so the bundle runs and (best-effort) can register notifications.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || echo "warn: ad-hoc codesign failed"

echo "built $PWD/$APP"
