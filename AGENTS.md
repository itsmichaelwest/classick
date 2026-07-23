# AGENTS.md — Classick

Orientation for agent-driven work in this repo. Read this first, then dive into
the specific area you're touching.

## What this is

Cross-platform (Windows + macOS) sync tool that copies a FLAC library to an
iPod Classic, transcoding to ALAC on the fly. Three parts:

1. **Rust core (`crates/classick/`)** — one self-contained `classick` binary
   (`classick.exe` on Windows). Wraps libgpod via FFI; spawns the transcoder
   (ffmpeg/refalac on Windows, the system `afconvert` on macOS); writes the
   iTunesDB. Runs in three modes: `--ipc-mode` (subprocess driven by a GUI),
   `--daemon` (long-lived tray/menu-bar companion), or interactive TUI (default
   when stdout is a TTY). Device detection is implemented for Windows and macOS;
   `#[cfg(windows)]` still gates the optional SCSI/SysInfoExtended inquiry
   implementation, but ordinary mount detection and USB-derived libgpod
   identity resolution are cross-platform. SCSI is not a product requirement.
2. **WinUI 3 tray app (`ui/windows/`)** — .NET 10 desktop app. Lives in the
   system tray, owns the daemon process, surfaces device state + sync progress
   + settings + first-run wizard.
3. **SwiftUI menu-bar app (`ui/macos/`)** — macOS 15+ app, the Mac counterpart
   to the tray app. Same daemon + IPC; owns the daemon, shows device state,
   manual/auto sync, settings (incl. the Rockbox-compatibility toggle). Ships
   notarized + self-updating (Sparkle); released via `scripts/release-macos.sh`.

Each UI owns the daemon and talks to it over the same newline-delimited JSON
IPC: a named pipe (`\\.\pipe\classick`) on Windows, a Unix socket (under
`$TMPDIR`) on macOS, plus stdin/stdout JSON for per-sync events. The wire
format is in `docs/ipc-protocol.md` — that document is the source of truth; all
implementations must agree with it.

For the current architecture and safety invariants, see `docs/architecture.md`
and `docs/device-safety.md`. For battle-scars and hard-won gotchas, see `LEARNINGS.md` —
**always read this before touching anything iTunes-DB-adjacent**.

## Top-level layout

```
classick/                  Cargo workspace root
├── Cargo.toml              Workspace manifest (members + shared package fields)
├── Cargo.lock              Workspace lockfile
├── crates/
│   └── classick/          The Rust crate (lib + bin)
│       ├── Cargo.toml      Package manifest
│       ├── build.rs        Bindgen for libgpod headers + DLL copy
│       ├── src/            CLI, orchestrator, apply loop, IPC, daemon,
│       │                   libgpod FFI, transcode, manifest, …
│       ├── tests/          Cargo integration tests + fixtures
│       ├── examples/       Standalone Rust spike binaries
│       └── vendor/         Vendored libgpod build artefacts (DLLs, headers)
├── ui/
│   ├── windows/            WinUI 3 / .NET 10 tray app (see its own README).
│   └── macos/              SwiftUI menu-bar app, macOS 15+ (see its own README).
├── docs/
│   ├── README.md            Current documentation index
│   ├── architecture.md      Components, data authorities, and flow
│   ├── ipc-protocol.md      Wire-format entry point and source of truth
│   ├── device-safety.md     Device/source mutation invariants
│   └── archive/             Historical design and investigation records
├── scripts/                One-off PowerShell helpers (e.g. probe-daemon.ps1)
├── target/                 Cargo workspace build output (gitignored)
├── README.md               Short overview + build/status pointers
├── LEARNINGS.md            Discovered gotchas — read before iTunes-DB work
└── AGENTS.md               (this file)
```

## Rust core (`crates/classick/src/`)

Single `classick` binary, library + bin layout (`lib.rs` re-exports modules,
`main.rs` is a thin wrapper).

Key modules — read these to understand a given concern:

