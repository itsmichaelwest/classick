# macOS Core Enablement (design)

**Goal:** make the Rust core (`classick`) build, run, and sync on macOS, and
finish its daemon backend so a native SwiftUI menu-bar app can be built against
it with **zero further Rust work**. Verified end-to-end against real hardware: a
Mac + a physical iPod Classic.

**Non-goal:** reverse-engineering Apple's iTunesDB signature (the Music.app
"cannot read → do NOT Restore" dialog is expected and documented, not fixed);
Linux device identity (that gap stays as-is); any Swift, any GUI; any
distribution/notarization/`.dmg` packaging.

This is **sub-project 1 of 3**. The other two get their own specs:

1. **macOS core enablement** — this document.
2. **SwiftUI app** — the native menu-bar client (pure Swift against a proven backend).
3. **Packaging & distribution** — the self-contained, signed, notarized
   `Classick.app` → `.dmg` → GitHub release.

Distribution comes last because you cannot bundle an app that does not exist
yet. SP1 deliberately links against the Homebrew dependency closure for
development; SP3 handles the standalone bundle.

---

## Context: what already works on macOS

Investigation of the crate (see `crates/classick/src/`) found the core is
already substantially cross-platform. The macOS gaps are narrow:

| Concern | Status on macOS | File |
|---|---|---|
| IPC transport (Unix socket accept loop) | ✅ works | `daemon/ipc_server.rs:32–48, 174–218` |
| Mount detection (`/Volumes/*` + `iPod_Control` probe) | ✅ works | `ipod/device.rs:1743–1756` |
| Transcode + console-suppression shim | ✅ no-op on Unix | `transcode.rs`, `windows_proc.rs:31–51` |
| Daemon runtime / scheduler / history | ✅ cross-platform | `daemon/` |
| TUI / plain / IPC progress backends | ✅ cross-platform | `progress.rs` |
| libgpod FFI linking (non-Windows pkg-config path) | ⚠️ present but needs a real libgpod | `build.rs:233–293` |
| **Device identity** (FirewireGUID + model) | ❌ Windows-only | `ipod/device.rs:106–150, 450–807` |
| **Music.app guard** | ❌ silent no-op on Unix | `preflight.rs:139–145` |
| **Daemon device watcher** (hotplug) | ❌ Windows-only | `daemon/` |

The single platform seam for identity is `resolve_libgpod_identity()`, which
returns a `LibgpodIdentity` struct consumed by `OwnedDb` / DB signing. Providing
a macOS implementation of that one function leaves everything downstream
untouched.

---

## Milestone M1 — Engine proof

The goal of M1 is to **prove the engine syncs on macOS** via the existing
CLI/TUI — no daemon, no GUI. This is where the project's real risk lives
(libgpod on macOS, device identity, the Music.app trap).

### M1.1 — libgpod dev build (from source)

**There is no `libgpod` Homebrew formula** (verified: `brew info libgpod` →
"No available formula"). This mirrors the Windows situation exactly — no
prebuilt libgpod exists anywhere, so the Windows build vendored a from-source
MinGW build (`vendor/libgpod/BUILD-NOTES.md`). macOS is the same story with a
uniform clang toolchain, which is actually **simpler** than Windows: no
MinGW→MSVC import-lib dance, no `.def` files, no giant vendored runtime closure.
We build with clang and link the `.dylib` directly.

**The exact Windows source and patches are reusable.** The Windows build used
**fadingred/libgpod @ `4a8a33ef4bc58eee1baca6793618365f75a5c3fa` (0.8.0)** with
three patches under `vendor/libgpod/patches/`. All three are toolchain-agnostic
modern-fixes, not Windows-specific:

- `0001-drop-libplist-sqlite3-add-gmodule.patch` — the deliberate "iPod Classic
  uses iTunesDB, not iTunesCDB" scoping decision (drops `itdb_sqlite.c`
  consumers). Reuse verbatim.
- `0002-glib-2.88-gstatbuf-fix.patch` — `struct stat` → `GStatBuf`. Homebrew's
  glib is equally modern, so the same fix is required.
- `0003-pixbuf-codepaths-glib-2.88-fixes.patch` — `GStatBuf` + `g_object_ref`
  casts in the artwork code paths. Same requirement on macOS.

**Deliverable:** `scripts/build-libgpod-macos.sh` that:

