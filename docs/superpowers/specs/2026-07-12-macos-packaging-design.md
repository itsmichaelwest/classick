# macOS Native Transcode + Packaging (design)

**Goal:** ship Classick as a self-contained, Developer-ID-signed, **notarized
`Classick.dmg`** that any Apple-Silicon Mac opens with no Gatekeeper friction and
**auto-updates via Sparkle** — published to GitHub Releases by one local command.
Along the way, drop the ffmpeg dependency on macOS in favour of the native
`afconvert` + a Rust tag reader, so nothing extra has to be bundled.

**This is sub-project 3 of 3** (SP1 = engine, SP2 = SwiftUI app, both merged to
`main`). Two phases:

- **Phase A — native macOS transcode** (a small `#[cfg(target_os = "macos")]`
  engine change): `afconvert` for FLAC→ALAC + the `lofty` crate for probing/tags,
  replacing the ffmpeg/ffprobe shell-outs. Windows is untouched.
- **Phase B — packaging & distribution**: bundle the libgpod dylib closure,
  Developer-ID-sign + harden + notarize, integrate Sparkle, build the `.dmg`,
  and publish via a local release script.

**Prerequisites (confirmed):** paid Apple Developer Program membership + a
**Developer ID Application** certificate in the login Keychain.

**Scope decisions (confirmed):**
- **arm64-only** (Apple Silicon). Universal/Intel is a clean follow-up.
- **Local release script**, not CI. (Sparkle works fully locally — the app is
  Apple-signed locally, updates are EdDSA-signed locally.)
- **afconvert + lofty**, no bundled ffmpeg.

**Non-goals:** Windows changes (its ffmpeg/refalac path stays); universal
binaries; the History browser / rich-panel / dry-run review (still v1.1); Mac
App Store distribution (the app is intentionally non-sandboxed).

---

## Phase A — Native macOS transcode

The engine currently shells out to ffmpeg (encode) and ffprobe (probe: codec,
duration, tags) — see `crates/classick/src/transcode.rs` + `tags.rs`. On macOS
we replace both with platform-native paths, keeping Windows on ffmpeg.

### A.1 — Encode via `afconvert`
- FLAC→ALAC: `afconvert -f m4af -d alac <src> <dst.m4a>` — verified on macOS 26
  (reads FLAC natively, emits valid ALAC; it's Apple's own Core Audio ALAC
  encoder, i.e. iTunes-identical output → maximally iPod-compatible).
- Introduce a macOS transcode backend behind the existing encoder abstraction
  (`EncoderChoice`/`transcode_to_alac`). On macOS the default encoder is
  `afconvert` (a system binary at `/usr/bin/afconvert` — no config path needed);
  the `--ffmpeg`/`ffmpeg_path` config becomes irrelevant on macOS (document it).
- Passthrough (MP3/AAC/ALAC copy) is unchanged — it's a byte copy / container
  handling that never needed ffmpeg for the copy itself.

### A.2 — Probe/tags via `lofty`
- Add the **`lofty`** crate (well-maintained Rust audio-metadata reader) as a
  macOS-target dependency. Replace the `ffprobe` JSON parse with lofty for:
  source codec (drives passthrough-vs-transcode), duration, and tags
  (title/artist/album/albumartist/track/disc/genre/date/compilation) that feed
  the manifest + libgpod track metadata.
- Keep the change macOS-scoped (`#[cfg(target_os = "macos")]`) so the tested
  Windows ffprobe path is untouched; the probe result type (`ProbeOutput`) is
  the seam both backends produce.

### A.3 — Verification (hardware gate)
Sync a FLAC album to the real iPod using the afconvert+lofty path and confirm it
**plays on-device** with correct metadata (artist/album/title) and artwork, and
that an incremental re-sync is a no-op. This is the Phase A milestone; it should
be solid since afconvert is Apple's encoder.

---

## Phase B — Packaging & distribution

### B.1 — Dylib closure → relocatable bundle
`otool -L` shows `classick` + `libgpod.4.dylib` reference an absolute vendored
path + `/opt/homebrew` dylibs (glib, gobject, gmodule, gdk-pixbuf, gettext,
libxml2, + transitive pcre2/ffi/png/jpeg/tiff/webp/zlib…).

A `scripts/bundle-macos-libs.sh` (invoked from the release script / an Xcode
build phase):
1. Walk the dylib closure recursively from `classick` (`otool -L`), collecting
   every non-`/usr/lib`, non-`/System` dylib.
2. Copy each into `Classick.app/Contents/Frameworks/`.
3. Rewrite install-names to `@rpath/...` and each dylib's own id via
   `install_name_tool`; add `@loader_path/../Frameworks` as an rpath on the
   `classick` binary (in `Contents/Resources`).
4. Stage the **gdk-pixbuf loaders** into the bundle and set `GDK_PIXBUF_MODULE_DIR`
   (relocatable) at daemon startup — the `loaders.cache` absolute-path problem
   from `vendor/libgpod/BUILD-NOTES.md` applies on macOS too.
