# macOS Native Transcode + Packaging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ship a self-contained, Developer-ID-signed, notarized `Classick.dmg` with Sparkle auto-updates, published to GitHub Releases by one local command — after dropping ffmpeg on macOS in favour of `afconvert` + `lofty`.

**Architecture:** Phase A swaps the macOS transcode/probe backend (afconvert + lofty) behind the existing `ProbeOutput`/`transcode_to_alac` seams, `#[cfg(target_os = "macos")]`, Windows untouched. Phase B bundles the libgpod dylib closure into the `.app` with `@rpath` fixups, signs + notarizes everything, integrates Sparkle 2, and builds the dmg via a local release script.

**Tech Stack:** Rust (`lofty` crate + `afconvert`), Swift/Sparkle 2 (SPM), XcodeGen, `install_name_tool`, `codesign`, `notarytool`, `stapler`, `create-dmg`, `gh`.

## Global Constraints

- **macOS transcode is `#[cfg(target_os = "macos")]`.** Never change the Windows ffmpeg/ffprobe/refalac path. The shared seam is `ProbeOutput` (produced by both `probe` backends) and the `transcode_to_alac(src, dst, _) -> Result<()>` signature.
- **Encode:** `afconvert -f m4af -d alac <src> <dst>` (system binary `/usr/bin/afconvert`). No config path; the macOS default ignores `config.ffmpeg`.
- **Probe/tags:** the `lofty` crate. `ProbeOutput` has NO duration field — do not add one (libgpod computes duration). Map lofty → `ProbeTags` (title/artist/album/album_artist/date/track/track_total/disc/disc_total/genre/composer, all `Option<String>`); embedded picture → a synthetic `ProbeStream { codec_type: "video", disposition: attached_pic=1 }` so `has_embedded_art` works unchanged; audio codec → a `ProbeStream { codec_type: "audio", codec_name }` where ALAC-vs-AAC comes from lofty's `Mp4Codec`.
- **arm64-only.** `--arch arm64` where relevant.
- **Signing:** Developer ID Application (Team ID from the login Keychain), **hardened runtime**, sign **inside-out** (dylibs → daemon → Sparkle components → app). Ad-hoc is dev-only.
- **Sparkle keys:** `SUPublicEDKey` (public) goes in Info.plist; the **private EdDSA key stays in the Keychain and is NEVER committed**. Same for the notarization credential (keychain profile).
- **Appcast** on GitHub Pages; `SUFeedURL` points at it; binaries on GitHub Releases.
- **Bundle self-containment:** after B1, `otool -L` on every bundled binary/dylib shows only `@rpath`/`/usr/lib`/`/System` — no `/opt/homebrew`, no repo paths. The app must run with Homebrew off `PATH`.
- **No secrets in the repo.** No `.p12`, no private keys, no app-specific passwords.

---

## File Structure

- `crates/classick/Cargo.toml` **(modify)** — add `lofty` to the macOS target deps.
- `crates/classick/src/transcode.rs` **(modify)** — macOS `probe`, `transcode_to_alac`, `extract_cover_art`, `verify_tools_available` behind cfg; add `macos_probe.rs` submodule for the lofty→ProbeOutput mapping.
- `crates/classick/src/transcode/macos_probe.rs` **(create)** — `probe_output_from_lofty(path) -> Result<ProbeOutput>` (pure-ish, unit-tested against a FLAC fixture).
- `scripts/bundle-macos-libs.sh` **(create)** — dylib closure → Frameworks + `install_name_tool` fixups.
- `scripts/release-macos.sh` **(create)** — the full local release pipeline.
- `ui/macos/project.yml` **(modify)** — Sparkle SPM package + dependency.
- `ui/macos/Info.plist` **(modify)** — `SUFeedURL`, `SUPublicEDKey`, `SUEnableAutomaticChecks`.
- `ui/macos/Sources/Classick/Updates/Updater.swift` **(create)** — `SPUStandardUpdaterController` wrapper.
- `ui/macos/Sources/Classick/Views/MenuContent.swift` **(modify)** — "Check for Updates…" row.
- `ui/macos/Sources/Classick/ClassickApp.swift` **(modify)** — own the updater; wire the menu action.
- `ui/macos/Classick.entitlements` **(create)** — hardened-runtime entitlements.
- `appcast.xml` + `docs/` (GitHub Pages) **(create)** — Sparkle feed.
- `ui/macos/README.md`, `crates/classick/vendor/libgpod/BUILD-NOTES.md` **(modify)** — release docs.