1. Verifies Homebrew deps are present:
   `glib gdk-pixbuf libgcrypt libxml2 pkg-config autoconf automake libtool intltool gtk-doc ffmpeg`.
2. Clones fadingred/libgpod at the pinned SHA into a scratch dir.
3. Applies the three patches with `git apply`.
4. Runs `NOCONFIGURE=1 ./autogen.sh`, then `./configure` with the flags from
   BUILD-NOTES (`--disable-static --without-hal --disable-gtk-doc
   --disable-introspection --without-python --disable-more-warnings`),
   installing to a **prefix** (repo-local, e.g. `vendor/libgpod/macos-prefix/`,
   gitignored — or `/usr/local`).
5. `make -j && make install` (from `src/` if the tests directory fails to
   build, per BUILD-NOTES).
6. Prints the `PKG_CONFIG_PATH` line needed so `build.rs` finds
   `libgpod-1.0.pc`.

**`libgcrypt` is mandatory** — it is load-bearing for the iPod Classic 7G
iTunesDB signature (`itdb_hashAB.c` + gmodule dynamic hash). Without it the
DB is rejected on-device. This retires the "will it sign correctly?" risk into a
concrete dependency.

**`build.rs`:** the existing non-Windows pkg-config path
(`build.rs:233–293`) consumes the installed libgpod unchanged. Fix the
misleading `brew install libgpod glib pkg-config` comment — that command fails
on the first package and must not be presented as the macOS setup path.

**Verification of this step:** `configure` summary shows
`Artwork support ..........: yes`; linker line gains `-lgdk_pixbuf-2.0` and
`-lgcrypt`; `pkg-config --exists libgpod-1.0` succeeds.

### M1.2 — Device identity resolution (IOKit)

New `#[cfg(target_os = "macos")]` implementation of `resolve_libgpod_identity()`
in `ipod/device.rs`, returning the same `LibgpodIdentity` struct as the Windows
path so all downstream signing is untouched.

**Layered (on-disk → IOKit recovery), matching the Windows structure:**

- **Layer 1 — on-disk `SysInfo`.** Read `iPod_Control/Device/SysInfo` from the
  mount, parse `FirewireGuid` + `ModelNumStr` via the existing
  `parse_sysinfo_field` helper. Instant when present (older iTunes/prior tool).
- **Layer 2 — IOKit recovery.** Map the mount to its IOKit device via
  **DiskArbitration** (`DADiskCreateFromVolumePath` → `DADiskCopyIOMedia`),
  walk up the registry to the parent Apple USB device (vendor `0x05AC`), and
  read device properties directly:
  - `USB Serial Number` string → **`FirewireGuid`** (for USB iPods these are the
    same 16-hex value), formatted `0x…`;
  - `idProduct` (PID) + the IOMedia **`Size`** property (capacity) → the
    existing `identify_ipod(pid, capacity)` heuristic → `ModelNumStr`.

**Decision (informed reversal, 2026-07-12):** implement via the servo
`core-foundation-rs` crate set (`io-kit-sys`, `core-foundation`,
`core-foundation-sys`) as new macOS-only dependencies. This **supersedes** the
existing `ioreg`/`df` shellout path (`macos_recover_ipod_info`,
`macos_ioreg_iousb`, `macos_bsd_name_for_mount`, `macos_find_ipod_device`,
`macos_dict_contains_bsd` in `device.rs`), which is removed. Rationale: we are
also doing the event-driven IOKit watcher (M2.2), so a single native
IOKit/CoreFoundation layer serves both — and it fixes the ioreg path's
hardcoded-`None` capacity gap natively (via IOMedia `Size`) instead of adding a
`diskutil` shellout. This overrides the device.rs:1180–1182 "IOKit FFI out of
scope for the initial pass" note; the initial pass is over.

### M1.3 — Music.app guard

New `#[cfg(target_os = "macos")]` branch in `preflight.rs`, mirroring the
Windows Toolhelp32 process-scan. Detect a running Music app
(bundle `com.apple.Music`, plus `AMPLibraryAgent`) and **refuse to sync** with
the same warning copy already used on Windows. Today the Unix path is a silent
no-op (`preflight.rs:139–145`); this closes the hole that would otherwise let a
user drop into the Restore trap on macOS.

### M1.4 — `cargo test` green on macOS