| Module | What lives there |
|---|---|
| `main.rs` | Mode selection (daemon / IPC / TUI / plain), logging init, hands off to `orchestrator` |
| `cli.rs` | `clap` definitions for `Cli` |
| `orchestrator.rs` | Top-level run flow: config-reset loop → wizard → `apply_loop::run` |
| `apply_loop.rs` | Per-`Action` (Add/Modify/Remove/Metadata) match arms, `add_one`, `do_metadata_only`, `build_rebuild_manifest`, periodic checkpoints |
| `source.rs` | Recursive source walk + per-file metadata extraction |
| `manifest.rs` | JSON manifest (per-track state); `Action` plan computation |
| `preflight.rs` | iTunes-running guard, mount checks, libgpod sanity |
| `transcode.rs` | ffmpeg / refalac shellout + ffprobe parsing |
| `tags.rs` | Tag normalization from ffprobe output |
| `ipod/` | libgpod wrappers: `OwnedDb`, device + layout helpers |
| `ffi.rs` | Raw bindgen-generated libgpod bindings |
| `progress.rs`, `worker_wire.rs` | ratatui/plain progress plus protocol-3 worker transport |
| `wire/` | Shared protocol-3 messages, routing, validation, and golden-vector authority |
| `daemon/` | Long-lived daemon mode: runtime, IPC server, device watcher, scheduler, sync orchestrator, history |
| `config.rs`, `config_file.rs` | CLI → resolved Config; TOML persistence at `%APPDATA%\classick\config.toml` |
| `wizard.rs` | Interactive TUI first-run wizard (CLI side; UI has its own) |
| `scsi_inquiry.rs`, `sysinfo_extended.rs` | Windows-only SCSI pass-through to identify exact iPod model |
| `windows_proc.rs` | `NoConsoleWindow` helper for child processes (suppresses cmd flash) |
| `try_with_prompt.rs` | Retry/Skip/Abort prompt loop used by apply-loop error sites |
| `logging.rs` | `tracing-subscriber` init; routes to file in IPC/daemon mode, stderr in TUI mode |

### Build & test

Prerequisites:
- Rust stable on the MSVC toolchain.
- **MSYS2 at `C:\msys64`** (overridable via `MSYS2_ROOT` env var). `build.rs`
  uses MSYS2's MinGW64 sysroot for GLib headers (bindgen input — libgpod's
  `itdb.h` includes `<glib.h>`) and copies `gdk-pixbuf-query-loaders.exe` plus
  pixbuf loader DLLs into the output dir for runtime image decoding.
- Vendored libgpod under `crates/classick/vendor/libgpod/` (already
  committed; nothing to do).

```powershell
# From repo root (workspace root), in a normal PowerShell prompt
cargo build --release            # release build → target/release/classick.exe
cargo build                      # debug build  → target/debug/classick.exe
cargo test                       # unit + integration tests
cargo test -- --test-threads=1   # serialize when poking the real pipe
```

Cargo workspace target dir is at the repo root, so `target/release/` and
`target/debug/` paths are unchanged from the pre-workspace layout — handy for
the .NET csproj that bundles the binary as build-time content.

A successful build also copies the vendored libgpod runtime DLLs into the
target directory (handled by `build.rs`). Missing those DLLs is the most
common cause of "gpod.dll was not found" at startup.

The daemon-integration tests under `crates/classick/tests/daemon_runtime_integration.rs`
need a per-test config + pipe sandbox; see the `sandbox()` helper there and
the corresponding entry in `LEARNINGS.md` if you're touching them.

### Rust conventions

- `anyhow::Result` at the top, `thiserror` only when a typed error materially
  helps a caller. Wrap with `.context(...)` at every meaningful boundary so
  `{e:#}` walks a useful chain on the UI side.
- `tracing::{info, warn, error, debug}` — no `println!` outside `examples/`.
  In IPC mode `stdout` IS the wire; any stray print corrupts the JSON stream.
- Windows-specific code goes behind `#[cfg(windows)]`. Don't break the
  non-Windows test compile if you can avoid it.
- `unsafe` is allowed for the libgpod FFI layer; everything above `ipod/`
  should be safe Rust.
- Long-running subprocess invocations on Windows MUST go through
  `windows_proc::NoConsoleWindow` to suppress the cmd-window flash.

## Windows UI (`ui/windows/`)

WinUI 3 tray app, .NET 10, x64 + ARM64. See `ui/windows/README.md` for full
detail; the short version:

```powershell
cd ui\windows
dotnet build Classick.UI.slnx -c Debug
dotnet test  Classick.UI.Tests/Classick.UI.Tests.csproj
dotnet run --project Classick.UI/Classick.UI.csproj
```

The UI csproj bundles `..\..\..\target\release\classick.exe` (+ libgpod DLLs)
into its output directory at build time, so a clean dev loop is:

```powershell
# From workspace root:
cargo build --release
cd ui\windows
dotnet run --project Classick.UI/Classick.UI.csproj
```

A `WarnIfCoreMissing` MSBuild target warns (does not fail) when the Rust
binary hasn't been built yet.

## macOS UI (`ui/macos/`)

SwiftUI menu-bar app (SPM package), macOS 15+, Swift 6 strict concurrency. See
`ui/macos/README.md` for full detail; the short version:

```bash
# From workspace root — the app embeds + spawns the daemon:
cargo build --release
ui/macos/bundle.sh                    # -> ui/macos/Classick.app (ad-hoc, for dev)
cd ui/macos && swift test             # unit tests (WireCodec, AppModel reducer, …)
```

Transcoding on macOS is the system `afconvert` (never bundle ffmpeg on macOS).
The app is **not** sandboxed — it spawns a daemon that needs raw device access
and a shared `$TMPDIR` Unix-socket; enabling the App Sandbox breaks IPC. Signed
+ notarized release: `scripts/release-macos.sh <version>` (Developer ID identity
+ `classick-notary` profile in the Keychain); the Sparkle appcast is published
to the `gh-pages` branch — see `LEARNINGS.md` for the appcast-URL gotcha.

