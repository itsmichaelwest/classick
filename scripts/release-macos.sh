#!/usr/bin/env bash
# One-command local macOS release: build → bundle dylib closure → Developer ID
# sign (inside-out, hardened runtime) → notarize + staple → dmg → appcast.
# Secrets live only in the Keychain (signing identity + `classick-notary`
# notarytool profile); nothing sensitive is read from or written to the repo.
#
# Usage: scripts/release-macos.sh <version>   e.g.  scripts/release-macos.sh 0.1.0
#        RELEASE_GH=1 scripts/release-macos.sh 0.1.0   # also `gh release create`
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION="${1:?usage: release-macos.sh <version>}"

APP="ui/macos/Classick.app"
ENTITLEMENTS="ui/macos/Classick.entitlements"
DIST="dist"
DMG="$DIST/Classick-$VERSION.dmg"

# Developer ID Application identity (derived from the Keychain — not hardcoded).
ID="$(security find-identity -v -p codesigning | awk -F'"' '/Developer ID Application/{print $2; exit}')"
[ -n "$ID" ] || { echo "no Developer ID Application identity in Keychain"; exit 1; }
echo "==> signing identity: $ID"

echo "==> [1/8] build daemon (cargo)"
cargo build --release

echo "==> [2/8] build app (xcodebuild, Developer ID + hardened runtime)"
( cd ui/macos && xcodegen generate >/dev/null )
rm -rf "$APP"
xcodebuild -project ui/macos/Classick.xcodeproj -scheme Classick -configuration Release \
  -derivedDataPath ui/macos/.build-xcode \
  CODE_SIGN_IDENTITY="$ID" CODE_SIGN_STYLE=Manual ENABLE_HARDENED_RUNTIME=YES \
  OTHER_CODE_SIGN_FLAGS="--timestamp" build >/dev/null
cp -R "ui/macos/.build-xcode/Build/Products/Release/Classick.app" "$APP"
# Re-embed the daemon (xcodebuild's phase already did, but ensure it's the fresh one).
cp target/release/classick "$APP/Contents/Resources/classick"

echo "==> [3/8] bundle libgpod dylib closure (relocatable @rpath)"
scripts/bundle-macos-libs.sh "$APP"

echo "==> [4/8] sign inside-out (dylibs + daemon), then the app"
while IFS= read -r f; do
  codesign --force --options runtime --timestamp --sign "$ID" "$f"
done < <(find "$APP/Contents/Frameworks" -name '*.dylib'; echo "$APP/Contents/Resources/classick")
# Seal the app last (Sparkle.framework was already signed by xcodebuild).
codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$ID" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

echo "==> [5/8] create dmg"
mkdir -p "$DIST"; rm -f "$DMG"
create-dmg --volname "Classick $VERSION" --app-drop-link 450 180 --window-size 640 400 \
  --icon "Classick.app" 190 180 "$DMG" "$APP" >/dev/null
codesign --force --timestamp --sign "$ID" "$DMG"

echo "==> [6/8] notarize + staple (this waits on Apple; a few minutes)"
xcrun notarytool submit "$DMG" --keychain-profile classick-notary --wait
xcrun stapler staple "$APP"
xcrun stapler staple "$DMG"

echo "==> [7/8] gatekeeper assessment"
spctl -a -vv -t install "$DMG" || true
codesign --verify --deep --strict "$APP" && echo "app signature OK"

echo "==> [8/8] Sparkle appcast (EdDSA-signs the update using the Keychain key)"
SIGN_UPDATE="ui/macos/.build-xcode/SourcePackages/artifacts/sparkle/Sparkle/bin/sign_update"
GEN_APPCAST="ui/macos/.build-xcode/SourcePackages/artifacts/sparkle/Sparkle/bin/generate_appcast"
if [ -x "$GEN_APPCAST" ]; then
  "$GEN_APPCAST" "$DIST" || echo "  (generate_appcast: check output; appcast.xml written to $DIST)"
fi

if [ "${RELEASE_GH:-0}" = "1" ]; then
  echo "==> gh release create v$VERSION"
  gh release create "v$VERSION" "$DMG" --title "Classick $VERSION" --notes "Classick $VERSION"
fi

echo "==> done: $DMG"