Gate genuinely Windows-only tests behind `#[cfg(windows)]`. Add unit tests for
the two new pure functions: the `SysInfo` field parse and the PID→model
heuristic. IOKit itself is validated manually against the real device (not
unit-testable without hardware).

### M1.5 — Verification (the gate)

On the real Mac + real iPod Classic:

1. Plug in the iPod → mounts at `/Volumes/<name>`.
2. Run a real sync via TUI:
   `cargo run --release -- --source <flac-dir> [--ipod /Volumes/<name>]`.
3. Confirm: tracks **play on-device**; **artwork renders** (thumbnail `.ithmb`
   blobs written under `iPod_Control/Artwork/`); a second run is an
   **incremental no-op**; the **Music.app guard fires** when Music is open.
4. Confirm the "cannot read, please Restore" dialog behaves as documented —
   do **not** click Restore.

**M1 is done when a sync completes and plays on your iPod.**

---

## Milestone M2 — Daemon backend

The goal of M2 is a **complete, proven macOS backend** the SwiftUI app can talk
to — so SP2 is pure Swift.

### M2.1 — Daemon runs on macOS

The daemon runtime and Unix-socket accept loop are already cross-platform. This
is primarily "start `classick --daemon`, confirm it binds the socket and accepts
connections," plus wiring the pieces below.

### M2.2 — Event-driven IOKit device watcher