## IPC contract

`docs/ipc-protocol.md` is the source of truth. Implementations on each side:

- Rust: `crates/classick/src/wire/` (shared records and validation),
  `crates/classick/src/daemon/ipc_server.rs` (desktop transport), and
  `crates/classick/src/worker_wire.rs` (worker transport)
- C#: `ui/windows/Classick.UI.Core/Ipc/WireCodec.cs`,
  `WireDeviceModels.cs`, and `WireOperationModels.cs`
- Swift: `ui/macos/Sources/Classick/Ipc/WireModels.swift` (event/command
  Codables) + `DaemonClient.swift` (Unix-socket transport)

Versioning is semver with a `hello` handshake — see §1 of the protocol doc.
A breaking change anywhere on the wire is a major bump and both sides must
move together.

The named-pipe label `\\.\pipe\classick` is set by `PIPE_NAME` in
`crates/classick/src/daemon/ipc_server.rs` (with `default_pipe_name()`
returning a Unix-socket path on non-Windows) and mirrored on the .NET side by
`Classick.UI.Core.AppIdentity`. **These two MUST stay in sync** — the pipe
label is the IPC contract.

## Conventions across the repo

- **Conventional Commits.** `feat(scope): …`, `fix(scope): …`, etc. Scopes in
  use: `daemon`, `ui`, `ipc`, `apply-loop`, `transcode`, `tags`,
  `manifest`, `wizard`, `preflight`, `progress`, `docs`, `build`, `ci`,
  `chore`. The recent `git log` is the best style guide. (The older
  `ui-windows` scope was renamed to `ui` when the directory moved under
  `ui/windows/`.)
- **`LEARNINGS.md`.** New gotchas, debugging insights, and non-obvious
  conventions go here, one bullet per learning. Check for duplicates before
  adding. Don't log routine info.
- **Design docs.** Substantial or ambiguous changes get a concise design under
  `docs/design/` before implementation. Keep only durable decisions in the
  active docs set; completed task scripts belong in Git history.
- **Bug fixes get regression tests** where reasonable — see the daemon-runtime
  integration suite for the pattern.
- **Keep files ≤ ~500 LOC.** Split aggressively. `apply_loop.rs` is the one
  that always wants to grow.
- **Comments earn their place.** Default to none. When you write one,
  document *why* (a hidden constraint, a workaround, a surprising invariant).
  Don't restate what the code does.

## Things to remember when editing

These hit hard in practice — see `LEARNINGS.md` for the full incident reports:

- **A libgpod-managed iPod is not intrinsically incompatible with Apple
  software.** On-device verification shows correctly identified and signed
  Classick databases remain manageable by Finder/iTunes/Music. The
  `preflight::verify_itunes_not_running` guard is a conservative
  concurrent-writer safety measure while Classick mutates device state.
- **Classick does not initialize restored iPods.** A valid Apple-created
  `iTunesDB` is the current mutation baseline. Initialization support is
  explicitly deferred; do not manufacture the initial database or Apple
  preferences as part of an unrelated fix.
- **db.write() checkpointing matters.** Every Nth track
  (`SYNC_CHECKPOINT_EVERY` in `crates/classick/src/lib.rs`) the apply loop
  flushes the in-memory DB + manifest so a crash leaves at most N orphans,
  not the whole library.
- **Mounted database parsing is artwork-aware.** `OwnedDb::open` must use
  libgpod's mount-aware `itdb_parse`, because `itdb_parse_file` does not load
  the companion `ArtworkDB`. The pinned libgpod also double-frees a parsed
  Genius CUID unless the compatibility drop path takes and nulls it first.
- **ffmpeg needs `-nostdin` + `.stdin(Stdio::null())`.** Inherited piped
  stdin from a daemon-spawned subprocess wedges ffmpeg at ~97% of a track
  during stream finalization. Both layers stay.
- **Daemon shutdown must kill the sync subprocess explicitly.** Windows has
  no SIGHUP-style parent-death signal; a naive `std::process::exit(0)` skips
  Drop and orphans the child. Always go through the orchestrator's bounded-
  kill path.
- **Don't use `--no-verify` or skip hooks.** If a pre-commit hook fails, fix
  the underlying issue. Per CLAUDE.md, never amend — make a new commit.
- **`git add -A` / `git add .` is forbidden by default.** Stage specific files
  by name. The only exception is when nothing is staged at all.

## Where to look for more context

- `docs/README.md` — current documentation index and authority rules
- `docs/architecture.md` — current architecture and data ownership
- `docs/device-safety.md` — device, publication, and recovery invariants
- `LEARNINGS.md` — concise current gotchas and debugging insights
- `docs/ipc-protocol.md` — wire format authority
- `docs/archive/` — historical context only; never current authority
- `ui/windows/README.md` — WinUI-specific build/run/test/conventions
- `git log` — recent commits are the best signal for what's in flux
