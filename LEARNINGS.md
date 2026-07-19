# Classick learnings

Keep this file concise and current. Record only non-obvious constraints that
would prevent a future regression or materially shorten debugging. Historical
incidents and completed gate reports are archived in
`docs/archive/LEARNINGS-history.md`.

## Device and data safety

- On-device verification shows that a Classick-managed database is not
  intrinsically unreadable to iTunes/Music. Do not repeat the earlier
  "libgpod iPods are always rejected" claim. The running-process preflight is
  only a conservative concurrent-access guard while Classick mutates device
  state, not a workaround for a permanent format incompatibility.
- libgpod's `itdb_start_sync` is a no-op on regular mounted iPods; only its
  iPhone-family backend performs the Apple `lockdownd`/AFC sync handshake and
  locks `/com.apple.itunes.lock_sync`. Mounted devices therefore need a
  Classick lease plus external-generation fencing, while future iPod touch
  support needs a distinct mobile-device backend.
- Treat database, artwork, playlists, ownership, and manifests as one
  coordinated publication. Rollback must restore the exact recorded
  bytes-or-absence for every authority before becoming terminal.
- Recover pending journals before planning a new diff. Require exact schema,
  mount, raw serial, session identity, hashes, and owned paths; ambiguous or
  escaping journals stay untouched and block mutation.
- A checkpoint that reopens a candidate DB must restore FirewireGuid and
  ModelNumStr and resolve playlists against post-staging DBIDs.
- libgpod may drop loaded artwork thumbnails when rewriting a parsed DB. A
  writing path must preserve the artwork snapshot or re-thumbnail every track
  and force the fresh artwork-build path.
- Unlink a track from every playlist before freeing it. Reconcile filesystem
  orphans and dangling DB references together.
- Apple playlists are owned only by recorded libgpod ID and structural kind;
  Rockbox projections are owned only by recorded filename and content hash.
  Same-name, empty-smart, firmware, podcast, On-The-Go, and modified files are
  foreign unless exact authority proves otherwise.
- New Rockbox publication is no-replace. Deletion quarantine names must be
  derived from durable authority, and the containing directory must be synced
  after unlink before advancing recovery.
- Finalization assumes Classick is the sole cooperative writer. Held directory
  handles and Windows handle-bound operations narrow races, but macOS/Linux do
  not provide an inode-CAS pathname unlink primitive.
- The source library is read-only. Derived audio, indexes, manifests, journals,
  and playlists belong in Classick state/staging directories or on the iPod.

## Sync execution

- Keep both ffmpeg `-nostdin` and `stdin(Stdio::null())`; an inherited daemon
  command pipe otherwise wedges ffmpeg during finalization.
- Windows child processes that could create a console must use
  `windows_proc::NoConsoleWindow`.
- Cancellation, pause, UI shutdown, OS signals, and parent death converge on
  one drain. Send one stop command, retain admission through publication, and
  consume the terminal event plus EOF. Use a progress-reset inactivity
  watchdog, not a fixed total-duration cap.
- Album boundaries are the cancellation and fit-retry unit. Do not admit the
  next album after stop is observed.
- Temporary transcode and artwork paths must be unique per call; workers can
  overlap and retry.
- Worker panics and subprocess crashes must become explicit failed outcomes;
  they must not strand the daemon in syncing/finalizing state.
- Source recovery owns deferred intent until reachability is terminal. A
  UI-authorized retry may escalate authentication UI; background attempts may
  not infer that permission.

## Daemon and IPC

- There are two independently versioned newline-delimited JSON protocols:
  subprocess `1.4.0` and daemon `2.0.0`. `hello` is always first.
- Preserve socket-line order through one actor/reader path. Per-line detached
  tasks can let later events overtake an invalid handshake.
- Every durable mutation keeps its request ID until canonical persisted state
  acknowledges that exact request. An echo, write completion, or uncorrelated
  broadcast is not an acknowledgement.
- Target device commands by raw serial and route sync progress by serial plus
  session ID. Connection generation is also required for off-thread metadata
  reads so stale results cannot attach to a reconnected device.
- `devices/registry.json` is the live configured-device authority;
  `config.toml`'s identity is migration input only.
- Retain truthful terminal attempts in memory when history persistence fails;
  an idle snapshot must not hide a real failed or cancelled attempt.
- Unknown daemon state strings decode to idle on clients so an additive server
  state cannot discard an entire status update.
- Adding a command/event requires updating exhaustive Rust, Swift, and C#
  matches together. Wire compatibility is major-version based; minor additions
  must remain ignorable/defaultable.

## Cross-platform behavior

- Windows bindgen needs a real `libclang.dll` plus both GLib include roots.
  Runtime builds also need the complete vendored libgpod/GLib DLL closure copied
  beside `classick.exe`; derive the target profile directory from `OUT_DIR`.
- libgpod's GLib symbols such as `g_error_free` require the GLib import library,
  not only `gpod.lib`.
- A WinUI tray-only app stays alive only when `TaskbarIcon` is an application
  XAML resource. Use the library's second-window context-menu mode.
- macOS uses `afconvert`, not ffmpeg. Convert through 16-bit PCM; `afconvert`
  rejects some higher-bit-depth FLAC inputs when asked for ALAC directly.
- macOS tests use committed FLAC fixtures and must not synthesize them with
  ffmpeg.
- Adding or removing a Swift source requires `xcodegen generate`; SwiftPM tests
  auto-discover files but the committed Xcode project does not.
- The macOS app is not sandboxed: sandboxing prevents the daemon/device/socket
  access the architecture requires.
- Sparkle appcast enclosures must point at GitHub Release assets. Commit and
  push version changes before creating the tag/release, then publish the
  appcast.

## Tests and fixtures

- Daemon integration tests need isolated config, history, state, and pipe/socket
  paths. They must never read the developer's live config or collide with a
  running daemon.
- Cross-platform daemon tests belong in their own integration files;
  `daemon_runtime_integration.rs` is Windows-gated.
- Fake iPod mounts need `iPod_Control/Music/F00`; libgpod uses an existing
  `F##` directory and will not create the first one.
- PID-only temporary directories collide under parallel tests. Include a
  process-local atomic sequence or another unique component.
- XcodeGen determinism is checked by regenerating and requiring no project-file
  diff after committed source changes.