The daemon already has a cross-platform `PollingDeviceWatcher` (production impl
of the `DeviceWatcher` trait in `daemon/device_watcher.rs`) that works on macOS
via `scan_for_ipod()`. Per the informed decision (2026-07-12), macOS instead
gets a **new event-driven `IokitDeviceWatcher`** — a second `impl DeviceWatcher`
— which the daemon runtime selects on macOS; `PollingDeviceWatcher` stays as the
Windows production watcher and the test double. The trait's own doc comment
already anticipated this swap ("M5 polish can swap in an event-driven impl
without touching the runtime").

`IokitDeviceWatcher::start()` spawns a dedicated `std::thread` running a
`CFRunLoop`, and bridges events into the trait's `mpsc::Receiver<DeviceEvent>`:

- Register `IOServiceAddMatchingNotification` for `kIOMatchedNotification` +
  `kIOTerminatedNotification` on the Apple iPod (vendor `0x05AC`); the C
  callback receives the `mpsc::Sender` via its `refcon`.
- On **match** (attach): a USB attach fires **before** the volume mounts, but a
  `DeviceEvent::Connected(DetectedIpod)` needs the mount path (`drive`). So wait
  for the mount — either a DiskArbitration mount callback or a bounded
  poll of `/Volumes` — then run `resolve_libgpod_identity()` (M1.2) to fill
  `serial`/`model_label`, and send `Connected`.
- On **terminate** (detach): send `Disconnected { serial }` immediately.
- Reuse the existing `Debouncer` (500ms) unchanged.
- Watcher shutdown: signal the CFRunLoop to stop (`CFRunLoopStop`) and join the
  thread when the receiver is dropped, so the daemon's bounded-shutdown path
  leaks nothing.

### M2.3 — Path reconciliation (the IPC contract)

Two paths must be pinned to macOS idioms and made consistent across Rust, docs,
and the future Swift client:

- **Socket:** canonical **`$TMPDIR/classick.sock`**, resolved via
  `confstr(_CS_DARWIN_USER_TEMP_DIR)` (the Apple-sanctioned per-user runtime
  directory), **not** the raw env var. Rationale: a daemon socket is
  runtime/ephemeral state, not user data; the Darwin user temp dir is stable
  per-UID across reboots, auto-reaped, and both Rust (`confstr`) and Swift
  (`NSTemporaryDirectory()`) resolve to the same value; the ~60-char path is
  safely under the 104-byte `sun_path` limit. The Rust `default_pipe_name()`
  already falls to `$TMPDIR/classick.sock` when `$XDG_RUNTIME_DIR` is unset (it
  is, on macOS); **harden it to call `confstr` directly** and update the docs +
  the C# reference (which incorrectly expects `~/.classick/daemon.sock`). This
  assumes the app is **not App-Store-sandboxed** — which it cannot be (raw
  device access, spawns a daemon) — so a container-private `$TMPDIR` is not a
  concern.
- **Config:** **`~/Library/Application Support/classick/config.toml`** (this is
  user data, so Application Support is correct). Already resolves correctly:
  `config_file.rs::default_path()` uses `dirs::config_dir()`, which returns
  `~/Library/Application Support` on macOS. Work here is **verify + fix the
  misleading `%APPDATA%` comment** — no code change to the path logic.

### M2.4 — Verification

A throwaway `examples/` socket probe that mimics the Swift client:

1. Connect to `$TMPDIR/classick.sock`.
2. `hello` handshake — validate `protocol_version`.
3. `subscribe_device_events`; plug/unplug the iPod → confirm
   `device_connected` / `device_disconnected` arrive with correct identity.
4. `trigger_sync` → confirm the daemon spawns the sync subprocess
   (`classick --ipc-mode`) and forwards progress events end-to-end.

**M2 is done when the socket probe sees live hotplug events and drives a sync.**

---

## Risks & mitigations

1. **macOS pixbuf/glib code paths need incremental fixes beyond patch `0003`.**
   The Windows build discovered pixbuf-path fixes iteratively. Mitigate by
   building libgpod **first**, before any Rust work, so compiler errors surface
   immediately. Any new fixes become additional patch files under
   `vendor/libgpod/patches/`.
2. **IOKit USB-serial ≠ `FirewireGuid` on some Classic revisions** → wrong
   signature → DB rejected on-device. Mitigate: the on-disk `SysInfo` layer
   wins when present; before trusting the IOKit-derived value, validate it
   against a known-good `SysInfo` on the actual test unit.
3. **`configure` needs an extra macOS-specific flag** (Linux-only HAL/`sgutils`
   assumptions). `--without-hal` is already set; surface anything else at first
   build and record it in BUILD-NOTES.
4. **gdk-pixbuf loaders undiscoverable at runtime** → silent artwork failure.
   For the *dev* build the Homebrew loaders + cache are found automatically;
   the relocatable-loaders problem is an SP3 (packaging) concern, flagged there.
5. **IOKit/CoreFoundation FFI + CFRunLoop lifecycle** (M1.2, M2.2) is the new
   `unsafe` surface introduced by the full-IOKit decision. Risks: the C
   notification callback must carry the tokio `Sender` safely via `refcon`
   (no use-after-free); the CFRunLoop thread must stop cleanly on receiver drop
   (`CFRunLoopStop` + join) so daemon shutdown leaks nothing; DiskArbitration
   mount→device mapping can race a not-yet-mounted volume (mitigated by the
   attach→wait-for-mount step). Mitigate by keeping the FFI in one focused
   module, unit-testing the pure parts (property parsing, guid formatting), and
   validating the unsafe paths against the real device.

---

## Definition of done

- `scripts/build-libgpod-macos.sh` produces a working libgpod; `build.rs` links
  it via pkg-config; `cargo build --release` succeeds on macOS.
- `cargo test` passes on macOS.
- A real FLAC→ALAC sync to the physical iPod completes via TUI, plays on-device
  with artwork, and re-syncs as a no-op.
- The Music.app guard refuses to sync while Music is running.
- `classick --daemon` runs on macOS; the socket probe observes live
  `device_connected`/`device_disconnected` and drives a `trigger_sync` to
  completion.
- Socket path (`$TMPDIR/classick.sock` via `confstr`) and config path
  (`~/Library/Application Support/classick/config.toml`) are reconciled in Rust
  and in `docs/ipc-protocol.md`.
- `vendor/libgpod/BUILD-NOTES.md` gains a macOS section documenting the build.

---

## Follow-on specs (not this document)

- **SP2 — SwiftUI app:** native `NSStatusItem` menu-bar client + popover +
  review dialog + 4-tab settings + 5-step wizard + prompt overlay +
  notifications + daemon spawning, over the v1.1.0 Unix-socket protocol. Owns
  UI only; the daemon (this spec) owns config, device polling, scheduling, sync
  orchestration.
- **SP3 — Packaging & distribution:** self-contained `Classick.app` bundling
  `classick` + the full libgpod dylib closure (`install_name_tool`/`@rpath`/
  `@loader_path`) + relocatable pixbuf loaders + an LGPL ffmpeg build; Developer
  ID signing + hardened runtime + notarize + staple + `.dmg` → GitHub release.
