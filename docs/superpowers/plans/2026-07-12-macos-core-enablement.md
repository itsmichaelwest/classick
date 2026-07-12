# macOS Core Enablement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `classick` Rust core build, run, and sync on macOS, and complete its daemon backend (event-driven IOKit hotplug + reconciled socket path) so a native SwiftUI app can be built against a fully-proven backend with zero further Rust work.

**Architecture:** Reuse the existing cross-platform core; fill three macOS gaps with a single native IOKit/CoreFoundation FFI layer. Device identity and hotplug both route through IOKit (superseding a placeholder `ioreg` shellout); the Music.app guard uses a `pgrep`-style process scan; the daemon socket path is hardened to the Darwin per-user temp dir. libgpod is built from source (fadingred fork + 3 existing patches) and linked via pkg-config.

**Tech Stack:** Rust (MSVC-parity on macOS via clang), libgpod 0.8.0 (fadingred fork), Homebrew (glib/gdk-pixbuf/libgcrypt/libxml2/pkg-config/ffmpeg), `core-foundation` + `core-foundation-sys` + `io-kit-sys` crates (servo `core-foundation-rs` set), tokio, `plist` (already a dep).

## Global Constraints

- **libgpod source:** fadingred/libgpod @ `4a8a33ef4bc58eee1baca6793618365f75a5c3fa` (0.8.0). Apply `vendor/libgpod/patches/0001..0003` verbatim.
- **`libgcrypt` is mandatory** in the libgpod build — it signs the iPod Classic 7G iTunesDB (`hashAB`). Without it the DB is rejected on-device.
- **All new native FFI is macOS-only:** `#[cfg(target_os = "macos")]`. Never break the Windows or Linux compile.
- **No `println!` outside `examples/`.** Use `tracing::{info,warn,error,debug}`. In IPC/daemon mode stdout is the wire.
- **Long-running subprocess spawns go through `windows_proc::NoConsoleWindow`** (`.no_console()`) — no-op on macOS, kept for parity.
- **`anyhow::Result` + `.context(...)`** at boundaries. `unsafe` allowed only in the FFI layer (`macos_iokit.rs`); everything above stays safe.
- **Conventional Commits**, scopes: `build`, `device`, `preflight`, `daemon`, `ipc`, `docs`. Never `git add -A` unless nothing is staged; stage files by name.
- **Socket path is the IPC contract** — Rust `default_pipe_name()` and `docs/ipc-protocol.md` must agree exactly.
- **`IpodIdentity`** is `{ model_num: &'static str, label: &'static str }`. **`LibgpodIdentity`** is `{ firewire_guid: String, model_num_str: String }`. **`UsbIpodInfo`** is `{ firewire_guid: String, pid: Option<u16>, capacity_bytes: Option<u64>, disk_number: Option<u32>, identity: Option<IpodIdentity>, sysinfo_extended_xml: Option<String>, sysinfo_extended_parsed: Option<ParsedSysInfo> }`. **`DetectedIpod`** is `{ serial: String, model_label: String, drive: String, name: Option<String>, volume_guid: Option<String> }`.

---

## File Structure

- `scripts/build-libgpod-macos.sh` **(create)** — reproducible from-source libgpod build → prefix.
- `crates/classick/build.rs` **(modify)** — fix the false `brew install libgpod` comment (`:217-226`).
- `crates/classick/Cargo.toml` **(modify)** — add `[target.'cfg(target_os = "macos")'.dependencies]`.
- `crates/classick/src/ipod/macos_iokit.rs` **(create)** — the entire IOKit/CoreFoundation/DiskArbitration FFI layer: mount→USB-device resolution, property reads, match/terminate notifications. Only file with `unsafe`.
- `crates/classick/src/ipod/mod.rs` **(modify)** — register `macos_iokit` module (macOS-only).
- `crates/classick/src/ipod/device.rs` **(modify)** — reimplement `macos_recover_ipod_info` via `macos_iokit` (`:1189-1322`, remove ioreg helpers); layer the non-Windows `resolve_libgpod_identity` (`:155-167`).
- `crates/classick/src/preflight.rs` **(modify)** — macOS `verify_itunes_not_running` + `detect_apple_processes` (`:139-145`).
- `crates/classick/src/daemon/iokit_watcher.rs` **(create)** — `IokitDeviceWatcher` (event-driven `impl DeviceWatcher`).
- `crates/classick/src/daemon/mod.rs` **(modify)** — declare/export `iokit_watcher` (macOS-only).
- `crates/classick/src/daemon/runtime.rs` **(modify)** — cfg-select the watcher at `:61`.
- `crates/classick/src/daemon/ipc_server.rs` **(modify)** — harden `default_pipe_name` with `confstr` on macOS (`:32-48`).
- `crates/classick/src/config_file.rs` **(modify)** — fix the `%APPDATA%` comment (`:104`).
- `crates/classick/examples/daemon-probe.rs` **(create)** — Unix-socket probe mimicking the Swift client.
- `docs/ipc-protocol.md` **(modify)** — reconcile the macOS socket path.
- `vendor/libgpod/BUILD-NOTES.md` **(modify)** — add the macOS build section.

---

## MILESTONE M1 — Engine proof

### Task 1: libgpod from-source build + macOS compile

**Files:**
- Create: `scripts/build-libgpod-macos.sh`
- Modify: `crates/classick/build.rs:217-226`

**Interfaces:**
- Produces: an installed `libgpod-1.0.pc` on `PKG_CONFIG_PATH` so `build.rs::build_pkg_config` (`:233-293`) resolves libgpod + glib.

