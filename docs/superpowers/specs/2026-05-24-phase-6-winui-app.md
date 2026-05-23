# Phase 6 — Native Windows UI (WinUI 3) over JSON IPC

**Goal:** Ship a polished, native-feeling Windows desktop app for ipod-sync that drives the existing Rust core via a stable, versioned IPC protocol. The TUI stays as the cross-platform fallback; macOS (SwiftUI) and Linux (GTK/Adwaida) frontends will reuse the same IPC contract in later phases.

**Scope statement:** Define the architecture, IPC contract, and milestone breakdown for the Windows-first native UI. This spec covers M1–M4. The detailed M1 implementation plan lives in `docs/superpowers/plans/2026-05-24-phase-6-m1-ipc-shell.md`.

## Locked-in tech stack

| Layer | Choice | Notes |
|---|---|---|
| UI language / runtime | C# 13 + **.NET 10** (LTS, Nov 2025) | Long-term support; ships with VS 2022 17.10+ |
| UI framework | **WinUI 3** via Windows App SDK (latest stable, 1.6+) | Native controls, Mica/Acrylic, modern XAML |
| Packaging (M1–M3) | **Unpackaged** WinUI 3 app | Simpler dev loop; no MSIX needed until distribution |
| Packaging (M4) | MSIX, sideload signed | Defer signing/store decisions to M4 |
| MVVM helper | CommunityToolkit.Mvvm | `[ObservableProperty]`, `[RelayCommand]` source generators |
| IPC transport | **Newline-delimited JSON over stdin/stdout** | UTF-8, one message per line |
| IPC protocol shape | Custom typed-envelope (not JSON-RPC 2.0) | One client; no batching/notifications distinction; keeps Rust + C# serde simple |
| Rust core entry | `ipod-sync.exe --ipc-mode` | New flag; replaces TUI backend with `IpcBackend` |
| Repo layout | Sibling `ui-windows/` directory at repo root | Independent `.sln`/`.csproj`; Cargo workspace stays Rust-only |
| Logging (Rust side, IPC mode) | File-only: `%LOCALAPPDATA%\ipod-sync\logs\{timestamp}.log` | stdout reserved for JSON stream |
| Logging (C# side) | Same `%LOCALAPPDATA%\ipod-sync\logs\` dir, `ui-{timestamp}.log` | Symmetric with Rust side; easy correlation |

---

## Architecture diagram

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  Windows                                                                     │
│                                                                              │
│   ┌────────────────────────────┐       ┌────────────────────────────────┐    │
│   │  IpodSync.UI.exe           │       │  ipod-sync.exe                 │    │
│   │  (WinUI 3 + .NET 10)       │       │  (Rust core)                   │    │
│   │                            │       │                                │    │
│   │  ┌──────────────────────┐  │       │  ┌──────────────────────────┐  │    │
│   │  │ MainWindow / Pages   │  │       │  │ main.rs                  │  │    │
│   │  │   Review / Progress  │  │       │  │  ↓ branches on flag      │  │    │
│   │  │   Wizard / Config    │  │       │  │ orchestrator + apply_loop│  │    │
│   │  └─────────┬────────────┘  │       │  └──────────┬───────────────┘  │    │
│   │            │ ViewModels    │       │             │                  │    │
│   │  ┌─────────▼────────────┐  │       │  ┌──────────▼───────────────┐  │    │
│   │  │ CoreProcess (IPC)    │  │       │  │ Progress backends        │  │    │
│   │  │  - spawn child       │  │       │  │   TuiBackend (default)   │  │    │
│   │  │  - read events       │◄─┼───────┼──┤   PlainBackend (--no-tui)│  │    │
│   │  │  - write commands    │──┼───────┼─►│   IpcBackend (--ipc-mode)│  │    │
│   │  │  - channels.Reader   │  │       │  └──────────────────────────┘  │    │
│   │  └──────────────────────┘  │       │                                │    │
│   │                            │       │   Tracing → file (IPC mode)    │    │
│   │   Log → file               │       │   Tracing → stderr (TUI/plain) │    │
│   └────────────────────────────┘       └────────────────────────────────┘    │
│                                                                              │
│            ────────────── stdin/stdout, NDJSON, UTF-8 ──────────────         │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘

Existing sibling modes (unchanged):
  - ipod-sync.exe            → TuiBackend  (ratatui, interactive)
  - ipod-sync.exe --no-tui   → PlainBackend (line-oriented log)
  - ipod-sync.exe --ipc-mode → IpcBackend  (NEW; JSON over stdin/stdout)
```

---

## Why this architecture

### Why native-per-platform instead of Tauri / Electron / cross-platform widgets

The post-v1 roadmap (`2026-05-18-post-v1-roadmap.md` Phase 6) listed Tauri as the leading candidate. Reversed for these reasons:

- **Polish ceiling.** WinUI 3 gets Mica, NavigationView, dark-mode parity, MenuFlyouts, and AnimatedIcon for free. A WebView2 app simulating those is endless catch-up work.
- **Per-platform native ≠ 3× the work.** The Rust core is the same; only the view layer changes. Each platform's view layer is small (~5–10k LOC each at maturity); the tedious part — sync orchestration, libgpod, transcoding — is already done.
- **Windows-first is achievable.** Solo developer, primary platform is Windows. Ship Windows native, then add macOS/Linux as separate frontends sharing the same IPC contract. No "great everywhere" launch deadline.
- **Existing TUI covers the cross-platform fallback.** Anyone on Linux/macOS who wants the tool today still gets a working interactive experience via the TUI. Phase 6 doesn't break that.

### Why IPC over FFI or in-process embedding

- **Crash isolation.** A libgpod write panic in the Rust core takes down the core process, not the UI. The UI shows an error dialog and can retry.
- **Build independence.** C# devs don't need a Rust toolchain to iterate on UI; Rust devs don't need Visual Studio to iterate on the core.
- **Reuse across UIs.** macOS (SwiftUI) and Linux (GTK) frontends consume the same protocol. No per-platform FFI binding maintenance.
- **Distribution simplicity.** Ship two .exe files. No P/Invoke marshaling for `Itdb_iTunesDB *` pointers. No `csbindgen`/`uniffi` build step.
- **Already-channel-shaped.** The Rust core's `Progress` system is already a `(Sender<ProgressEvent>, Receiver<Decision>)` channel pair. The IPC backend just serializes/deserializes the same enums to/from JSON. Conceptually it's one more `*Backend` variant alongside `TuiBackend` and `PlainBackend`.

### Why stdin/stdout JSON over named pipes / TCP / WebSocket

- **No port allocation, no firewall prompts, no auth.** Parent process owns the handle.
- **OS lifecycle for free.** When the UI dies, the OS closes the child's stdin, and the core's read loop sees EOF and shuts down. No zombie processes from netcode bugs.
- **Trivial debugging.** Pipe the core's output to a file: `ipod-sync.exe --ipc-mode > messages.ndjson < commands.ndjson`. Replay protocol traces.
- **Single client by design.** Phase 5 (daemon) might later expose a named-pipe protocol for multi-client scenarios. Phase 6's UI doesn't need that — it owns its core process exclusively.

### Why a custom typed-envelope protocol instead of JSON-RPC 2.0

- **One client; no batching.** JSON-RPC 2.0's request/response correlation, batch arrays, and notification-vs-request distinction add ceremony we don't need.
- **The Rust types already shape it.** `ProgressEvent` and `Decision` are tagged unions; serde's `#[serde(tag = "type", rename_all = "snake_case")]` gives us a clean wire format directly. C# `System.Text.Json` polymorphism with `[JsonDerivedType]` reads it symmetrically.
- **Versioning is simpler.** Single `protocol_version` string in the `hello` event. No JSON-RPC version field plus our own to manage.
- **Smaller wire size.** No `{"jsonrpc":"2.0","method":"...","params":{...}}` envelope around every message.

### Why .NET 10 + WinUI 3 (and not WPF / WinForms / .NET Framework)

- **WinUI 3 is the current Windows-native story.** Microsoft's official direction; gets Fluent design updates first.
- **.NET 10 is LTS** (released Nov 2025, supported through Nov 2028). Stable target for a long-lived consumer app.
- **C# 13** features (collection expressions, primary constructors) cut boilerplate; CommunityToolkit.Mvvm source generators are mature.
- **WPF is fine but old-feeling.** No Mica, no modern NavigationView, controls library skews enterprise.
- **WinForms is out** — no modern look, poor HiDPI defaults.

### Why sibling `ui-windows/` instead of a `crates/ui-windows/` nested project

- **Different toolchain.** C# project files (`.sln`/`.csproj`) don't belong in a Cargo workspace. Sibling directory keeps Cargo's `cargo build` clean.
- **Future-proofs for `ui-macos/` and `ui-linux/`.** Same pattern.
- **Independent CI.** GitHub Actions can run `cargo test` and `dotnet test` in separate jobs without cross-pollution.
- **Cross-references stay valid.** The UI's `CoreProcess` knows where `ipod-sync.exe` is (sibling in dev: `..\..\target\release\ipod-sync.exe`; sibling in production install: `.\ipod-sync.exe`).

---

## IPC protocol

### Wire format

- **Encoding:** UTF-8.
- **Framing:** newline-delimited JSON. Each line is a complete JSON object. No trailing comma, no pretty-printing on the wire (single-line JSON only).
- **Direction A (core → UI):** events written to the core's stdout, one per line, as they happen.
- **Direction B (UI → core):** commands written to the core's stdin, one per line, when the user acts.
- **No interleaving constraint.** The core may emit multiple events while waiting for a command response. The UI must keep its read loop running at all times.
- **Encoding of types:** snake_case JSON, lower_snake for enum variants. The `type` field is the discriminator on every message.

### Correlation

Prompt and form events carry an `id: u64`. The matching decision command MUST echo the same `id`. Stale or out-of-order responses are dropped by the core. The UI's `CoreProcess` should drop responses to ids it never sent.

### Versioning

The first event the core emits after spawn is:

```json
{"type":"hello","protocol_version":"1.0.0","core_version":"0.1.0"}
```

- `protocol_version` is semver. Bump major on breaking changes; bump minor for additive events the UI can ignore; bump patch for doc-only changes.
- `core_version` is `env!("CARGO_PKG_VERSION")` — informational, shown in the UI's About dialog.
- The UI verifies `protocol_version` major matches its supported range before sending any command. Mismatch → show an error dialog, do not proceed.

### Error model

- Errors are non-fatal by default — the core may emit any number of `error` events during a run.
- A fatal error is followed immediately by a `finish` event with `success: false`.
- The UI accumulates errors into a list and surfaces them in a "Sync log" panel or modal.
- `recovery_hints` is an optional array of short strings the UI can render as suggested next steps.

```json
{"type":"error","message":"ffmpeg failed for /path/to/track.flac","recovery_hints":["Skip this track","Verify the source file isn't corrupt"]}
```

### Process lifecycle

1. UI spawns `ipod-sync.exe --ipc-mode` with stdin/stdout piped, stderr inherited or captured.
2. UI reads the `hello` event. Verifies protocol_version. Sends `start` command (currently a no-op; reserved for future per-run options) — or directly waits for events as the core runs the existing orchestrator.
3. UI processes events; sends decisions when prompted.
4. On user-initiated shutdown:
   - UI sends `{"type":"cancel"}`.
   - UI waits up to **5 seconds** for the core to emit `finish` and close stdout (EOF).
   - If timeout exceeded: UI calls `Process.Kill(entireProcessTree: true)`.
5. On core crash:
   - UI sees stdout EOF without a `finish` event.
   - UI shows a crash dialog with the captured stderr tail and a "Show log file" button.

This mirrors the bounded-join pattern `Progress::finish` already uses in `src/progress.rs` (5s deadline, force-exit on timeout).

---

### Message types — events (core → UI)

All messages carry a `type` discriminator. Unknown `type` values are ignored by the UI (forward-compat).

| `type` | Direction | Fields | Purpose |
|---|---|---|---|
| `hello` | core→UI | `protocol_version: string`, `core_version: string` | First message; protocol handshake |
| `header` | core→UI | `source: string`, `ipod: string`, `manifest: string` | Resolved paths for display |
| `summary` | core→UI | `add: u32`, `modify: u32`, `metadata_only: u32`, `remove: u32`, `unchanged: u32`, `total_planned: u32` | Action plan counts |
| `review` | core→UI | `summary: ActionPlanSummary`, `no_delete: bool` | Request review decision |
| `prompt` | core→UI | `id: u64`, `message: string`, `options: string[]` | Modal multi-choice prompt |
| `form` | core→UI | `id: u64`, `label: string`, `initial: string`, `hint: string` | Text-input prompt |
| `track_start` | core→UI | `current: u32`, `total: u32`, `label: string` | Per-track progress |
| `track_done` | core→UI | (none) | Increment progress |
| `log` | core→UI | `message: string` | Informational log line |
| `error` | core→UI | `message: string`, `recovery_hints?: string[]` | Non-fatal or fatal error |
| `finish` | core→UI | `success: bool` | Run complete; core will close stdout shortly |

### Message types — commands (UI → core)

| `type` | Direction | Fields | Purpose |
|---|---|---|---|
| `start` | UI→core | (none; reserved for future options) | Begin orchestration. (M1: implicit on spawn; reserved for M2+) |
| `review_decision` | UI→core | `choice: "apply" \| "dry_run" \| "quit"`, `no_delete: bool` | Response to `review` event |
| `prompt_decision` | UI→core | `id: u64`, `choice: u32` | Response to `prompt` event |
| `form_decision` | UI→core | `id: u64`, `value: string \| null` | Response to `form` event; null = aborted |
| `cancel` | UI→core | (none) | Request graceful shutdown |

### JSON examples

```json
{"type":"hello","protocol_version":"1.0.0","core_version":"0.1.0"}
{"type":"header","source":"\\\\nas\\music\\flac","ipod":"G:\\","manifest":"C:\\Users\\me\\AppData\\Roaming\\ipod-sync\\manifests\\000a1b2c3d4e5f60.json"}
{"type":"summary","add":12,"modify":3,"metadata_only":0,"remove":0,"unchanged":1260,"total_planned":15}
{"type":"review","summary":{"add":12,"modify":3,"metadata_only":0,"remove":0,"unchanged":1260},"no_delete":false}
```

UI replies:
```json
{"type":"review_decision","choice":"apply","no_delete":false}
```

Core resumes:
```json
{"type":"track_start","current":1,"total":15,"label":"Aphex Twin - Selected Ambient Works II - #ATC1"}
{"type":"log","message":"transcoded via ffmpeg n7.0 in 6.3s"}
{"type":"track_done"}
```

Mid-sync prompt:
```json
{"type":"prompt","id":7,"message":"ffmpeg failed for track 'Boards of Canada - Rocket'. Choose:","options":["Retry","Skip this track","Abort"]}
```

UI replies:
```json
{"type":"prompt_decision","id":7,"choice":1}
```

Form prompt (first-launch wizard, M2):
```json
{"type":"form","id":1,"label":"Enter the path to your FLAC source library","initial":"","hint":"UNC paths like \\\\server\\music are supported"}
```

UI replies:
```json
{"type":"form_decision","id":1,"value":"\\\\nas\\music\\flac"}
```

Or aborted:
```json
{"type":"form_decision","id":1,"value":null}
```

Run end:
```json
{"type":"finish","success":true}
```

---

## Rust changes for `--ipc-mode`

High-level (detailed task breakdown in the M1 plan):

### New CLI flag

```rust
/// Speak JSON-over-stdio instead of rendering a TUI. Used by the WinUI
/// frontend (and any future native UI). Disables the TUI; routes tracing
/// to a file under %LOCALAPPDATA%\ipod-sync\logs\.
#[arg(long)]
pub ipc_mode: bool,
```

`--ipc-mode` is mutually exclusive with `--no-tui` (rejected at parse time with a clap conflict).

### New `IpcBackend` in `progress.rs`

`Progress::start` gains an `ipc_mode: bool` parameter. The dispatch becomes:

```rust
pub fn start(use_tui: bool, ipc_mode: bool) -> Result<(Self, Receiver<Decision>)> {
    let backend = if ipc_mode {
        Backend::Ipc
    } else if use_tui && std::io::stdout().is_terminal() {
        Backend::Tui
    } else {
        Backend::Plain
    };
    // ... existing channel setup, spawn the appropriate run_* function ...
}
```

`run_ipc(event_rx, decision_tx)` mirrors `run_tui`/`run_plain` but:
- Serializes each `ProgressEvent` to a single line of JSON on stdout (`println!` is fine; stdout is line-buffered on Windows when piped — confirm and add `stdout().flush()` after each write to be safe).
- Spawns a second OS thread that reads lines from stdin, deserializes each as a `Command`, and routes the result to `decision_tx` as a `Decision`.
- Emits the `hello` event before draining `event_rx`.
- On `Finish` event, emits `{"type":"finish","success":true}` (or `success: false` if the run errored — the orchestrator passes this via a separate channel or a final event variant; see M1 Task 2).
- Does NOT enable crossterm raw mode or alternate screen.

### Logging routing

In `logging::init`, add a third mode:

```rust
pub fn init(verbose: bool, use_tui: bool, ipc_mode: bool) {
    // ...
    if ipc_mode {
        let log_path = log_file_path()?; // %LOCALAPPDATA%\ipod-sync\logs\<ts>.log
        let file = std::fs::File::create(&log_path)?;
        builder.with_writer(Mutex::new(file)).init();
    } else if use_tui {
        builder.with_writer(std::io::sink).init();
    } else {
        builder.init();
    }
}
```

The IPC backend does NOT route tracing through itself (no `log` event per tracing line) — too noisy, and a JSON-parser surfacing every libgpod CRITICAL would be miserable. Tracing goes to the file; only deliberate `progress.log()` / `progress.error()` calls cross the IPC boundary.

### `main.rs` branch

```rust
let use_tui = !cli.no_tui && !cli.ipc_mode && std::io::stdout().is_terminal();
ipod_sync::logging::init(cli.verbose, use_tui, cli.ipc_mode);
let (progress, decision_rx) = Progress::start(use_tui, cli.ipc_mode)?;
```

Everything downstream of `Progress::start` is unchanged — the orchestrator doesn't know or care which backend is active.

---

## C# / WinUI 3 project layout

```
F:\repos\ipod-sync\ui-windows\
├── IpodSync.UI.sln                              (Visual Studio solution)
├── README.md                                    (build prereqs, dev loop)
├── .gitignore                                   (bin/, obj/, .vs/, *.user)
├── IpodSync.UI\                                 (WinUI 3 app project)
│   ├── IpodSync.UI.csproj
│   ├── App.xaml / App.xaml.cs
│   ├── MainWindow.xaml / MainWindow.xaml.cs
│   ├── Assets\                                  (icons, splash)
│   ├── Views\
│   │   ├── ReviewPage.xaml / .cs
│   │   ├── ProgressPage.xaml / .cs
│   │   ├── WizardPage.xaml / .cs                (M2)
│   │   ├── ConfigPage.xaml / .cs                (M2)
│   │   └── LibraryPage.xaml / .cs               (M3)
│   ├── ViewModels\
│   │   ├── MainViewModel.cs
│   │   ├── ReviewViewModel.cs
│   │   └── ProgressViewModel.cs
│   ├── Services\
│   │   ├── ICoreProcess.cs                      (interface for testability)
│   │   ├── CoreProcess.cs                       (real subprocess + IPC)
│   │   └── CoreLocator.cs                       (finds ipod-sync.exe)
│   ├── Models\
│   │   ├── Events.cs                            (event records: Hello, Header, Summary, ...)
│   │   ├── Commands.cs                          (command records: ReviewDecision, ...)
│   │   └── ProtocolVersion.cs
│   └── Converters\                              (XAML value converters)
└── IpodSync.UI.Tests\                           (xUnit test project)
    ├── IpodSync.UI.Tests.csproj
    ├── CoreProcessTests.cs                      (mock ICoreProcess)
    └── ProtocolSerializationTests.cs            (round-trip JSON for every message type)
```

**Notes:**
- **One project for M1.** `IpodSync.UI` holds both the IPC client and the UI. Split into `IpodSync.UI` + `IpodSync.Core` (class library) only if M2+ needs the IPC client reused elsewhere (e.g. a separate CLI tool or tray app). Premature factoring until then.
- **Unpackaged** (Project property `<WindowsPackageType>None</WindowsPackageType>`). MSIX packaging deferred to M4.
- **MVVM via CommunityToolkit.Mvvm.** ViewModels derive from `ObservableObject`; properties use `[ObservableProperty]`; commands use `[RelayCommand]`. The toolkit source generators emit boilerplate at compile time — no runtime reflection.
- **System.Text.Json with polymorphic serialization.** Event/command records use `[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]` + `[JsonDerivedType(typeof(HelloEvent), "hello")]` etc. No third-party JSON library.
- **System.Threading.Channels** for the read loop → ViewModel handoff. Bounded channel (capacity 256), single producer / single consumer.

---

## Milestone overview

### M1 — IPC protocol + Rust `--ipc-mode` + minimal WinUI shell (~1 week)

**Deliverables:**
- `docs/ipc-protocol.md` — formal protocol spec with JSON schemas.
- Rust: `--ipc-mode` CLI flag, `IpcBackend` in `progress.rs`, file-based tracing routing, `main.rs` branch.
- C#: `IpodSync.UI` solution and project, app launches, shows `Hello, ipod-sync` and a `Start sync` button.
- C#: `CoreProcess` class spawns `ipod-sync.exe --ipc-mode`, reads events, writes commands.
- C#: `ReviewPage` displays the action plan, has Apply / Dry-run / Quit buttons.
- C#: `ProgressPage` shows a progress bar driven by `track_start`/`track_done`, plus a log tail.
- Manual smoke test: real sync runs end-to-end via the WinUI app against a real iPod.

**Shippable at end of M1:** A working WinUI app that can drive a sync. Path config still uses `config.toml`; no wizard, no library browser, no installer. Useful for the developer (you) and brave testers.

**Detailed plan:** `docs/superpowers/plans/2026-05-24-phase-6-m1-ipc-shell.md`.

### M2 — First-launch wizard + config panel (~3-5 days)

**Adds:**
- `WizardPage` for first-launch: source folder picker (native `FolderPicker`), iPod mount auto-detect with manual override, save-and-continue.
- `ConfigPage` to view/edit persisted config: source path, ffmpeg path, encoder choice, --no-delete, --passthrough-wav defaults.
- C# reads/writes `%APPDATA%\ipod-sync\config.toml` directly (mirroring the Rust `config_file` module's behavior) so the wizard can persist before any sync runs. Alternative: extend the IPC protocol with a `read_config`/`write_config` round-trip; decide during M2 brainstorm.
- IPC additions: probably `request_config` / `config` (event) and `write_config` (command) if we go with the Rust-mediated path. Or no IPC changes if C# writes the TOML directly.

**Shippable at end of M2:** A WinUI app a non-developer can install and configure without ever opening the TUI or editing TOML.

### M3 — Library browser (~1-2 weeks)

**Adds:**
- `LibraryPage` showing a virtualized list of tracks: artist, album, title, source vs iPod status (present/missing/modified/etc.).
- Filter by status, search by title/artist/album.
- Per-track context menu: "Re-sync this track", "Exclude from sync".
- IPC additions: `list_tracks` command + `tracks` event chunked over multiple messages (probably 100 per chunk to keep the wire fast).
- Backed by manifest data (no full source rescan required for browse).
- Album art thumbnails (decoded from manifest's referenced source files, or cached on disk).

**Shippable at end of M3:** Feature parity with what iTunes used to do for browsing.

### M4 — Polish + distribution (~1 week, overlaps with future distribution work)

**Adds:**
- MSIX packaging, sideload signing.
- Dark mode QA pass; Mica/Acrylic backdrops.
- Accessibility: AutomationProperties on all controls, full keyboard navigation, screen reader sweep with Narrator.
- Custom dialogs: replace the M1 "ipod-sync.exe not found" message box with a proper recovery dialog.
- About box, version display, link to GitHub.
- Optional: code-signed installer (EV cert decision deferred — depends on whether we want SmartScreen to recognize the publisher immediately).

**Shippable at end of M4:** A signed MSIX installer you can hand to a friend who doesn't know what a terminal is.

---

## Risks and open questions

| # | Risk | Likelihood | Mitigation |
|---|---|---|---|
| 1 | **Windows App SDK update churn breaks the build.** Microsoft has shipped breaking changes between WinUI 3 versions before. | Med | Pin to a specific Windows App SDK version in csproj; document the version in the README; bump deliberately. |
| 2 | **stdout buffering on Windows pipes corrupts the JSON stream.** Rust's stdout is line-buffered when attached to a TTY but block-buffered when piped. The UI could hang waiting for a flush that never comes. | High | Explicitly flush stdout after every event write in `IpcBackend`. Test with an integration test that pipes the core's output to a Rust test harness and asserts each line arrives within a deadline. |
| 3 | **Child-process death isn't detected promptly by the UI.** If the Rust core panics with no `finish`, the UI needs to notice within seconds. | Med | `CoreProcess` reads stdout in a dedicated thread; EOF on stdout (any reason) → emit a synthetic "core_died" event on the channel; UI reacts. Also subscribe to `Process.Exited` event. |
| 4 | **Async UI ↔ subprocess race conditions.** Sending a `cancel` while the core is mid-prompt could cause the core to read garbage if the UI's writer thread isn't drained before close. | Med | Single writer thread for stdin; `cancel` becomes a queued message like any other; close stdin only after the writer thread observes it. |
| 5 | **Protocol versioning + backwards compatibility.** If we ship v1.0.0 and later add a non-optional field, old UIs against new cores break (or vice versa). | Low (we control both) | Treat all field additions as optional with defaults; reserve breaking changes for major bumps; document the compatibility matrix in `docs/ipc-protocol.md`. |
| 6 | **Cross-process debugging is harder than in-process.** "Why did the UI not update?" turns into "which process is at fault?" | Low | Symmetric log file dirs (`%LOCALAPPDATA%\ipod-sync\logs\{ts}.log` and `ui-{ts}.log`). Add a "Show logs" menu item in M4. Provide a `--ipc-mode --tee logs\messages.ndjson` flag in M2+ that mirrors the JSON stream to a file for replay. |
| 7 | **MSIX signing requirements unclear.** Microsoft Store wants different things from sideload installers. EV cert costs $400+/year. | Med | Defer all signing decisions to M4. M1–M3 ship as unpackaged `.exe`. |
| 8 | **CommunityToolkit.Mvvm source generator + .NET 10 RC interactions.** Source generators sometimes lag major .NET releases. | Low | Pin to the latest released CommunityToolkit.Mvvm version; if generators break, fall back to manual `INotifyPropertyChanged` for affected ViewModels. |

---

## Out of scope for Phase 6

- **macOS UI** (SwiftUI) — reuses Phase 6's IPC protocol; separate phase.
- **Linux UI** (GTK 4 / libadwaita) — same.
- **Daemon mode** (Phase 5 territory) — the WinUI app spawns the core on demand; no background service.
- **Smart playlists** — deferred indefinitely (SPEC §7 out-of-scope).
- **Play count writeback** — Phase 5a deferred.
- **Multi-iPod support in the UI** — Phase 4 lands the multi-iPod core; Phase 6 M1 assumes one iPod at a time. UI multi-iPod support could land mid-Phase 6 as M3.5 or post-M4.
- **TUI removal** — the TUI stays as the cross-platform fallback.
- **Auto-update mechanism** — manual download/install for now.
- **Telemetry / crash reporting** — none; local-only tool.

---

## Acceptance criteria

### M1 PASS criteria

1. **Build:** `cargo build --release` succeeds; `dotnet build ui-windows\IpodSync.UI.sln -c Release` succeeds.
2. **Tests:** `cargo test` passes (Rust tests + new IPC backend tests); `dotnet test ui-windows\` passes (protocol round-trip tests + CoreProcess unit tests).
3. **Launch:** Double-click `IpodSync.UI.exe` in `ui-windows\IpodSync.UI\bin\Release\net10.0-windows10.0.19041.0\`. App window opens within 2 seconds, shows the Main page with a "Start sync" button.
4. **Handshake:** Clicking Start spawns `ipod-sync.exe --ipc-mode`. Within 5 seconds, the UI displays the Header info (source, iPod, manifest paths) and the Summary counts.
5. **Review:** The Review page renders with Apply / Dry-run / Quit buttons. The action plan numbers match what the TUI would show for the same library state.
6. **Apply:** Clicking Apply drives the sync to completion. The progress bar updates per `track_start`; the log tail shows each `log` event. The Done dialog appears when `finish` arrives.
7. **Quit:** Clicking Quit (or closing the window) sends `cancel`, waits up to 5s for `finish`, force-kills if needed. No orphan `ipod-sync.exe` processes left running.
8. **Crash recovery:** Force-kill `ipod-sync.exe` from Task Manager mid-sync. The UI shows a crash dialog within 3 seconds with the last few log lines.
9. **Version mismatch:** Manually corrupt the `hello` event's `protocol_version` (e.g. via a build flag). The UI shows a clean error and does not proceed.
10. **No JSON corruption:** Over a 1000-track sync run, no malformed line appears in the stream (verified via `--tee` if implemented, otherwise manually inspecting stderr).

### M2 / M3 / M4 PASS criteria

Defined in their respective plans, written as the milestones approach.