---

## PHASE A — Native macOS transcode

### Task A1: `lofty` dep + macOS probe (lofty → ProbeOutput)

**Files:**
- Modify: `crates/classick/Cargo.toml`
- Create: `crates/classick/src/transcode/macos_probe.rs`
- Modify: `crates/classick/src/transcode.rs` (declare submodule; cfg the `probe` fn)

**Interfaces:**
- Produces: `#[cfg(target_os="macos")] pub fn probe_output_from_lofty(path: &Path) -> Result<ProbeOutput>` and a macOS `probe(src, _ffmpeg) -> Result<ProbeOutput>` delegating to it.
- Consumes: `ProbeOutput`, `ProbeFormat`, `ProbeTags`, `ProbeStream`, `ProbeDisposition` (unchanged).

- [ ] **Step 1: Add lofty (macOS target dep)**

In `crates/classick/Cargo.toml`, under `[target.'cfg(target_os = "macos")'.dependencies]`:

```toml
lofty = "0.22"
```

- [ ] **Step 2: Write the failing mapping test against a real FLAC fixture**

Copy a small tagged FLAC to `crates/classick/tests/fixtures/tagged.flac` (a real file with title/artist/album/track tags + embedded art — reuse one from the library). In `macos_probe.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_flac_tags_codec_and_art() {
        let p = probe_output_from_lofty(std::path::Path::new(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tagged.flac"))).unwrap();
        let tags = p.format.tags.as_ref().unwrap();
        assert!(tags.title.is_some());
        assert!(tags.artist.is_some());
        assert!(tags.album.is_some());
        assert!(p.streams.iter().any(|s| s.codec_type == "audio" && s.codec_name.as_deref() == Some("flac")));
        assert!(crate::transcode::has_embedded_art(&p)); // if the fixture has art
    }
}
```

- [ ] **Step 3: Run — verify it fails to compile**

Run: `cargo test -p classick --target-dir target maps_flac_tags` (on macOS)
Expected: FAIL (function undefined).

- [ ] **Step 4: Implement `probe_output_from_lofty`**

```rust
use crate::transcode::{ProbeDisposition, ProbeFormat, ProbeOutput, ProbeStream, ProbeTags};
use anyhow::{Context, Result};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::*;
use std::path::Path;

pub fn probe_output_from_lofty(path: &Path) -> Result<ProbeOutput> {
    let tagged = lofty::read_from_path(path)
        .with_context(|| format!("lofty read {}", path.display()))?;

    // Audio codec: FileType → codec_name; ALAC vs AAC needs the Mp4 codec.
    let codec_name = codec_name_for(&tagged, path);
    let format_name = Some(format!("{:?}", tagged.file_type()).to_lowercase());

    let mut tags = ProbeTags::default();
    if let Some(t) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        tags.title = t.get_string(&ItemKey::TrackTitle).map(str::to_owned);
        tags.artist = t.get_string(&ItemKey::TrackArtist).map(str::to_owned);
        tags.album = t.get_string(&ItemKey::AlbumTitle).map(str::to_owned);
        tags.album_artist = t.get_string(&ItemKey::AlbumArtist).map(str::to_owned);
        tags.date = t.get_string(&ItemKey::Year).or(t.get_string(&ItemKey::RecordingDate)).map(str::to_owned);
        tags.track = t.get_string(&ItemKey::TrackNumber).map(str::to_owned);
        tags.track_total = t.get_string(&ItemKey::TrackTotal).map(str::to_owned);
        tags.disc = t.get_string(&ItemKey::DiscNumber).map(str::to_owned);
        tags.disc_total = t.get_string(&ItemKey::DiscTotal).map(str::to_owned);
        tags.genre = t.get_string(&ItemKey::Genre).map(str::to_owned);
        tags.composer = t.get_string(&ItemKey::Composer).map(str::to_owned);
    }

    let mut streams = vec![ProbeStream { codec_type: "audio".into(), codec_name, disposition: None }];
    let has_pic = tagged.tags().iter().any(|t| !t.pictures().is_empty());
    if has_pic {
        streams.push(ProbeStream {
            codec_type: "video".into(),
            codec_name: Some("mjpeg".into()),
            disposition: Some(ProbeDisposition { attached_pic: Some(1) }),
        });
    }

    Ok(ProbeOutput { streams, format: ProbeFormat { format_name, tags: Some(tags) } })
}

fn codec_name_for(tagged: &lofty::file::TaggedFile, _path: &Path) -> Option<String> {
    use lofty::file::FileType;
    Some(match tagged.file_type() {
        FileType::Flac => "flac",
        FileType::Mpeg => "mp3",
        FileType::Vorbis => "vorbis",
        FileType::Opus => "opus",
        FileType::Wav => "pcm",
        FileType::Aiff => "pcm",
        FileType::Mp4 => return mp4_codec(tagged),   // alac vs aac
        _ => return None,
    }.to_owned())
}
```