- [ ] **Step 1: Write the build script**

Create `scripts/build-libgpod-macos.sh`:

```bash
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
NOCONFIGURE=1 ./autogen.sh
./configure --prefix="$PREFIX" --disable-static --without-hal \
            --disable-gtk-doc --disable-introspection \
            --without-python --disable-more-warnings

echo "==> make + install"
make -j"$(sysctl -n hw.ncpu)"
make install

PC="$PREFIX/lib/pkgconfig/libgpod-1.0.pc"
[ -f "$PC" ] || { echo "ERROR: $PC not produced"; exit 1; }
grep -q "Artwork support" config.log 2>/dev/null || true
echo "==> Done. Add to your shell / cargo env:"
echo "    export PKG_CONFIG_PATH=\"$PREFIX/lib/pkgconfig:\$PKG_CONFIG_PATH\""
```

- [ ] **Step 2: Make it executable and run it**

Run: `chmod +x scripts/build-libgpod-macos.sh && ./scripts/build-libgpod-macos.sh`
Expected: ends with "Done." and a `libgpod-1.0.pc` under the prefix. If a patch fails to apply or `configure` reports `Artwork support ..........: no`, STOP and fix (missing gdk-pixbuf) before continuing.

- [ ] **Step 3: Verify pkg-config resolves libgpod**

Run:
```bash
export PKG_CONFIG_PATH="$PWD/crates/classick/vendor/libgpod/macos-prefix/lib/pkgconfig:$PKG_CONFIG_PATH"
pkg-config --exists libgpod-1.0 && pkg-config --modversion libgpod-1.0
```
Expected: prints `0.8.0` (or similar), exit 0.

- [ ] **Step 4: Fix the misleading build.rs comment**

In `crates/classick/build.rs`, replace the `macOS: brew install libgpod glib pkg-config` line in the comment block (`:217-226`) with:

```rust
//   macOS:         no libgpod formula exists — run scripts/build-libgpod-macos.sh
//                  then export PKG_CONFIG_PATH to its prefix (see that script
//                  and vendor/libgpod/BUILD-NOTES.md).
```

- [ ] **Step 5: Verify the crate compiles on macOS**

Run: `cargo build --release`
Expected: SUCCESS (links libgpod + glib via pkg-config). If bindgen fails on a glib header, confirm `glib-2.0` is resolvable via `pkg-config --cflags glib-2.0`.

- [ ] **Step 6: Commit**

```bash
git add scripts/build-libgpod-macos.sh crates/classick/build.rs
git commit -m "build(device): from-source libgpod build script for macOS + fix pkg-config comment"
```

---

### Task 2: IOKit FFI layer — mount → device identity

**Files:**
- Modify: `crates/classick/Cargo.toml` (add macOS deps)
- Create: `crates/classick/src/ipod/macos_iokit.rs`
- Modify: `crates/classick/src/ipod/mod.rs`

**Interfaces:**
- Consumes: `IpodIdentity`, `identify_ipod(pid: u16, capacity_bytes: Option<u64>) -> Option<IpodIdentity>` from `device.rs`.
- Produces:
  - `pub struct IokitUsbIdentity { pub firewire_guid: String, pub pid: Option<u16>, pub capacity_bytes: Option<u64> }`
  - `pub fn identity_for_mount(mount: &std::path::Path) -> Option<IokitUsbIdentity>` — DiskArbitration mount→IOMedia→USB parent; reads `USB Serial Number`, `idProduct`, IOMedia `Size`.
  - `pub fn format_firewire_guid(usb_serial: &str) -> String` — `0x` + uppercase (pure, unit-tested).

- [ ] **Step 1: Add the macOS dependency table**

In `crates/classick/Cargo.toml`, after the `[target.'cfg(windows)'.dependencies]` block, add:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
# Native IOKit/CoreFoundation/DiskArbitration FFI for device identity + hotplug.
# servo/core-foundation-rs set — supersedes the prior ioreg/df shellouts.
core-foundation = "0.10"
core-foundation-sys = "0.8"
io-kit-sys = "0.4"
libc = "0.2"
```

- [ ] **Step 2: Write the failing test for guid formatting**

Create `crates/classick/src/ipod/macos_iokit.rs` with just:

```rust
//! Native macOS device identity + hotplug via IOKit / CoreFoundation /
//! DiskArbitration. The only module in the crate with `unsafe` above the
//! libgpod FFI layer. Supersedes the earlier `ioreg`/`df` shellout path.

