# Multi-device Sync Stabilization Execution Index

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement these plans task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the approved multi-device, portable-state, artwork-safe, lifecycle, playlist-delivery, and macOS UI stabilization as eight independently reviewable plans.

**Architecture:** Device identity is serial-keyed end to end. `DeviceRegistry` owns remembered identity, `SessionAdmission` owns the current capacity-one sync policy, `ManifestStore` owns portable device truth, and `CheckpointCoordinator` publishes DB/art/manifest/playlist ownership coherently. Apple and Rockbox playlist representations derive from one verified membership, while the Swift app consumes authoritative snapshots and acknowledged additive mutations into `[DeviceSerial: DeviceViewState]`.

**Tech Stack:** Rust stable, Tokio, serde, libgpod, UTF-8 M3U8, Swift 6 strict concurrency, SwiftUI/Core Transferable/AppKit, macOS 15 deployment floor, NetFS on macOS.

## Global Constraints

- Preserve the dirty worktree. Stage only files named by the task being committed; never revert or overwrite unrelated playlist/macOS work.
- Keep `\\jupiter\data\media\music` read-only. Device writes are allowed only after Plan 3's coordinated transaction exists.
- Keep old Windows/config/wire shapes readable during the compatibility window. New macOS commands always carry serial and request ID where defined.
- Do not raise the deployment target above macOS 15. Gate newer visual APIs with availability checks.
- Add regression coverage before each bug fix and observe the focused RED failure before implementing.
- Keep new files below roughly 500 lines and split the existing oversized runtime, wire, and view files along the boundaries named in these plans.
- Before every listed commit, run `git add` with each exact path in that task's **Files** section (never `git add .` or `git add -A`), inspect `git diff --cached`, then run the listed `git commit`.
- Run Rust and Swift test processes sequentially when they may touch shared daemon/socket state.

## Execution Order

- [ ] [Plan 1 — Device registry and keyed state](2026-07-18-device-registry-state.md)
- [ ] [Plan 2 — Portable manifest and source recovery](2026-07-18-portable-manifest-source-recovery.md), after Plan 1
- [ ] [Plan 3 — Artwork-safe sync and cancellation](2026-07-18-artwork-safe-sync-cancellation.md), after Plans 1–2
- [ ] [Plan 4 — Daemon lifecycle and ordered IPC](2026-07-18-daemon-lifecycle-ordered-ipc.md), after Plan 1; integrate after Plan 3 for shutdown/finalization tests
- [ ] [Plan 5 — macOS state and UI stabilization](2026-07-18-macos-state-ui-stabilization.md), after Plans 1–4
- [ ] [Plan 6A — iPod playlist integrity](2026-07-18-ipod-playlist-integrity.md), after Plans 1–3
- [ ] [Plan 6B — Rockbox playlist projection](2026-07-18-rockbox-playlist-projection.md), after Plan 6A
- [ ] [Plan 6C — Native library drag-and-drop](2026-07-18-native-library-drag-drop.md), after Plans 4–6B

## Final Gate

- [ ] Run `cargo test -p classick -- --test-threads=1`.
- [ ] Run `cd ui/macos && swift test`.
- [ ] Run `xcodebuild -project ui/macos/Classick.xcodeproj -scheme Classick -configuration Debug -destination 'platform=macOS' MACOSX_DEPLOYMENT_TARGET=15.0 CODE_SIGNING_ALLOWED=NO build`.
- [ ] Run `ui/macos/bundle.sh`.
- [ ] Run `dotnet test ui/windows/Classick.UI.Tests/Classick.UI.Tests.csproj` on a host with .NET 10, and require Windows CI to compile/test the Rust Windows cfg plus WinUI before merge; record this as an external gate when executing on macOS.
- [ ] Before any live write, record the mounted serial/free space, copy or hash `iTunesDB`, `ArtworkDB`, and every `.ithmb`, and run the new read-only audit expecting the known six-album failures.
- [ ] On macOS 27, verify discovery of two iPods when two physical devices are available, serial-targeted commands, SMB remount behavior, quit-during-sync, menu-bar sizing, and every DeviceRow phase. If only one physical iPod is available, record that limitation and rely on the platform-neutral A+B integration gate.
- [ ] When a macOS 15 VM is available, verify the availability-gated material fallback; the mandatory local gate is the deployment-target-15 build, and the unavailable VM must be recorded rather than falsely claimed.
- [ ] On the mounted iPod, run a completed sync through the coordinated transaction, then a cancellation during an album, then rerun the read-only audit expecting the baseline failures to disappear; compare database/artwork hashes, eject/boot/playback, and confirm the six reported albums retain art.
- [ ] Capture the exact playlist audit, run one coordinated Classick write, and inspect before booting firmware. Eject and boot Apple firmware once, remount and inspect before running Classick; repeat a second Apple-firmware boot without another Classick write. Record whether the exact `ipod-classic-video-kind-v1` record is created on every boot or only after a libgpod-authored write, then run Classick normalization and require at most one exact canonical record while every near-match/foreign playlist remains unchanged.
- [ ] Sync one manual and one smart Classick playlist, verify membership order and playback in Apple firmware, boot Rockbox and verify the corresponding `/Playlists/Classick/*.m3u8` order/playback, then exercise rename and unsubscribe and prove foreign Apple and Rockbox playlists remain untouched.
- [ ] On macOS 27, drag one Library artist/album/genre onto an explicit iPod and a manual playlist. Verify native copy cursor/preview, one-target accent highlight, sidebar scrolling, invalid-target snap-back, VoiceOver destination text, immediate-sync and next-sync settings, disconnected/busy deferral, authoritative completion feedback, and no stale-editor overwrite. If interaction permission is unavailable in this session, complete automated target/payload/reducer coverage and record the user-assisted interaction gate as outstanding rather than claiming it passed.