(Implement `mp4_codec` via lofty's `Mp4File::codec()` → `Mp4Codec::Alac`→"alac", `Aac`→"aac". Confirm the exact lofty 0.22 `ItemKey`/`Mp4Codec` API against `cargo doc`/context7 and adjust names.)

Declare the submodule in `transcode.rs`: `#[cfg(target_os = "macos")] mod macos_probe;` and make the fields it uses (`ProbeStream.codec_type` etc.) constructible from it (they're `pub`).

- [ ] **Step 5: Add the macOS `probe` that delegates**

In `transcode.rs`, gate the existing ffprobe `probe` as `#[cfg(not(target_os = "macos"))]`, and add:

```rust
#[cfg(target_os = "macos")]
pub fn probe(src: &Path, _ffmpeg_path: &Path) -> Result<ProbeOutput> {
    macos_probe::probe_output_from_lofty(src)
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p classick maps_flac_tags` then `cargo test -p classick`
Expected: PASS; whole suite green on macOS.

- [ ] **Step 7: Commit**

```bash
git add crates/classick/Cargo.toml crates/classick/src/transcode.rs crates/classick/src/transcode/macos_probe.rs crates/classick/tests/fixtures/tagged.flac
git commit -m "transcode(macos): probe via lofty into ProbeOutput (drops ffprobe on macOS)"
```

---

### Task A2: Encode via `afconvert` + `verify_tools_available`

**Files:** Modify `crates/classick/src/transcode.rs`.

**Interfaces:** macOS `transcode_to_alac(src, dst, _ffmpeg) -> Result<()>` (afconvert) and `verify_tools_available(_ffmpeg) -> Result<()>` (checks afconvert).

- [ ] **Step 1: macOS transcode + verify**

Gate the ffmpeg versions `#[cfg(not(target_os="macos"))]`; add:

```rust
#[cfg(target_os = "macos")]
pub fn transcode_to_alac(src: &Path, dst: &Path, _ffmpeg_path: &Path) -> Result<()> {
    let status = Command::new("/usr/bin/afconvert")
        .args(["-f", "m4af", "-d", "alac"])
        .arg(src).arg(dst)
        .stdin(Stdio::null())
        .no_console()
        .status()
        .map_err(|e| anyhow!("failed to spawn afconvert: {e}"))?;
    if !status.success() {
        return Err(anyhow!("afconvert failed on {}", src.display()));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn verify_tools_available(_ffmpeg_path: &Path) -> Result<()> {
    if !Path::new("/usr/bin/afconvert").exists() {
        return Err(anyhow!("/usr/bin/afconvert not found"));
    }
    Ok(())
}
```

- [ ] **Step 2: Build + verify afconvert produces ALAC (integration, hardware-free)**

Add a macOS-gated integration test that transcodes the fixture FLAC to a temp `.m4a` and asserts success + non-empty output. (Encoding correctness is validated by the Phase A hardware gate.)

Run: `cargo test -p classick transcode` (macOS)
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/classick/src/transcode.rs
git commit -m "transcode(macos): FLAC->ALAC via afconvert (drops ffmpeg on macOS)"
```

---

### Task A3: Cover art via lofty (`extract_cover_art`)

**Files:** Modify `crates/classick/src/transcode.rs`.

**Interfaces:** macOS `extract_cover_art(src, dst, _ffmpeg) -> Result<()>` writes the first embedded picture's bytes to `dst`.

- [ ] **Step 1: Implement (macOS) + test**

Gate the ffmpeg version `#[cfg(not(target_os="macos"))]`; add a macOS version that reads the first `Picture` via lofty and writes `picture.data()` to `dst`. Add a test: extract from the fixture FLAC → assert `dst` exists + is a valid image header (JPEG `FFD8` or PNG `89504E47`).

Run: `cargo test -p classick cover_art` (macOS)
Expected: PASS.

- [ ] **Step 2: Commit**

```bash
git add crates/classick/src/transcode.rs
git commit -m "transcode(macos): extract embedded cover art via lofty"
```

---

### Task A4: cargo test green on macOS + no ffmpeg references

**Files:** any macOS-gated test cleanup.

- [ ] **Step 1: Full suite + grep for stray ffmpeg calls on macOS**

Run: `cargo test -p classick` and confirm green. Then confirm the macOS code path never spawns ffmpeg/ffprobe: `grep -rn "ffprobe\|Command::new(ffmpeg" crates/classick/src` and ensure every hit is `#[cfg(not(target_os="macos"))]` or Windows-only (refalac).

- [ ] **Step 2: Commit any gating fixes**

```bash
git add crates/classick/src
git commit -m "transcode(macos): ensure no ffmpeg/ffprobe on the macOS path"
```

---

### Task A5: Phase A hardware gate (manual)

- [ ] **Step 1: Sync a fresh album via the afconvert+lofty path**

Rebuild `cargo build --release`; via the app (or CLI) sync a FLAC album NOT already on the iPod (use `ipod-albums` to pick).

- [ ] **Step 2: Verify on-device**

Eject; on the iPod confirm the tracks **play**, show correct **artist/album/title**, and **artwork** renders. Re-sync → no-op.

- [ ] **Step 3: Record**

Add a `LEARNINGS.md` bullet (afconvert ALAC on iPod; any lofty tag quirk). Commit.

**Phase A DONE when an afconvert-encoded album plays on the iPod with correct metadata + art.**

---

## PHASE B — Packaging & distribution

### Task B1: Bundle the dylib closure (relocatable)

**Files:** Create `scripts/bundle-macos-libs.sh`.

- [ ] **Step 1: Write the closure-bundling script**

```bash
#!/usr/bin/env bash
# Copy the classick daemon's non-system dylib closure into <App>/Contents/Frameworks
# and rewrite install-names to @rpath so the app is self-contained. Arg: path to
# the built Classick.app. Run after xcodebuild + before signing.
set -euo pipefail
APP="${1:?usage: bundle-macos-libs.sh <Classick.app>}"
BIN="$APP/Contents/Resources/classick"
FW="$APP/Contents/Frameworks"
mkdir -p "$FW"

collect() { # recursively list non-system dylib deps of $1
  otool -L "$1" | tail -n +2 | awk '{print $1}' \
    | grep -vE '^/usr/lib/|^/System/|^@' || true
}

declare -A seen
queue=("$BIN")
while [ ${#queue[@]} -gt 0 ]; do
  cur="${queue[0]}"; queue=("${queue[@]:1}")
  while read -r dep; do
    [ -z "$dep" ] && continue
    base="$(basename "$dep")"
    if [ -z "${seen[$base]:-}" ]; then
      seen[$base]=1
      cp -f "$dep" "$FW/$base"
      chmod u+w "$FW/$base"
      queue+=("$FW/$base")
    fi
  done < <(collect "$cur")
done

# Rewrite ids + references to @rpath, and add an rpath to the daemon.
install_name_tool -add_rpath "@loader_path/../Frameworks" "$BIN" 2>/dev/null || true
for f in "$FW"/*.dylib; do
  install_name_tool -id "@rpath/$(basename "$f")" "$f"
done
for f in "$BIN" "$FW"/*.dylib; do
  while read -r dep; do
    [ -z "$dep" ] && continue
    install_name_tool -change "$dep" "@rpath/$(basename "$dep")" "$f"
  done < <(collect "$f")
done
echo "bundled ${#seen[@]} dylibs into $FW"
```

- [ ] **Step 2: Run it against a fresh build + verify self-containment**

Run:
```bash
cd ui/macos && ./bundle.sh && cd ../..
scripts/bundle-macos-libs.sh ui/macos/Classick.app
echo "=== any non-@rpath, non-system refs left? (should be none) ==="
for f in ui/macos/Classick.app/Contents/Resources/classick ui/macos/Classick.app/Contents/Frameworks/*.dylib; do
  otool -L "$f" | tail -n +2 | grep -E '/opt/homebrew|/Users/' && echo "LEAK in $f" || true
done
```
Expected: no `/opt/homebrew` or `/Users/` lines.

- [ ] **Step 3: Runtime check with Homebrew off PATH**

Run the daemon directly with a sanitized environment and confirm it starts + resolves libgpod:
`env -i HOME="$HOME" PATH=/usr/bin:/bin ui/macos/Classick.app/Contents/Resources/classick --help` (or a dry-run) → no dyld errors.

Also stage the **gdk-pixbuf loaders** into `Contents/Frameworks/gdk-pixbuf` and have the daemon set `GDK_PIXBUF_MODULE_DIR` to that path at startup (add a `#[cfg(target_os="macos")]` env set in `main.rs` before any libgpod call). Verify artwork on a synced track.

- [ ] **Step 4: Commit**

```bash
git add scripts/bundle-macos-libs.sh crates/classick/src/main.rs
git commit -m "build(macos): bundle libgpod dylib closure into the app (relocatable @rpath)"
```

---

### Task B2: Sparkle integration (framework + menu + keys)

**Files:** Modify `ui/macos/project.yml`, `Info.plist`, `MenuContent.swift`, `ClassickApp.swift`; create `Updates/Updater.swift`.

- [ ] **Step 1: Add Sparkle via SPM to the app target**

In `project.yml`:
```yaml
packages:
  Sparkle:
    url: https://github.com/sparkle-project/Sparkle
    from: "2.6.0"
```
and under the `Classick` target: `dependencies: - package: Sparkle`. Then `cd ui/macos && xcodegen generate`.

- [ ] **Step 2: Generate the EdDSA keypair**

Run Sparkle's `generate_keys` (from the built Sparkle SPM artifacts or `brew install --cask sparkle` tools). It prints the **public** key and stores the **private** key in the Keychain. Put the public key in `Info.plist`:
```xml
<key>SUPublicEDKey</key><string>PASTE_PUBLIC_KEY</string>
<key>SUFeedURL</key><string>https://<user>.github.io/classick/appcast.xml</string>
<key>SUEnableAutomaticChecks</key><true/>
```

- [ ] **Step 3: Updater wrapper + menu action**

`Updates/Updater.swift`:
```swift
import Sparkle
@MainActor final class Updater {
    let controller = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: nil, userDriverDelegate: nil)
    func checkForUpdates() {
        NSApp.activate(ignoringOtherApps: true)
        controller.checkForUpdates(nil)
    }
}
```
Own it in `AppDelegate`; add a `MenuContent` row `Button("Check for Updates…") { onCheckForUpdates() }` wired from the App body to `appDelegate.updater.checkForUpdates`.

- [ ] **Step 4: Build + smoke (dev, ad-hoc)**

Run: `cd ui/macos && ./bundle.sh && open Classick.app`; click "Check for Updates…" → Sparkle's UI appears (it will fail to find/verify a feed in dev — that's expected until B4/B5). Confirms wiring + entitlements load.

- [ ] **Step 5: Commit** (do NOT commit the private key)

```bash
git add ui/macos/project.yml ui/macos/Info.plist ui/macos/Classick.xcodeproj ui/macos/Sources/Classick/Updates/Updater.swift ui/macos/Sources/Classick/Views/MenuContent.swift ui/macos/Sources/Classick/ClassickApp.swift
git commit -m "feat(ui-macos): Sparkle auto-update integration (Check for Updates + public key + feed URL)"
```

---

### Task B3: Developer ID signing + hardened runtime

**Files:** Create `ui/macos/Classick.entitlements`; extend the release script's signing step.

- [ ] **Step 1: Entitlements**

`Classick.entitlements` (non-sandboxed, hardened-runtime friendly):
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.security.cs.allow-jit</key><false/>
  <key>com.apple.security.cs.disable-library-validation</key><false/>
</dict></plist>
```
(Library validation stays ON — everything is one Team ID. Add exceptions only if a nested unsigned component demands it.)

- [ ] **Step 2: Sign inside-out (a `sign-macos.sh` fragment used by the release script)**

```bash
ID="Developer ID Application: <YOUR NAME> (<TEAMID>)"
APP="ui/macos/Classick.app"
for f in "$APP"/Contents/Frameworks/*.dylib "$APP"/Contents/Resources/classick; do
  codesign --force --options runtime --timestamp --sign "$ID" "$f"
done
# Sparkle components (XPC services + Autoupdate + Updater.app inside the framework)
find "$APP"/Contents/Frameworks/Sparkle.framework -type f \( -name "*.xpc" -o -name "Autoupdate" -o -name "Updater.app" \) -prune -exec \
  codesign --force --options runtime --timestamp --sign "$ID" {} \;
codesign --force --options runtime --timestamp --entitlements ui/macos/Classick.entitlements \
  --sign "$ID" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
```
(Follow Sparkle's signing doc for the exact component list; adjust as its layout dictates.)

- [ ] **Step 3: Verify**

Run the two blocks above; expect `codesign --verify --deep --strict` to pass with no errors.

- [ ] **Step 4: Commit**

```bash
git add ui/macos/Classick.entitlements scripts/
git commit -m "build(macos): Developer ID inside-out signing + hardened runtime + entitlements"
```

---

### Task B4: Notarization + staple

**Files:** extend the release script.

- [ ] **Step 1: One-time notarytool credential (documented, not committed)**

`xcrun notarytool store-credentials classick-notary --apple-id <id> --team-id <TEAMID> --password <app-specific-password>` — stores a keychain profile. Document this; never commit the password.

- [ ] **Step 2: Submit + staple (against the dmg from B5, or the zipped app first)**

```bash
xcrun notarytool submit Classick.dmg --keychain-profile classick-notary --wait
xcrun stapler staple Classick.app
xcrun stapler staple Classick.dmg
spctl -a -vv -t install Classick.dmg    # expect: accepted, source=Notarized Developer ID
```

- [ ] **Step 3: Commit** (script changes only)

```bash
git add scripts/
git commit -m "build(macos): notarize + staple the app and dmg"
```

---

### Task B5: DMG + appcast

**Files:** extend the release script; create `appcast.xml`/Pages.

- [ ] **Step 1: create-dmg**

`brew install create-dmg`, then:
```bash
create-dmg --volname "Classick" --app-drop-link 450 160 --window-size 600 360 \
  Classick.dmg ui/macos/Classick.app
```

- [ ] **Step 2: Sign the dmg + generate appcast**

Sign the dmg (`codesign --sign "$ID" Classick.dmg`), then Sparkle's `generate_appcast <dir-of-release-archives>` (it EdDSA-signs each and writes/updates `appcast.xml`). Set the appcast's download URLs to the GitHub Release asset URLs.

- [ ] **Step 3: Host appcast on GitHub Pages**

Put `appcast.xml` under `docs/` (or a `gh-pages` branch); enable Pages; confirm `https://<user>.github.io/classick/appcast.xml` serves it. This URL must equal `SUFeedURL`.

- [ ] **Step 4: Commit**

```bash
git add appcast.xml docs/ scripts/
git commit -m "build(macos): create-dmg + Sparkle appcast on GitHub Pages"
```

---

### Task B6: `scripts/release-macos.sh` (the one command)

**Files:** Create `scripts/release-macos.sh`.

- [ ] **Step 1: Chain everything**

```bash
#!/usr/bin/env bash
set -euo pipefail
VERSION="${1:?usage: release-macos.sh vX.Y.Z}"
cargo build --release
( cd ui/macos && ./bundle.sh )
scripts/bundle-macos-libs.sh ui/macos/Classick.app
scripts/sign-macos.sh                       # B3 signing
create-dmg ... Classick.dmg ui/macos/Classick.app
codesign --sign "$ID" Classick.dmg
xcrun notarytool submit Classick.dmg --keychain-profile classick-notary --wait
xcrun stapler staple ui/macos/Classick.app
xcrun stapler staple Classick.dmg
generate_appcast <release-dir>
gh release create "$VERSION" Classick.dmg --title "Classick $VERSION" --notes "…"
# publish appcast.xml to Pages (commit/push docs/ or gh-pages)
echo "released $VERSION"
```
(Fill in the `ID`/paths; keep secrets out of the file — read the Team ID from `security find-identity` at runtime.)

- [ ] **Step 2: Dry-run each stage** on a test tag; fix failures (sign/notary log).

- [ ] **Step 3: Commit**

```bash
git add scripts/release-macos.sh
git commit -m "build(macos): one-command local release pipeline"
```

---

### Task B7: Release gate (manual)

- [ ] **Step 1: Clean-install check**

Download the released `.dmg` (fresh, or on a second account/VM), open it, drag Classick to Applications, launch. Expected: **no Gatekeeper prompt**; `spctl -a -vv` on the installed app = accepted/Notarized.

- [ ] **Step 2: Sparkle update round-trip**

Bump the version, run `release-macos.sh vNEXT`, then in the *installed older* app → "Check for Updates…" → it detects vNEXT and installs it.

- [ ] **Step 3: Docs + commit**

Update `ui/macos/README.md` (release steps, key-in-Keychain note) + BUILD-NOTES. Commit.

**SP3 DONE when a notarized dmg installs Gatekeeper-clean on a toolchain-free Mac and the installed app self-updates via Sparkle.**

---

## Self-Review

**Spec coverage:** Phase A.1 encode → A2; A.2 probe/tags → A1; A.3 gate → A5; cover art (implied by A) → A3. Phase B.1 closure → B1; B.2 sign/notarize → B3/B4; B.3 Sparkle → B2/B5; B.4 dmg/release → B5/B6; B.5 gate → B7. arm64/local/Developer-ID/keys-secret in Global Constraints. ✅

**Placeholder note:** Phase A has complete TDD code for the lofty→ProbeOutput mapping (the DRY-critical, correctness-sensitive part) + the afconvert command; the exact lofty 0.22 `ItemKey`/`Mp4Codec` symbol names are flagged to confirm against docs (API detail, not a design gap). Phase B is shell/tooling verified by running the commands (signing/notarization can't be unit-tested); every step has the concrete command + expected output. Secrets (private EdDSA key, notary password, Team ID) are referenced but never written to the repo, per the Global Constraints.

**Type consistency:** `ProbeOutput`/`ProbeFormat`/`ProbeTags`/`ProbeStream`/`ProbeDisposition` are used exactly as defined in `transcode.rs`; the macOS `probe`/`transcode_to_alac`/`extract_cover_art`/`verify_tools_available` keep their existing signatures (the `ffmpeg_path` arg is accepted-but-ignored on macOS) so `apply_loop` callers are unchanged.