/// Format a USB iSerialNumber string as libgpod's `FirewireGuid`
/// (`0x` prefix, uppercase hex). For USB iPods the USB serial number
/// string is the FireWire GUID.
pub fn format_firewire_guid(usb_serial: &str) -> String {
    format!("0x{}", usb_serial.trim().to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_serial_as_uppercase_hex_guid() {
        assert_eq!(format_firewire_guid("000a27002138b0a8"), "0x000A27002138B0A8");
    }

    #[test]
    fn trims_and_uppercases() {
        assert_eq!(format_firewire_guid("  ab12cd  "), "0xAB12CD");
    }
}
```

Register the module — in `crates/classick/src/ipod/mod.rs` add:

```rust
#[cfg(target_os = "macos")]
pub mod macos_iokit;
```

- [ ] **Step 3: Run the test to verify it passes (pure fn already correct)**

Run: `cargo test -p classick macos_iokit`
Expected: 2 passed. (This proves the module is wired and the pure helper is right; the FFI is added next and verified against hardware.)

- [ ] **Step 4: Implement the FFI resolution (interface + algorithm)**

Add to `macos_iokit.rs` the struct and the `identity_for_mount` function. This is `unsafe` FFI verified against hardware, not unit tests. Implement exactly this algorithm using the named symbols:

```rust
use std::path::Path;

#[derive(Debug, Clone)]
pub struct IokitUsbIdentity {
    pub firewire_guid: String,
    pub pid: Option<u16>,
    pub capacity_bytes: Option<u64>,
}

/// Resolve a mounted iPod volume to its USB identity via DiskArbitration +
/// IOKit. Returns None if the mount isn't a USB Apple device.
pub fn identity_for_mount(mount: &Path) -> Option<IokitUsbIdentity> {
    // ALGORITHM (all calls behind `unsafe`; wrap CF objects so they're
    // CFRelease'd on drop — core_foundation::base::TCFType handles this):
    //
    // 1. DASessionCreate(kCFAllocatorDefault).
    // 2. CFURLCreateWithFileSystemPath for `mount` (kCFURLPOSIXPathStyle,
    //    is_directory = true); DADiskCreateFromVolumePath(session, url).
    // 3. DADiskCopyIOMedia(disk) -> io_service_t for the IOMedia object.
    //    (io-kit-sys exposes DADiskCopyIOMedia via the DiskArbitration
    //    framework; link it — see Step 5.)
    // 4. Read the IOMedia "Size" property:
    //    IORegistryEntryCreateCFProperty(media, CFSTR("Size"), ...) -> CFNumber
    //    -> u64 capacity_bytes.
    // 5. Walk parents until an entry has idVendor == 0x05AC:
    //    IORegistryEntryGetParentEntry(entry, kIOServicePlane, &parent) in a
    //    loop; at each node read "idVendor" (CFNumber). Stop at the USB device.
    // 6. On that USB device read:
    //      "USB Serial Number" (CFString) -> format_firewire_guid(...)
    //      "idProduct"        (CFNumber) -> pid: u16
    // 7. IOObjectRelease every io_object_t; CFRelease handled by wrappers.
    //
    // Return None (not panic) on any missing property or non-Apple device.
    todo!("implement per algorithm above")
}
```

- [ ] **Step 5: Link the frameworks in build.rs (macOS)**

In `crates/classick/build.rs`, at the top of the macOS/non-Windows path (inside `build_pkg_config` or the platform dispatch that calls it), emit the framework link directives:

```rust
#[cfg(target_os = "macos")]
{
    println!("cargo:rustc-link-lib=framework=IOKit");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=DiskArbitration");
}
```
(Place this so it runs on macOS builds — `build.rs`'s `main` already branches on target; add a macOS arm if absent.)

- [ ] **Step 6: Build to confirm it links**

Run: `cargo build --release`
Expected: SUCCESS. (`todo!()` compiles; it will only panic if called. Linkage of the three frameworks is what we're proving here.)

- [ ] **Step 7: Implement the `todo!()` body, then hardware-verify**

Replace the `todo!()` with the implementation. Then, with the iPod plugged in and mounted, add a temporary `dbg!` call site (or use the example from Task 10 early) and run to confirm `identity_for_mount(Path::new("/Volumes/<name>"))` returns a plausible `firewire_guid` (16 hex digits), `pid` (e.g. `0x1261` for Classic), and non-zero `capacity_bytes`. Remove any temporary debug code.

Run: `cargo test -p classick macos_iokit` (pure tests still green)
Expected: 2 passed; and the manual check prints a real identity.

- [ ] **Step 8: Commit**

```bash
git add crates/classick/Cargo.toml crates/classick/src/ipod/macos_iokit.rs crates/classick/src/ipod/mod.rs crates/classick/build.rs
git commit -m "device: native IOKit device identity (mount -> FirewireGuid + PID + capacity)"
```

---

### Task 3: Route macOS USB recovery through IOKit; remove ioreg

**Files:**
- Modify: `crates/classick/src/ipod/device.rs:1189-1322` (replace `macos_recover_ipod_info` body; delete `macos_ioreg_iousb`, `macos_bsd_name_for_mount`, `macos_find_ipod_device`, `macos_dict_contains_bsd`)

**Interfaces:**
- Consumes: `macos_iokit::identity_for_mount` (Task 2), `identify_ipod` (device.rs).
- Produces: `macos_recover_ipod_info(mount: &Path) -> Option<UsbIpodInfo>` (unchanged signature; called by the dispatch at `device.rs:601-603`, which stays as-is).

- [ ] **Step 1: Replace the `macos_recover_ipod_info` implementation**

Replace the body (`device.rs:1189-1233`) with:

```rust
#[cfg(target_os = "macos")]
fn macos_recover_ipod_info(mount: &std::path::Path) -> Option<UsbIpodInfo> {
    let ident = crate::ipod::macos_iokit::identity_for_mount(mount)?;
    let identity = ident.pid.and_then(|p| identify_ipod(p, ident.capacity_bytes));
    Some(UsbIpodInfo {
        firewire_guid: ident.firewire_guid,
        pid: ident.pid,
        capacity_bytes: ident.capacity_bytes,
        disk_number: None,
        identity,
        sysinfo_extended_xml: None,
        sysinfo_extended_parsed: None,
    })
}
```

- [ ] **Step 2: Delete the now-dead ioreg helpers**

Remove `macos_ioreg_iousb`, `macos_bsd_name_for_mount`, `macos_find_ipod_device`, `macos_dict_contains_bsd` (`device.rs:1235-1322`) and the "ioreg/df" TODO comment block above them (`:1180-1187`). If `plist` is now unused on macOS, leave the crate dep (it's used by `sysinfo_extended` on Windows and is harmless).

- [ ] **Step 3: Build for macOS**

Run: `cargo build --release`
Expected: SUCCESS, no unused-function warnings for the removed helpers.

- [ ] **Step 4: Hardware check — detection now yields correct model**

With the iPod mounted, run the daemon detection path indirectly via a quick check: `cargo run --release -- --source /tmp/empty --ipod "/Volumes/<name>" --dry-run` (or the closest existing dry-run flag). Confirm the logged identity shows a real `ModelNumStr` (e.g. `MC293`, not `xPID_1261`) — proving capacity disambiguation works.
Expected: log line `iPod identity: FirewireGuid=0x..., ModelNumStr=<real model>`.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/ipod/device.rs
git commit -m "device: route macOS USB recovery through IOKit, drop ioreg shellout"
```

---

### Task 4: Layer the non-Windows `resolve_libgpod_identity`

**Files:**
- Modify: `crates/classick/src/ipod/device.rs:155-167`

**Interfaces:**
- Consumes: `parse_sysinfo_field` (`:1536`), `recover_ipod_info_from_usb` (`:591`), `LibgpodIdentity`.
- Produces: `resolve_libgpod_identity(&Path) -> Result<LibgpodIdentity>` (non-Windows), now layered on-disk → USB recovery, matching Windows (`:106-150`).

- [ ] **Step 1: Replace the non-Windows stub**

Replace `device.rs:152-167` with:

```rust
/// Non-Windows identity resolution. Layer 1: on-disk SysInfo (older
/// firmware / prior tool). Layer 2: USB recovery via
/// `recover_ipod_info_from_usb` (IOKit on macOS; SysInfo-only elsewhere).
/// Mirrors the Windows structure so apply-time signing works without a
/// populated SysInfo file.
#[cfg(not(windows))]
pub fn resolve_libgpod_identity(ipod_mount: &Path) -> Result<LibgpodIdentity> {
    let sysinfo_path = crate::ipod::layout::sysinfo_path(ipod_mount);
    let sysinfo_text = std::fs::read_to_string(&sysinfo_path).unwrap_or_default();
    let disk_guid = parse_sysinfo_field(&sysinfo_text, "FirewireGuid").filter(|s| !s.is_empty());
    let disk_model = parse_sysinfo_field(&sysinfo_text, "ModelNumStr").filter(|s| !s.is_empty());

    if let (Some(guid), Some(model_num_str)) = (disk_guid.clone(), disk_model.clone()) {
        return Ok(LibgpodIdentity { firewire_guid: guid, model_num_str });
    }

    let recovered = recover_ipod_info_from_usb(ipod_mount)
        .ok_or_else(|| anyhow!("USB recovery failed for {}", ipod_mount.display()))?;
    let firewire_guid = disk_guid.unwrap_or_else(|| recovered.firewire_guid.clone());
    let model_num_str = recovered
        .identity
        .map(|id| id.model_num.to_string())
        .or(disk_model)
        .ok_or_else(|| anyhow!(
            "could not determine ModelNumStr for iPod at {} (PID {:?}, capacity {:?} bytes)",
            ipod_mount.display(), recovered.pid, recovered.capacity_bytes,
        ))?;
    Ok(LibgpodIdentity { firewire_guid, model_num_str })
}
```

- [ ] **Step 2: Write a unit test for the on-disk (Layer 1) path**

This path is pure filesystem — testable without hardware. Add to `device.rs`'s `#[cfg(test)] mod tests`:

```rust
#[cfg(not(windows))]
#[test]
fn resolve_identity_reads_on_disk_sysinfo() {
    let dir = std::env::temp_dir().join(format!("classick-sysinfo-{}", std::process::id()));
    let device_dir = dir.join("iPod_Control").join("Device");
    std::fs::create_dir_all(&device_dir).unwrap();
    std::fs::write(
        device_dir.join("SysInfo"),
        "FirewireGuid: 0x000A27002138B0A8\nModelNumStr: MC293\n",
    ).unwrap();
    let id = resolve_libgpod_identity(&dir).unwrap();
    assert_eq!(id.firewire_guid, "0x000A27002138B0A8");
    assert_eq!(id.model_num_str, "MC293");
    std::fs::remove_dir_all(&dir).ok();
}
```
(Confirm `layout::sysinfo_path` maps to `iPod_Control/Device/SysInfo`; adjust the fixture path if the layout differs.)

- [ ] **Step 3: Run the test**

Run: `cargo test -p classick resolve_identity_reads_on_disk_sysinfo`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/classick/src/ipod/device.rs
git commit -m "device: layer non-Windows libgpod identity (on-disk SysInfo -> USB recovery)"
```

---

### Task 5: macOS Music.app guard

**Files:**
- Modify: `crates/classick/src/preflight.rs:139-145`

**Interfaces:**
- Consumes: `await_prompt(progress, decision_rx, msg, &[&str], &[PromptOutcome])`, `PromptOutcome::{Retry, Abort}`, `Progress`, `Receiver<Decision>`.
- Produces: `verify_itunes_not_running(&Progress, &Receiver<Decision>) -> Result<()>` (macOS), mirroring the Windows retry/abort loop.

- [ ] **Step 1: Write a failing test for the process-name classifier**

Add a pure classifier so it's unit-testable without spawning processes. In `preflight.rs`:

```rust
#[cfg(target_os = "macos")]
fn is_blocking_music_process(name: &str) -> bool {
    // Music.app is the modern iTunes; classic "iTunes" still exists on
    // older macOS. AMPLibraryAgent is advisory (non-blocking) — handled
    // by the caller, not here.
    let n = name.trim();
    n.eq_ignore_ascii_case("Music") || n.eq_ignore_ascii_case("iTunes")
}

#[cfg(all(test, target_os = "macos"))]
mod macos_guard_tests {
    use super::*;
    #[test]
    fn classifies_music_and_itunes_as_blocking() {
        assert!(is_blocking_music_process("Music"));
        assert!(is_blocking_music_process("iTunes"));
        assert!(!is_blocking_music_process("Finder"));
        assert!(!is_blocking_music_process("AMPLibraryAgent"));
    }
}
```

- [ ] **Step 2: Run it to verify it fails to compile (function not yet used by guard) then passes**

Run: `cargo test -p classick --target-dir target classifies_music_and_itunes_as_blocking` (on macOS)
Expected: PASS (pure fn). If dead-code-warns, that's fine until Step 3 wires it.

- [ ] **Step 3: Implement the macOS guard**

Replace the non-Windows stub (`:139-145`) with a macOS impl (keep a separate no-op for other Unix):

```rust
/// macOS: refuse to sync while Music.app (or legacy iTunes) is running —
/// both want exclusive iPod access, and Music.app's "cannot read, please
/// Restore" dialog is the trap we keep users out of.
#[cfg(target_os = "macos")]
pub fn verify_itunes_not_running(
    progress: &Progress,
    decision_rx: &Receiver<Decision>,
) -> Result<()> {
    loop {
        let running = macos_blocking_music_processes();
        if running.is_empty() {
            return Ok(());
        }
        let names = running.join(", ");
        let msg = format!(
            "Cannot sync while Music.app is running.\n\n\
             Detected: {names}.\n\n\
             Music and Classick both want exclusive access to the iPod.\n\
             Quit Music (do NOT click Restore if it asks — your iPod is fine).\n\n\
             Choose:"
        );
        let outcome = await_prompt(
            progress,
            decision_rx,
            msg,
            &["Retry (after quitting Music)", "Abort"],
            &[PromptOutcome::Retry, PromptOutcome::Abort],
        )?;
        match outcome {
            PromptOutcome::Retry => continue,
            _ => return Err(anyhow!("Music.app is running; aborted")),
        }
    }
}

/// Enumerate running blocking processes via `pgrep -x` (ships with macOS;
/// consistent with the crate's other non-Windows shellouts).
#[cfg(target_os = "macos")]
fn macos_blocking_music_processes() -> Vec<String> {
    use crate::windows_proc::NoConsoleWindow;
    let mut found = Vec::new();
    for proc_name in ["Music", "iTunes"] {
        let ok = std::process::Command::new("pgrep")
            .arg("-x")
            .arg(proc_name)
            .no_console()
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false);
        if ok && is_blocking_music_process(proc_name) {
            found.push(proc_name.to_string());
        }
    }
    found
}

