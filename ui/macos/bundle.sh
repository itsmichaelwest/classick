#!/usr/bin/env bash
# Build Classick.app via the Xcode project (Classick.xcodeproj, generated from
# project.yml by XcodeGen). xcodebuild produces the .app, embeds the classick
# daemon (a build phase), and ad-hoc signs it. Result is copied to
# ui/macos/Classick.app for convenience (`open ui/macos/Classick.app`).
#
# Real Developer ID signing + notarization + .dmg is SP3.
set -euo pipefail
cd "$(dirname "$0")"

CONFIG="${1:-Debug}"

# The app embeds + spawns the daemon; make sure it's built.
if [ ! -f ../../target/release/classick ]; then
  echo "==> building daemon (cargo build --release)"
  ( cd ../.. && cargo build --release )
fi

# Regenerate the project from project.yml if XcodeGen is available (keeps the
# committed .xcodeproj in sync); otherwise use the committed one as-is.
if command -v xcodegen >/dev/null 2>&1; then
  xcodegen generate >/dev/null
fi

echo "==> xcodebuild ($CONFIG)"
xcodebuild -project Classick.xcodeproj -scheme Classick -configuration "$CONFIG" \
  -derivedDataPath .build-xcode build >/dev/null

SRC=".build-xcode/Build/Products/$CONFIG/Classick.app"
rm -rf Classick.app
cp -R "$SRC" Classick.app
echo "built $PWD/Classick.app"
