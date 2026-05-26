# AGENTS.md — Classick

Orientation for agent-driven work in this repo. Read this first, then dive into
the specific area you're touching.

## What this is

Windows-native sync tool that copies a FLAC library to an iPod Classic,
transcoding to ALAC on the fly. Two halves:

1. **Rust core (`crates/classick/`)** — single self-contained `classick.exe`.
   Wraps libgpod via FFI; spawns ffmpeg (or refalac) for transcode; writes the
   iTunesDB. Runs in three modes: `--ipc-mode` (subprocess driven by a GUI),
   `--daemon` (long-lived tray companion), or interactive TUI (default when
   stdout is a TTY). Daemon mode compiles cross-platform (Unix-socket
   transport on non-Windows); the device-detection layer is still Windows-only.
2. **WinUI 3 tray app (`ui/windows/`)** — .NET 10 desktop app. Lives in the
   system tray, owns the daemon process, surfaces device state + sync progress
   + settings + first-run wizard.

The two halves talk over a named pipe (`\\.\pipe\classick`) for daemon
commands and over stdin/stdout newline-delimited JSON for per-sync events. The
wire format is in `docs/ipc-protocol.md` — that document is the source of
truth, both implementations must agree with it.

For the full design rationale and the rejected-alternatives table, see
`docs/SPEC.md`. For battle-scars and hard-won gotchas, see `LEARNINGS.md` —
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
│   └── windows/            WinUI 3 / .NET 10 tray app (see its own README).
│                           Future: ui/macos, ui/linux when those land.
├── docs/
│   ├── ipc-protocol.md     Wire format (Rust ↔ UI). Source of truth.
│   ├── SPEC.md             Full original design spec
│   ├── SCSI.md             SCSI INQUIRY notes (SysInfoExtended path)
│   └── superpowers/        Phase specs, plans, code reviews
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
| `progress.rs` | ratatui TUI + Plain + IPC progress backends, decision channel |
| `ipc.rs`, `ipc_daemon.rs` | Serde mirrors for the wire format (events, commands) |
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

## IPC contract

`docs/ipc-protocol.md` is the source of truth. Implementations on each side:

- Rust: `crates/classick/src/ipc.rs` (event/command records) +
  `crates/classick/src/progress.rs::run_ipc` (channel-to-wire backend)
- C#: `ui/windows/Classick.UI.Core/Ipc/IpcEvent.cs` and `IpcCommand.cs`
  (subprocess wire), `DaemonEvent.cs` / `DaemonCommand.cs` (daemon-pipe wire)

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
- **Design specs.** Major changes get a written design in
  `docs/superpowers/specs/YYYY-MM-DD-<topic>.md` before implementation.
  Implementation plans go in `docs/superpowers/plans/`. Reviews in
  `docs/superpowers/reviews/`.
- **Bug fixes get regression tests** where reasonable — see the daemon-runtime
  integration suite for the pattern.
- **Keep files ≤ ~500 LOC.** Split aggressively. `apply_loop.rs` is the one
  that always wants to grow.
- **Comments earn their place.** Default to none. When you write one,
  document *why* (a hidden constraint, a workaround, a surprising invariant).
  Don't restate what the code does.

## Things to remember when editing

These hit hard in practice — see `LEARNINGS.md` for the full incident reports:

- **iTunes will reject any libgpod-managed iPod** with a "cannot read,
  please Restore" dialog. The fundamental signature mismatch can't be fixed
  without reverse-engineering Apple's signing. We ship the
  `preflight::verify_itunes_not_running` guard and explicit warning copy in
  the wizard + Settings to keep users out of the Restore-loop trap.
- **db.write() checkpointing matters.** Every Nth track
  (`SYNC_CHECKPOINT_EVERY` in `crates/classick/src/lib.rs`) the apply loop
  flushes the in-memory DB + manifest so a crash leaves at most N orphans,
  not the whole library.
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

- `docs/SPEC.md` — full original design spec, rejected alternatives, FFI rationale
- `docs/SCSI.md` — SCSI INQUIRY notes for the SysInfoExtended path
- `LEARNINGS.md` — incidents, gotchas, debugging insights (chronological)
- `docs/ipc-protocol.md` — wire format authority
- `docs/superpowers/specs/` — per-phase design docs (read the newest first)
- `ui/windows/README.md` — WinUI-specific build/run/test/conventions
- `git log` — recent commits are the best signal for what's in flux