/// Other Unix (Linux): no iTunes/Music, no-op.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn verify_itunes_not_running(
    _progress: &Progress,
    _decision_rx: &Receiver<Decision>,
) -> Result<()> {
    Ok(())
}
```

- [ ] **Step 4: Build + run the unit test**

Run: `cargo build --release && cargo test -p classick classifies_music_and_itunes_as_blocking`
Expected: build SUCCESS, test PASS.

- [ ] **Step 5: Hardware/manual check**

Open Music.app, start a sync via TUI, confirm the guard prompts "Cannot sync while Music.app is running"; quit Music, choose Retry, confirm it proceeds.

- [ ] **Step 6: Commit**

```bash
git add crates/classick/src/preflight.rs
git commit -m "preflight: macOS Music.app guard (pgrep-based), mirrors Windows retry/abort loop"
```

---

### Task 6: `cargo test` green on macOS

**Files:**
- Modify: any test that assumes Windows (gate with `#[cfg(windows)]`).

**Interfaces:** none new.

- [ ] **Step 1: Run the whole suite and collect failures**

Run: `cargo test`
Expected: identify any compile errors or failures that are Windows-specific (e.g. tests constructing `DetectedIpod` with `drive: "G:\\"` are fine as data; tests calling Windows-only fns are not).