5. Verify: `otool -L` on the bundled binary/dylibs shows only `@rpath`/system
   paths (no `/opt/homebrew`, no repo paths), and the app runs with Homebrew
   uninstalled from PATH.

### B.2 — Sign + harden + notarize
- Sign **inside-out** with **Developer ID Application**: every bundled dylib,
  the `classick` daemon, Sparkle's `Autoupdate`/`Updater.app`/XPC services, then
  the app bundle. Enable **hardened runtime**. Library validation passes (all
  one Team ID). Minimal entitlements (the daemon spawns a signed child; Sparkle
  needs its documented entitlements).
- **Notarize**: `xcrun notarytool submit Classick.dmg --keychain-profile … --wait`,
  then `xcrun stapler staple` the app **and** the dmg. Verify `spctl -a -vv`.

### B.3 — Sparkle
- Add Sparkle 2 via **SPM** to the app target in `project.yml` (`packages:` +
  `dependencies: - package: Sparkle`); `xcodegen generate`.
- **"Check for Updates…"** menu row in `MenuContent` → `SPUStandardUpdaterController`
  (call `NSApp.activate(ignoringOtherApps:)` first — LSUIElement apps need it).
  Optional automatic background checks.
- **Keys**: run Sparkle's `generate_keys` once → **public** key into Info.plist
  (`SUPublicEDKey`), **private** key stays in the Keychain (never committed).
- **Appcast**: `generate_appcast` signs each release archive (EdDSA) and writes
  `appcast.xml`; host it on **GitHub Pages** with a stable URL set as
  `SUFeedURL`. Release binaries live on GitHub Releases.

### B.4 — DMG + release script
- `create-dmg` → a styled `.dmg` (drag-to-Applications) from the signed,
  notarized, stapled `.app`.
- **`scripts/release-macos.sh`** chains it: read version from a git tag →
  `cargo build --release` (daemon) → `xcodebuild` (app) → `bundle-macos-libs.sh`
  → sign → make dmg → notarize + staple → `generate_appcast` → `gh release
  create <tag>` with the dmg + push the updated `appcast.xml` to Pages.

### B.5 — Verification (release gate)
On a clean path (fresh download, ideally a second user/VM): the `.dmg` opens,
drag-installs, launches with **no Gatekeeper prompt**, `spctl -a` passes. Then
bump the version, re-run the release script, and confirm the **installed** app
detects + installs the update via Sparkle's "Check for Updates…".

---

## Risks & mitigations

1. **afconvert ALAC rejected by the iPod / metadata gaps.** Low (Apple's own
   encoder), but the Phase A hardware gate catches it. If passthrough-remux for
   AAC needs container work, handle it in A.1; FLAC→ALAC is the primary path.
2. **Dylib closure misses a transitive lib** → crash on a clean machine. Mitigate
   with the recursive `otool -L` walk + the "run with Homebrew off PATH" check
   in B.1, and test on a Mac without the build toolchain.
3. **Signing/notarization rejections** (unsigned nested binary, missing hardened
   runtime, entitlement mismatch). Mitigate: sign inside-out, `codesign
   --verify --deep --strict` before submitting, read `notarytool log` on reject.
4. **Sparkle under hardened runtime** needs its XPC/Autoupdate components signed
   with the right options and specific entitlements. Follow Sparkle's
   sandboxing/signing docs (we're non-sandboxed = the simpler path).
5. **Relocatable gdk-pixbuf loaders** — a stale absolute `loaders.cache` breaks
   artwork on other machines. Use `GDK_PIXBUF_MODULE_DIR` pointing at the
   bundled loaders dir; verify artwork on a clean machine.
6. **`lofty` tag coverage** differs subtly from ffprobe (field names, multivalue
   tags). Mitigate: unit-test the lofty→`ProbeOutput` mapping against a real FLAC
   fixture and compare a few tracks' tags to the ffprobe output.

---

## Definition of done

- Phase A: macOS syncs via afconvert+lofty (no ffmpeg/ffprobe invoked on macOS);
  a real album plays on the iPod with correct metadata + artwork; `cargo test`
  green on macOS; Windows path unchanged.
- Phase B: `scripts/release-macos.sh` produces a signed, notarized, stapled
  `Classick.dmg`; it installs + launches Gatekeeper-clean on a machine without
  the build toolchain; `spctl -a` passes.
- Sparkle: a version bump + release is detected and installed by the previously
  installed app via "Check for Updates…".
- `ui/macos/README.md` + `vendor/libgpod/BUILD-NOTES.md` document the release
  process; the Sparkle **private** key is documented as Keychain-only (never in
  the repo).

---

## Follow-on (not this spec)

- Universal (arm64 + x86_64) build for Intel Macs.
- CI (GitHub Actions) wrapping the local release script once secrets are set up.
- v1.1 app features (History browser, dry-run review, rich `.window` panel).