- [ ] **Step 2: Gate genuinely Windows-only tests**

For each failing test that exercises a `#[cfg(windows)]` function, add `#[cfg(windows)]` to the test. Do NOT gate tests that are merely using Windows-shaped strings but call cross-platform code — those must keep running on macOS.

- [ ] **Step 3: Re-run the suite**

Run: `cargo test`
Expected: ALL PASS on macOS.

- [ ] **Step 4: Commit**

```bash
git add crates/classick/src crates/classick/tests
git commit -m "test: gate Windows-only tests so the suite is green on macOS"
```

---

### Task 7: M1 gate — verified end-to-end sync (manual)

**Files:** none (verification only).

- [ ] **Step 1: Prepare a small FLAC set and plug in the iPod**

Confirm it mounts at `/Volumes/<name>` and Music.app is quit.

- [ ] **Step 2: Run a real sync via TUI**

Run: `PKG_CONFIG_PATH=... cargo run --release -- --source <flac-dir>`
Expected: device auto-detected; identity resolved; tracks transcode (FLAC→ALAC) and copy; run completes.

- [ ] **Step 3: Verify on-device**

Eject, unplug, and on the iPod itself: confirm the tracks appear and **play**, and album art renders. Re-plug and check `iPod_Control/Artwork/` contains `F*_*.ithmb` blobs.

- [ ] **Step 4: Verify incremental no-op**

Run the same sync again. Expected: 0 add / 0 modify — a no-op.

- [ ] **Step 5: Record the result**

Add a bullet to `LEARNINGS.md` capturing any macOS-specific surprise (e.g. a pixbuf loader path issue, a configure flag). Commit:

```bash
git add LEARNINGS.md
git commit -m "docs: record macOS M1 end-to-end sync verification"
```

**M1 DONE when a sync completes and plays on the iPod.**

---

## MILESTONE M2 — Daemon backend

### Task 8: Harden the daemon socket path (confstr)

**Files:**
- Modify: `crates/classick/src/daemon/ipc_server.rs:32-48`
- Modify: `docs/ipc-protocol.md`

**Interfaces:**
- Produces: `default_pipe_name() -> String` returning `<darwin-user-temp>/classick.sock` on macOS via `confstr(_CS_DARWIN_USER_TEMP_DIR)`, falling back to the existing `$TMPDIR`/`/tmp` chain.

- [ ] **Step 1: Add the confstr resolver (macOS)**

In `ipc_server.rs`, add a macOS helper and call it first in `default_pipe_name`'s Unix branch:

```rust
/// macOS: the Apple-sanctioned per-user runtime dir (`$TMPDIR` points here,
/// but confstr is robust against an unset/overridden env var). Stable
/// per-UID across reboots; ~60-char path stays under the 104-byte sun_path
/// limit. This is the IPC contract the SwiftUI client must match via
/// NSTemporaryDirectory().
#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<std::path::PathBuf> {
    use std::os::raw::c_char;
    const CS_DARWIN_USER_TEMP_DIR: libc::c_int = 65537; // _CS_DARWIN_USER_TEMP_DIR
    let need = unsafe { libc::confstr(CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
    if need == 0 { return None; }
    let mut buf = vec![0 as c_char; need];
    let got = unsafe { libc::confstr(CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr(), need) };
    if got == 0 || got > need { return None; }
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(std::path::PathBuf::from(cstr.to_string_lossy().into_owned()))
}
```

Add `libc = "0.2"` to the macOS dep table if Task 2 didn't (it did). In `default_pipe_name`, on macOS prefer `darwin_user_temp_dir()` before the `$XDG_RUNTIME_DIR`/`$TMPDIR`/`/tmp` chain, joining `classick.sock` (`PROJECT_DIR`-based name, matching the current fallback).

- [ ] **Step 2: Write a test that the path is absolute + ends correctly (macOS)**

```rust
#[cfg(target_os = "macos")]
#[test]
fn default_pipe_name_is_absolute_sock_under_temp() {
    let p = default_pipe_name();
    assert!(p.starts_with('/'), "must be absolute: {p}");
    assert!(p.ends_with(".sock"), "must be a .sock: {p}");
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p classick default_pipe_name_is_absolute_sock_under_temp`
Expected: PASS, and manually confirm it prints a `/var/folders/.../T/classick.sock`-style path.

- [ ] **Step 4: Update the protocol doc**

In `docs/ipc-protocol.md`, change the macOS socket reference to `$TMPDIR/classick.sock` (Darwin user temp dir via `confstr(_CS_DARWIN_USER_TEMP_DIR)`; Swift resolves the same via `NSTemporaryDirectory()`). Note the app is **not** App-Store-sandboxed (so `$TMPDIR` is shared between daemon and client). Remove any `~/.classick/daemon.sock` reference.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/daemon/ipc_server.rs docs/ipc-protocol.md
git commit -m "daemon(ipc): pin macOS socket to Darwin user temp dir via confstr; reconcile docs"
```

---

### Task 9: Event-driven IOKit device watcher

**Files:**
- Create: `crates/classick/src/daemon/iokit_watcher.rs`
- Modify: `crates/classick/src/daemon/mod.rs` (declare module)
- Modify: `crates/classick/src/daemon/runtime.rs:12,61` (cfg-select)
- Modify: `crates/classick/src/ipod/macos_iokit.rs` (add notification primitives)

**Interfaces:**
- Consumes: `DeviceWatcher` trait, `DeviceEvent::{Connected,Disconnected}`, `DetectedIpod`, `Debouncer`, `scan_for_ipod()` (for the mount→DetectedIpod fill after attach), `macos_iokit` notification primitives.
- Produces: `pub struct IokitDeviceWatcher; impl DeviceWatcher for IokitDeviceWatcher` with `new_production()`.

- [ ] **Step 1: Add IOKit notification primitives to `macos_iokit.rs`**

Add (FFI, hardware-verified) a function that runs a CFRunLoop and invokes Rust callbacks on USB attach/terminate for Apple vendor `0x05AC`:

```rust
/// Run a CFRunLoop that calls `on_change` whenever an Apple (0x05AC) USB
/// device is added or removed. Blocks until `CFRunLoopStop` is called on
/// this run loop (obtained via CFRunLoopGetCurrent inside). Intended to run
/// on a dedicated std::thread.
///
/// ALGORITHM:
/// 1. IONotificationPortCreate(kIOMainPortDefault); get its CFRunLoopSource,
///    add to CFRunLoopGetCurrent() under kCFRunLoopDefaultMode.
/// 2. IOServiceMatching(kIOUSBDeviceClassName); set "idVendor" = 0x05AC in the
///    matching dict.
/// 3. IOServiceAddMatchingNotification(port, kIOMatchedNotification, dict,
///    added_cb, refcon) and (kIOTerminatedNotification, ..., removed_cb, ...).
///    Drain both iterators once to arm them.
/// 4. `refcon` carries a raw pointer to a Box<dyn FnMut(Change)> (leaked for
///    the run loop's lifetime); the C callbacks reconstruct &mut and invoke it.
/// 5. CFRunLoopRun(). Returns when stopped.
pub fn run_usb_notifications(on_change: Box<dyn FnMut(UsbChange) + Send>) -> RunLoopHandle {
    todo!("implement per algorithm above")
}

pub enum UsbChange { Added, Removed }

/// Handle to stop the run loop from another thread (CFRunLoopStop + the
/// CFRunLoopRef captured at start). `stop()` unblocks `run_usb_notifications`.
pub struct RunLoopHandle { /* CFRunLoopRef (wrapped Send) */ }
impl RunLoopHandle { pub fn stop(&self) { todo!() } }
```

- [ ] **Step 2: Implement the watcher over the primitives**

Create `crates/classick/src/daemon/iokit_watcher.rs`:

```rust
//! Event-driven macOS device watcher. Implements `DeviceWatcher` via IOKit
//! USB match/terminate notifications on a dedicated CFRunLoop thread, bridged
//! into the trait's mpsc channel. Replaces `PollingDeviceWatcher` on macOS.

use crate::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
use crate::ipod::device;
use tokio::sync::mpsc;

pub struct IokitDeviceWatcher;

impl IokitDeviceWatcher {
    pub fn new_production() -> Self { Self }
}

impl DeviceWatcher for IokitDeviceWatcher {
    fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> {
        let (tx, rx) = mpsc::channel::<DeviceEvent>(crate::daemon::DEVICE_EVENT_CHANNEL_CAPACITY);
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let mut last: Option<crate::ipod::device::DetectedIpod> = None;
            let handle = crate::ipod::macos_iokit::run_usb_notifications(Box::new(move |change| {
                use crate::ipod::macos_iokit::UsbChange;
                match change {
                    UsbChange::Added => {
                        // Attach fires before the volume mounts; scan_for_ipod
                        // requires the mount. Bounded wait for it to appear.
                        for _ in 0..50 {
                            if let Some(d) = device::scan_for_ipod() {
                                if tx2.blocking_send(DeviceEvent::Connected(d.clone())).is_ok() {
                                    last = Some(d);
                                }
                                return;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                    UsbChange::Removed => {
                        if let Some(prev) = last.take() {
                            let _ = tx2.blocking_send(DeviceEvent::Disconnected { serial: prev.serial });
                        }
                    }
                }
            }));
            // When `rx` is dropped, blocking_send errors; stop the run loop.
            let _ = handle; // handle.stop() is called from the runtime on shutdown (see Step 4)
        });
        rx
    }
}
```

(Note: if the runtime needs an explicit stop on shutdown, expose the `RunLoopHandle` through the watcher; otherwise the thread exits when a `blocking_send` fails after `rx` drop. Wire whichever the runtime's shutdown path — `runtime.rs:290-316` — expects; the polling watcher relies on channel-drop, so match that.)

- [ ] **Step 3: Declare the module (macOS-only)**

In `crates/classick/src/daemon/mod.rs` add:

```rust
#[cfg(target_os = "macos")]
pub mod iokit_watcher;
```

- [ ] **Step 4: cfg-select the watcher in the runtime**

In `crates/classick/src/daemon/runtime.rs:61`, replace:

```rust
        watcher: Box::new(PollingDeviceWatcher::new_production()),
```

with:

```rust
        #[cfg(target_os = "macos")]
        watcher: Box::new(crate::daemon::iokit_watcher::IokitDeviceWatcher::new_production()),
        #[cfg(not(target_os = "macos"))]
        watcher: Box::new(PollingDeviceWatcher::new_production()),
```

- [ ] **Step 5: Build**

Run: `cargo build --release`
Expected: SUCCESS on macOS. (`PollingDeviceWatcher` remains referenced on non-macOS; ensure the `use` at `runtime.rs:12` isn't dead on macOS — gate it if the compiler warns.)

- [ ] **Step 6: Implement the two `todo!()`s in `macos_iokit.rs`, then hardware-verify**

Implement `run_usb_notifications` + `RunLoopHandle::stop`. Then verify with the probe in Task 10.

- [ ] **Step 7: Commit**

```bash
git add crates/classick/src/daemon/iokit_watcher.rs crates/classick/src/daemon/mod.rs crates/classick/src/daemon/runtime.rs crates/classick/src/ipod/macos_iokit.rs
git commit -m "daemon(device): event-driven IOKit hotplug watcher on macOS"
```

---

### Task 10: Daemon socket probe example

**Files:**
- Create: `crates/classick/examples/daemon-probe.rs`

**Interfaces:**
- Consumes: `classick::daemon::ipc_server::default_pipe_name`, the daemon JSON wire (from `docs/ipc-protocol.md`).

- [ ] **Step 1: Write the probe**

Create `crates/classick/examples/daemon-probe.rs`:

```rust
//! Connect to the running daemon's Unix socket, do the hello handshake,
//! subscribe to device events, and (optionally) trigger a sync. Mimics what
//! the SwiftUI client will do. macOS/Unix only.
//!
//! Usage:
//!   cargo run --example daemon-probe            # watch device events
//!   cargo run --example daemon-probe -- sync    # also send trigger_sync
//!
//! Exit codes: 0 ok, 1 connect failed, 2 handshake failed.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

fn main() {
    let path = classick::daemon::ipc_server::default_pipe_name();
    eprintln!("connecting to {path}");
    let stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => { eprintln!("connect failed: {e}"); std::process::exit(1); }
    };
    let mut writer = stream.try_clone().expect("clone");
    let mut reader = BufReader::new(stream);

    // Read hello.
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        eprintln!("no hello"); std::process::exit(2);
    }
    println!("<= {}", line.trim());

    // Subscribe to device events.
    writeln!(writer, r#"{{"type":"subscribe_device_events"}}"#).unwrap();
    writer.flush().unwrap();

    if std::env::args().nth(1).as_deref() == Some("sync") {
        writeln!(writer, r#"{{"type":"trigger_sync","source":"manual"}}"#).unwrap();
        writer.flush().unwrap();
    }

    // Stream events (plug/unplug the iPod to see device_connected/disconnected).
    for l in reader.lines() {
        match l { Ok(l) => println!("<= {l}"), Err(_) => break }
    }
}
```

(Confirm the exact command JSON shapes against `docs/ipc-protocol.md` — `subscribe_device_events` / `trigger_sync` field names — and adjust the literals to match `ipc_daemon.rs`.)

- [ ] **Step 2: Build the example**

Run: `cargo build --example daemon-probe`
Expected: SUCCESS.

- [ ] **Step 3: Commit**

```bash
git add crates/classick/examples/daemon-probe.rs
git commit -m "daemon: add Unix-socket probe example mimicking the SwiftUI client"
```

---

### Task 11: M2 gate — daemon hotplug + sync (manual)

**Files:** none (verification only).

- [ ] **Step 1: Start the daemon**

Run: `PKG_CONFIG_PATH=... cargo run --release -- --daemon`
Expected: binds `$TMPDIR/classick.sock`, logs "listening".

- [ ] **Step 2: Run the probe and exercise hotplug**

In another shell: `cargo run --example daemon-probe`
Plug the iPod in → expect a `device_connected` event with real serial + model. Unplug → expect `device_disconnected`. Confirm the CFRunLoop watcher fires promptly (event-driven, not on a 1.5s tick).

- [ ] **Step 3: Trigger a sync through the daemon**

Run: `cargo run --example daemon-probe -- sync`
Expected: daemon spawns `classick --ipc-mode`, forwards progress events (`SyncEvent` lines), completes. Verify tracks land on the iPod.

- [ ] **Step 4: Verify clean shutdown**

Ctrl-C the daemon; confirm no orphaned `classick` process and the socket file is gone/reusable on next start.

- [ ] **Step 5: Record the result**

```bash
git add LEARNINGS.md
git commit -m "docs: record macOS M2 daemon hotplug + sync verification"
```

**M2 DONE when the probe sees live hotplug events and drives a sync to completion.**

---

### Task 12: Docs — BUILD-NOTES macOS section + config comment

**Files:**
- Modify: `crates/classick/vendor/libgpod/BUILD-NOTES.md`
- Modify: `crates/classick/src/config_file.rs:104`

**Interfaces:** none.

- [ ] **Step 1: Add the macOS build section to BUILD-NOTES**

Append a `## macOS build (2026-07-12)` section documenting: no Homebrew formula; `scripts/build-libgpod-macos.sh`; the Homebrew deps (incl. `libgcrypt`); the prefix + `PKG_CONFIG_PATH`; that the 3 Windows patches apply unchanged; any extra macOS-specific patch discovered during Task 1/7; and that macOS links the `.dylib` directly (no import-lib dance).

- [ ] **Step 2: Fix the config path comment**

In `config_file.rs:104`, change the `%APPDATA%\classick\config.toml` comment to note the path is cross-platform via `dirs::config_dir()` (`~/Library/Application Support/classick/config.toml` on macOS, `%APPDATA%\classick\config.toml` on Windows).

- [ ] **Step 3: Commit**

```bash
git add crates/classick/vendor/libgpod/BUILD-NOTES.md crates/classick/src/config_file.rs
git commit -m "docs: macOS libgpod BUILD-NOTES section + cross-platform config path comment"
```

---

## Self-Review

**Spec coverage:**
- M1.1 libgpod build → Task 1 ✅
- M1.2 IOKit identity → Tasks 2, 3, 4 ✅
- M1.3 Music guard → Task 5 ✅
- M1.4 cargo test green → Task 6 ✅
- M1.5 verified sync → Task 7 ✅
- M2.1 daemon runs → exercised in Task 11 (no code change needed; runtime is cross-platform) ✅
- M2.2 IOKit watcher → Task 9 ✅
- M2.3 socket path → Task 8; config path → Task 12 ✅
- M2.4 socket probe → Tasks 10, 11 ✅
- Risk 5 (FFI/CFRunLoop lifecycle) → addressed in Tasks 2, 9 (Send wrappers, stop-on-drop) ✅

**Type consistency:** `LibgpodIdentity`, `UsbIpodInfo`, `IpodIdentity`, `DetectedIpod`, `DeviceEvent`, `PromptOutcome` used consistently per the Global Constraints block. `macos_recover_ipod_info` keeps its `(&Path) -> Option<UsbIpodInfo>` signature so the dispatch at device.rs:601 is untouched.

**Placeholder note:** The IOKit FFI bodies (Task 2 Step 4/7, Task 9 Step 1/6) are specified as **exact algorithms with named IOKit/DiskArbitration/CoreFoundation symbols + a `todo!()` skeleton**, verified against real hardware rather than unit tests — this is deliberate for exploratory `unsafe` FFI, not a vague placeholder. Every pure/testable unit (guid formatting, identity layering, process classifier, socket path) has complete TDD code.
