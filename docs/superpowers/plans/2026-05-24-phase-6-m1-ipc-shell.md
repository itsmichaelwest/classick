# Phase 6 M1: IPC Protocol + Rust `--ipc-mode` + Minimal WinUI Shell

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Tasks marked `(parallel-safe with Task N)` can be dispatched concurrently to independent implementer subagents.
>
> **For C# / WinUI tasks (Tasks 4-8):** Implementer subagents MUST load the `winui3-csharp-app` and `dotnet-csharp` skills before starting. Those skills cover Windows App SDK installation, csproj setup, MVVM patterns, source-generator gotchas, and unpackaged-app project properties. This plan won't recapitulate that material.

**Goal:** Ship a minimal native Windows UI for ipod-sync that drives the existing Rust core via newline-delimited JSON over stdin/stdout. End state: double-click `IpodSync.UI.exe`, see Review screen, press Apply, watch progress bar drive a real sync to a real iPod. No wizard, no library browser, no installer — those land in M2-M4.

**Architecture:** Three layers. (1) A documented IPC protocol (`docs/ipc-protocol.md`) defining every message type with JSON schemas. (2) A new `IpcBackend` in `src/progress.rs` alongside the existing `TuiBackend` and `PlainBackend` — same `ProgressEvent`/`Decision` channels, but the backend serializes events to stdout and deserializes commands from stdin. The orchestrator above doesn't know which backend is active. (3) A WinUI 3 app in a sibling `ui-windows/` directory: spawns `ipod-sync.exe --ipc-mode` as a child process, drives the protocol from C#, renders ViewModels via MVVM.

**Tech Stack:**
- Rust: stable (x86_64-pc-windows-msvc), existing serde + serde_json. No new crate deps.
- C#: .NET 10 SDK, Visual Studio 2022 17.10+, Windows App SDK 1.6+, CommunityToolkit.Mvvm, xUnit.
- IPC: newline-delimited JSON, UTF-8, custom typed-envelope protocol (see spec §"IPC protocol").

**Plan scope (M1 only):** No wizard (M2), no library browser (M3), no MSIX/signing (M4). Existing TUI mode stays unchanged. Path configuration still uses `config.toml` (user pre-runs the TUI once or edits the file). C# side is a single project, no class library split, no DI container.

**Gate (M1 acceptance criteria):** see spec §"M1 PASS criteria". Summary: build both sides, run a real sync end-to-end via the WinUI app, verify quit + crash + version-mismatch error paths.

---

## File Structure

```
F:\repos\ipod-sync\
├── docs\
│   └── ipc-protocol.md                       (new: full protocol spec)
├── src\
│   ├── cli.rs                                (modify: + --ipc-mode flag)
│   ├── progress.rs                           (modify: + IpcBackend + run_ipc)
│   ├── logging.rs                            (modify: + file routing in ipc mode)
│   ├── ipc.rs                                (new: serde records for wire types)
│   ├── lib.rs                                (modify: pub mod ipc)
│   └── main.rs                               (modify: branch on cli.ipc_mode)
├── tests\
│   └── ipc_integration.rs                    (new: spawn core, drive protocol, assert)
├── ui-windows\                               (new sibling dir)
│   ├── README.md                             (build prereqs, dev loop)
│   ├── .gitignore                            (bin/, obj/, .vs/, *.user)
│   ├── IpodSync.UI.sln                       (Visual Studio solution)
│   ├── IpodSync.UI\
│   │   ├── IpodSync.UI.csproj
│   │   ├── App.xaml / App.xaml.cs
│   │   ├── MainWindow.xaml / MainWindow.xaml.cs
│   │   ├── Assets\Square150x150Logo.scale-200.png   (placeholder)
│   │   ├── Views\
│   │   │   ├── ReviewPage.xaml / .cs
│   │   │   └── ProgressPage.xaml / .cs
│   │   ├── ViewModels\
│   │   │   ├── MainViewModel.cs
│   │   │   ├── ReviewViewModel.cs
│   │   │   └── ProgressViewModel.cs
│   │   ├── Services\
│   │   │   ├── ICoreProcess.cs
│   │   │   ├── CoreProcess.cs
│   │   │   └── CoreLocator.cs
│   │   └── Models\
│   │       ├── Events.cs
│   │       ├── Commands.cs
│   │       └── ProtocolVersion.cs
│   └── IpodSync.UI.Tests\
│       ├── IpodSync.UI.Tests.csproj
│       ├── ProtocolSerializationTests.cs
│       ├── FakeCoreProcessTests.cs
│       └── CoreLocatorTests.cs
└── LEARNINGS.md                              (modify: M1 result + decisions)
```

### Module responsibility delta

- **`docs/ipc-protocol.md`** (new) — the contract. Every wire message: discriminator, fields, types, JSON example, direction (core→UI or UI→core). Versioning rules. Process lifecycle. This file is the source of truth; both Rust and C# code reference it.
- **`cli`** — adds `--ipc-mode` boolean flag. Mutually exclusive with `--no-tui` at parse time.
- **`ipc`** (new) — wire-type definitions. Serde-tagged enums for events and commands, separate from `ProgressEvent`/`Decision` so the wire format is decoupled from the internal one (lets us evolve either independently). Module also owns helpers to serialize an event to a line and to parse a command line.
- **`progress`** — `Progress::start` signature grows an `ipc_mode: bool` parameter. Dispatches to new `run_ipc(event_rx, decision_tx)` when `ipc_mode == true`. `run_ipc` writes events to stdout as JSON lines (with explicit flush), spawns a stdin-reader thread that feeds commands back into the decision channel.
- **`logging`** — `init` gains an `ipc_mode: bool` parameter. When true, routes tracing to `%LOCALAPPDATA%\ipod-sync\logs\<timestamp>.log` instead of stderr. stdout MUST be reserved for the JSON stream.
- **`main`** — single new line: passes `cli.ipc_mode` through to `logging::init` and `Progress::start`. Adjusts `use_tui` computation.
- **`tests/ipc_integration.rs`** (new) — spawns `ipod-sync.exe --ipc-mode` as a subprocess, reads the `hello` event, asserts protocol_version, sends a `cancel`, asserts the process exits within 5s. Doesn't run a full sync (would need an iPod); validates protocol plumbing.
- **`ui-windows/`** (new) — entire C# project tree. See above. Independent of Cargo.

---

## Task 1: Design + document the IPC protocol (parallel-safe with Task 4 only — Tasks 2/3/5+ depend on this contract being settled)

**Files:**
- New: `F:\repos\ipod-sync\docs\ipc-protocol.md`

This task is mostly documentation. It produces the contract that Tasks 2 (Rust IpcBackend) and 5 (C# CoreProcess) implement against. Lock down field names, types, and direction before any code lands.

- [ ] **Step 1: Write the protocol document**

Create `docs/ipc-protocol.md` with these sections, modeled on the spec's "IPC protocol" section but expanded with full schemas and rationale notes:

1. **Wire format** — UTF-8, NDJSON, one message per line, no embedded newlines in field values, no pretty-printing.
2. **Discriminator field** — every message has `"type"` as the first field (by convention; serde doesn't guarantee field order but UI parsers can search). All `type` values are `lower_snake_case`.
3. **Direction A: events (core → UI)** — table with one row per event type. Columns: `type`, fields (name + JSON type + required/optional), purpose. Then a complete JSON example for each.
4. **Direction B: commands (UI → core)** — same shape.
5. **Correlation** — id pairing for prompt/form.
6. **Versioning** — protocol_version string in the `hello` event, semver rules, major-bump-on-breaking, additive events ignorable by older UIs.
7. **Error model** — non-fatal `error` events accumulate; fatal errors precede `finish { success: false }`.
8. **Process lifecycle** — handshake order, graceful shutdown (5s timeout), force-kill behavior.
9. **Implementation hints (informative, not normative)** — Rust serde tags + C# System.Text.Json polymorphism patterns.

Copy the schemas and JSON examples from `docs/superpowers/specs/2026-05-24-phase-6-winui-app.md` §"IPC protocol" — this doc is the deeper version of those tables.

Add a compatibility matrix table at the end:

```
| Protocol | Core version | UI version | Status      |
|----------|--------------|------------|-------------|
| 1.0.0    | 0.1.x        | 0.1.x      | Initial M1  |
```

- [ ] **Step 2: Build (sanity — no Rust changes, just docs)**

```powershell
cargo build 2>&1 | Select-Object -Last 3
```

- [ ] **Step 3: Commit**

```powershell
git -C F:\repos\ipod-sync add docs\ipc-protocol.md
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
docs(ipc): formal IPC protocol spec for Phase 6

NDJSON over stdin/stdout. UTF-8, one JSON object per line. Custom
typed-envelope (not JSON-RPC 2.0) keyed by 'type' discriminator.

Direction A (core -> UI): hello, header, summary, review, prompt, form,
track_start, track_done, log, error, finish.

Direction B (UI -> core): start (reserved), review_decision,
prompt_decision, form_decision, cancel.

Prompt/form correlation via 'id'. Versioning via protocol_version semver
in the 'hello' handshake. Bounded graceful shutdown (5s) on cancel,
force-kill otherwise. Compatibility matrix included.
EOF
)"
```

---

## Task 2: Rust wire types module `src/ipc.rs` (parallel-safe with Task 4)

**Files:**
- New: `F:\repos\ipod-sync\src\ipc.rs`
- Modify: `F:\repos\ipod-sync\src\lib.rs` (re-export)

Defines the serde-tagged enums for events and commands. Decoupled from `ProgressEvent`/`Decision` (Rust internal types) so the wire format and the internal channel format can evolve independently. Conversion happens in `run_ipc` (Task 3).

- [ ] **Step 1: Write the failing tests first**

Create `src/ipc.rs` skeleton with tests at the bottom:

```rust
//! IPC wire types — serde-tagged enums for the JSON protocol over stdin/stdout.
//!
//! These types are intentionally separate from `ProgressEvent` / `Decision` so
//! that the on-the-wire format and the internal channel format can evolve
//! independently. See `docs/ipc-protocol.md` for the full contract.

use serde::{Deserialize, Serialize};

/// Current protocol version emitted in the `hello` handshake.
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// Events the core emits to the UI. One JSON object per line on stdout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcEvent {
    Hello {
        protocol_version: String,
        core_version: String,
    },
    Header {
        source: String,
        ipod: String,
        manifest: String,
    },
    Summary {
        add: u32,
        modify: u32,
        metadata_only: u32,
        remove: u32,
        unchanged: u32,
        total_planned: u32,
    },
    Review {
        summary: IpcActionPlanSummary,
        no_delete: bool,
    },
    Prompt {
        id: u64,
        message: String,
        options: Vec<String>,
    },
    Form {
        id: u64,
        label: String,
        initial: String,
        hint: String,
    },
    TrackStart {
        current: u32,
        total: u32,
        label: String,
    },
    TrackDone,
    Log {
        message: String,
    },
    Error {
        message: String,
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        recovery_hints: Vec<String>,
    },
    Finish {
        success: bool,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpcActionPlanSummary {
    pub add: u32,
    pub modify: u32,
    pub metadata_only: u32,
    pub remove: u32,
    pub unchanged: u32,
}

/// Commands the UI sends to the core. One JSON object per line on stdin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcCommand {
    /// Reserved for M2+; M1 implicit on spawn.
    Start,
    ReviewDecision {
        choice: ReviewChoice,
        no_delete: bool,
    },
    PromptDecision {
        id: u64,
        choice: u32,
    },
    FormDecision {
        id: u64,
        /// `None` (JSON null) means the user aborted (Esc / Ctrl+C).
        value: Option<String>,
    },
    Cancel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewChoice {
    Apply,
    DryRun,
    Quit,
}

/// Serialize an event to a single-line JSON string (no trailing newline).
/// Caller is responsible for writing the line + flushing.
pub fn serialize_event(event: &IpcEvent) -> serde_json::Result<String> {
    serde_json::to_string(event)
}

/// Parse a single-line JSON string into a command. Whitespace-trims the input.
pub fn parse_command(line: &str) -> serde_json::Result<IpcCommand> {
    serde_json::from_str(line.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_semver_one_dot_zero_dot_zero() {
        assert_eq!(PROTOCOL_VERSION, "1.0.0");
    }

    #[test]
    fn hello_event_roundtrip() {
        let evt = IpcEvent::Hello {
            protocol_version: "1.0.0".to_string(),
            core_version: "0.0.1".to_string(),
        };
        let json = serialize_event(&evt).unwrap();
        assert!(json.contains(r#""type":"hello""#));
        assert!(json.contains(r#""protocol_version":"1.0.0""#));
        let parsed: IpcEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, evt);
    }

    #[test]
    fn summary_event_serializes_expected_fields() {
        let evt = IpcEvent::Summary {
            add: 12, modify: 3, metadata_only: 0, remove: 0, unchanged: 1260, total_planned: 15,
        };
        let json = serialize_event(&evt).unwrap();
        assert!(json.contains(r#""type":"summary""#));
        assert!(json.contains(r#""add":12"#));
        assert!(json.contains(r#""total_planned":15"#));
    }

    #[test]
    fn prompt_event_roundtrip_with_options() {
        let evt = IpcEvent::Prompt {
            id: 7,
            message: "Retry?".to_string(),
            options: vec!["Retry".to_string(), "Skip".to_string(), "Abort".to_string()],
        };
        let json = serialize_event(&evt).unwrap();
        assert_eq!(serde_json::from_str::<IpcEvent>(&json).unwrap(), evt);
    }

    #[test]
    fn track_done_event_serializes_as_just_type() {
        let evt = IpcEvent::TrackDone;
        let json = serialize_event(&evt).unwrap();
        assert_eq!(json, r#"{"type":"track_done"}"#);
    }

    #[test]
    fn error_event_omits_recovery_hints_when_empty() {
        let evt = IpcEvent::Error {
            message: "boom".to_string(),
            recovery_hints: vec![],
        };
        let json = serialize_event(&evt).unwrap();
        assert!(!json.contains("recovery_hints"));
    }

    #[test]
    fn error_event_includes_recovery_hints_when_set() {
        let evt = IpcEvent::Error {
            message: "boom".to_string(),
            recovery_hints: vec!["try X".to_string(), "try Y".to_string()],
        };
        let json = serialize_event(&evt).unwrap();
        assert!(json.contains(r#""recovery_hints":["try X","try Y"]"#));
    }

    #[test]
    fn review_decision_command_parses() {
        let line = r#"{"type":"review_decision","choice":"apply","no_delete":false}"#;
        let cmd = parse_command(line).unwrap();
        assert_eq!(cmd, IpcCommand::ReviewDecision {
            choice: ReviewChoice::Apply,
            no_delete: false,
        });
    }

    #[test]
    fn form_decision_command_accepts_null_value() {
        let line = r#"{"type":"form_decision","id":1,"value":null}"#;
        let cmd = parse_command(line).unwrap();
        assert_eq!(cmd, IpcCommand::FormDecision { id: 1, value: None });
    }

    #[test]
    fn form_decision_command_accepts_string_value() {
        let line = r#"{"type":"form_decision","id":1,"value":"D:\\music"}"#;
        let cmd = parse_command(line).unwrap();
        assert_eq!(cmd, IpcCommand::FormDecision {
            id: 1,
            value: Some(r"D:\music".to_string()),
        });
    }

    #[test]
    fn cancel_command_parses() {
        let cmd = parse_command(r#"{"type":"cancel"}"#).unwrap();
        assert_eq!(cmd, IpcCommand::Cancel);
    }

    #[test]
    fn unknown_command_type_errors() {
        let result = parse_command(r#"{"type":"nuke_from_orbit"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn malformed_json_errors() {
        let result = parse_command("not json");
        assert!(result.is_err());
    }

    #[test]
    fn parse_command_trims_whitespace_and_trailing_newline() {
        let line = "  {\"type\":\"cancel\"}\r\n";
        let cmd = parse_command(line).unwrap();
        assert_eq!(cmd, IpcCommand::Cancel);
    }
}
```

- [ ] **Step 2: Add `pub mod ipc;` to `src/lib.rs`**

In alphabetical order (likely between `ipod` and `logging` — check existing order and place accordingly).

- [ ] **Step 3: Build + test**

```powershell
cd F:\repos\ipod-sync
cargo build 2>&1 | Select-Object -Last 5
cargo test ipc:: 2>&1 | Select-Object -Last 20
```

Expected: clean build, all 13 ipc tests pass.

- [ ] **Step 4: Commit**

```powershell
git -C F:\repos\ipod-sync add src\ipc.rs src\lib.rs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(ipc): wire-type module with serde-tagged event + command enums

Defines IpcEvent (hello, header, summary, review, prompt, form,
track_start, track_done, log, error, finish) and IpcCommand
(start, review_decision, prompt_decision, form_decision, cancel)
with serde's tag = "type", rename_all = "snake_case". Separate
from internal ProgressEvent/Decision so wire and internal formats
evolve independently. PROTOCOL_VERSION constant = "1.0.0".

13 round-trip + edge-case tests cover: hello serialization,
unit-variant compact form (track_done), recovery_hints skipped when
empty, form_decision null vs string value, unknown-type rejection,
whitespace-tolerant parsing.
EOF
)"
```

---

## Task 3: `--ipc-mode` CLI flag + `IpcBackend` in `progress.rs` + `logging` file routing (depends on Task 2)

**Files:**
- Modify: `F:\repos\ipod-sync\src\cli.rs`
- Modify: `F:\repos\ipod-sync\src\progress.rs`
- Modify: `F:\repos\ipod-sync\src\logging.rs`
- Modify: `F:\repos\ipod-sync\src\main.rs`

The big Rust task. Adds the CLI flag, wires it through main, adds the new backend, routes tracing to a file when IPC mode is active.

- [ ] **Step 1: Add the `--ipc-mode` flag to `Cli`**

In `src/cli.rs`, after `pub no_tui: bool` (around line 89):

```rust
    /// Speak JSON-over-stdio instead of rendering a TUI. Used by the WinUI
    /// frontend (and any future native UI). Disables the TUI; routes tracing
    /// to a file under %LOCALAPPDATA%\ipod-sync\logs\.
    /// Conflicts with --no-tui.
    #[arg(long, conflicts_with = "no_tui")]
    pub ipc_mode: bool,
```

Add a test:
```rust
    #[test]
    fn parses_ipc_mode_flag() {
        let cli = Cli::try_parse_from(["ipod-sync", "--ipc-mode"]).unwrap();
        assert!(cli.ipc_mode);
    }

    #[test]
    fn ipc_mode_conflicts_with_no_tui() {
        let result = Cli::try_parse_from(["ipod-sync", "--ipc-mode", "--no-tui"]);
        assert!(result.is_err(), "clap should reject the combo");
    }

    #[test]
    fn ipc_mode_default_false() {
        let cli = Cli::try_parse_from(["ipod-sync"]).unwrap();
        assert!(!cli.ipc_mode);
    }
```

Update the existing `parses_no_args_with_defaults` test to assert `assert!(!cli.ipc_mode);`.

- [ ] **Step 2: Extend `logging::init` to accept `ipc_mode`**

In `src/logging.rs`:

```rust
pub fn init(verbose: bool, use_tui: bool, ipc_mode: bool) {
    let default = if verbose { "ipod_sync=debug,info" } else { "ipod_sync=info,warn" };
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact();

    if ipc_mode {
        // stdout is reserved for the JSON event stream. Route tracing to a
        // timestamped file under %LOCALAPPDATA%\ipod-sync\logs\.
        match open_ipc_log_file() {
            Ok(file) => {
                use std::sync::Mutex;
                builder.with_writer(Mutex::new(file)).init();
            }
            Err(e) => {
                // Last-ditch fallback: sink. Don't emit on stderr in case the
                // parent process is reading it.
                eprintln!("ipod-sync: could not open IPC log file: {e}");
                builder.with_writer(std::io::sink).init();
            }
        }
    } else if use_tui {
        builder.with_writer(std::io::sink).init();
    } else {
        builder.init();
    }

    install_glib_handler();
    debug!("logging initialized (verbose={verbose}, use_tui={use_tui}, ipc_mode={ipc_mode})");
}

fn open_ipc_log_file() -> std::io::Result<std::fs::File> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no LOCALAPPDATA"))?
        .join("ipod-sync")
        .join("logs");
    std::fs::create_dir_all(&base)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = base.join(format!("core-{ts}.log"));
    std::fs::File::create(path)
}
```

- [ ] **Step 3: Add `IpcBackend` dispatch in `Progress::start`**

In `src/progress.rs`, update the signature:

```rust
pub fn start(use_tui: bool, ipc_mode: bool) -> Result<(Self, Receiver<Decision>)> {
    let (event_tx, event_rx) = mpsc::channel();
    let (decision_tx, decision_rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        if ipc_mode {
            if let Err(e) = run_ipc(event_rx, decision_tx) {
                // Can't println — would corrupt the stream. Trace to file.
                tracing::error!("IPC backend failure: {e}");
            }
        } else {
            let is_tty = std::io::stdout().is_terminal();
            let active_tui = use_tui && is_tty;
            if active_tui {
                if let Err(e) = run_tui(event_rx, decision_tx) {
                    eprintln!("TUI failure: {e}; falling back to plain mode is not possible mid-run");
                }
            } else {
                run_plain(event_rx, decision_tx);
            }
        }
    });
    Ok((
        Self { sender: event_tx, thread: Some(thread) },
        decision_rx,
    ))
}
```

- [ ] **Step 4: Implement `run_ipc`**

In `src/progress.rs`, add the new function (near `run_plain`):

```rust
/// JSON-over-stdio backend. Each ProgressEvent is serialized to a single line
/// of JSON on stdout (with explicit flush). A reader thread parses commands
/// from stdin and feeds them back through `decision_tx` as Decisions.
///
/// stdout MUST stay clean — no println outside the IPC stream. Tracing goes
/// to a file (see logging::init). Errors here must not surface on stdout.
fn run_ipc(event_rx: Receiver<ProgressEvent>, decision_tx: Sender<Decision>) -> Result<()> {
    use crate::ipc::{
        parse_command, serialize_event, IpcActionPlanSummary, IpcCommand, IpcEvent,
        ReviewChoice, PROTOCOL_VERSION,
    };
    use std::io::{BufRead, Write};

    // 1. Send the handshake immediately so the UI knows we're alive.
    let hello = IpcEvent::Hello {
        protocol_version: PROTOCOL_VERSION.to_string(),
        core_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    write_event(&hello)?;

    // 2. Spawn the stdin reader thread.
    let decision_tx_for_reader = decision_tx.clone();
    let reader_thread = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut locked = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            match locked.read_line(&mut line) {
                Ok(0) => break, // EOF (parent closed our stdin)
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match parse_command(trimmed) {
                        Ok(cmd) => {
                            let decision = match cmd {
                                IpcCommand::Start => continue, // M1: no-op
                                IpcCommand::ReviewDecision { choice, no_delete } => {
                                    let rd = match choice {
                                        ReviewChoice::Apply => ReviewDecision::Apply { no_delete },
                                        ReviewChoice::DryRun => ReviewDecision::DryRun,
                                        ReviewChoice::Quit => ReviewDecision::Quit,
                                    };
                                    Decision::Review(rd)
                                }
                                IpcCommand::PromptDecision { id, choice } => {
                                    Decision::Prompt { id, choice: choice as usize }
                                }
                                IpcCommand::FormDecision { id, value } => {
                                    Decision::Form { id, value }
                                }
                                IpcCommand::Cancel => {
                                    // Map to Review(Quit) so the orchestrator's existing
                                    // Quit path runs (clean teardown, no manifest write).
                                    // Mid-sync Cancel is best-effort in M1; M2+ can plumb
                                    // a dedicated cancel channel through the orchestrator.
                                    Decision::Review(ReviewDecision::Quit)
                                }
                            };
                            if decision_tx_for_reader.send(decision).is_err() {
                                break; // main thread gone
                            }
                        }
                        Err(e) => {
                            tracing::warn!("ipc: malformed command {trimmed:?}: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("ipc: stdin read error: {e}");
                    break;
                }
            }
        }
    });

    // 3. Main event loop: pull from event_rx, translate to IpcEvent, write line.
    loop {
        match event_rx.recv() {
            Ok(event) => {
                let ipc_event = progress_event_to_ipc(event);
                let is_finish = matches!(ipc_event, IpcEvent::Finish { .. });
                write_event(&ipc_event)?;
                if is_finish {
                    break;
                }
            }
            Err(_) => break, // sender dropped
        }
    }

    // 4. Drop our handle to the reader thread; it'll exit on stdin EOF when
    //    the parent closes us. Don't join — could block indefinitely if the
    //    parent doesn't close stdin promptly. Progress::finish has its own
    //    bounded join deadline that covers this.
    drop(reader_thread);
    Ok(())
}

/// Write a single event to stdout as a JSON line. Flushes immediately —
/// Rust's stdout is block-buffered when piped, and an unflushed write would
/// keep the UI waiting.
fn write_event(event: &crate::ipc::IpcEvent) -> Result<()> {
    use std::io::Write;
    let line = crate::ipc::serialize_event(event)
        .map_err(|e| anyhow!("ipc: serialize failed: {e}"))?;
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();
    writeln!(locked, "{line}").map_err(|e| anyhow!("ipc: stdout write failed: {e}"))?;
    locked.flush().map_err(|e| anyhow!("ipc: stdout flush failed: {e}"))?;
    Ok(())
}

/// Translate an internal `ProgressEvent` into an `IpcEvent`. Lossless for M1.
fn progress_event_to_ipc(event: ProgressEvent) -> crate::ipc::IpcEvent {
    use crate::ipc::{IpcActionPlanSummary, IpcEvent};
    match event {
        ProgressEvent::Header { source, ipod, manifest } => {
            IpcEvent::Header { source, ipod, manifest }
        }
        ProgressEvent::Summary { add, modify, remove, unchanged, total_planned } => {
            IpcEvent::Summary {
                add: add as u32,
                modify: modify as u32,
                metadata_only: 0, // existing Summary variant doesn't carry this; widen in a follow-up
                remove: remove as u32,
                unchanged: unchanged as u32,
                total_planned: total_planned as u32,
            }
        }
        ProgressEvent::Review { summary, no_delete } => {
            IpcEvent::Review {
                summary: IpcActionPlanSummary {
                    add: summary.add as u32,
                    modify: summary.modify as u32,
                    metadata_only: summary.metadata_only as u32,
                    remove: summary.remove as u32,
                    unchanged: summary.unchanged as u32,
                },
                no_delete,
            }
        }
        ProgressEvent::Prompt(req) => IpcEvent::Prompt {
            id: req.id,
            message: req.message,
            options: req.options,
        },
        ProgressEvent::Form(req) => IpcEvent::Form {
            id: req.id,
            label: req.label,
            initial: req.initial,
            hint: req.hint,
        },
        ProgressEvent::TrackStart { current, total, label } => IpcEvent::TrackStart {
            current: current as u32,
            total: total as u32,
            label,
        },
        ProgressEvent::TrackDone => IpcEvent::TrackDone,
        ProgressEvent::Log(message) => IpcEvent::Log { message },
        ProgressEvent::Error(message) => IpcEvent::Error {
            message,
            recovery_hints: vec![],
        },
        ProgressEvent::Finish => IpcEvent::Finish { success: true },
    }
}
```

**NOTE on the `Summary { metadata_only: 0 }` shortcut:** the existing `ProgressEvent::Summary` doesn't carry `metadata_only`. Don't widen it here to keep this task focused on IPC plumbing; the `Review` event DOES carry it via `ActionPlanSummary`, which is what the UI actually uses to render the action plan. Document this in the inline comment. Fix in a follow-up task post-M1 if it matters.

**NOTE on `Cancel`:** mapping to `Decision::Review(Quit)` only works if the orchestrator is currently awaiting the Review decision. Mid-sync Cancel is best-effort in M1 — the orchestrator doesn't currently support cooperative cancellation. The UI's `CoreProcess` is responsible for force-killing after the 5s grace period (see Task 5). Document this limitation.

- [ ] **Step 5: Update `main.rs` to pass `ipc_mode`**

```rust
fn main() -> Result<()> {
    unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE")); }

    let cli = Cli::parse();

    // Three-way mode decision:
    //   --ipc-mode → IpcBackend (priority over everything else)
    //   --no-tui or no TTY → PlainBackend
    //   else → TuiBackend
    let ipc_mode = cli.ipc_mode;
    let use_tui = !ipc_mode && !cli.no_tui && std::io::stdout().is_terminal();

    ipod_sync::logging::init(cli.verbose, use_tui, ipc_mode);

    let (progress, decision_rx) = Progress::start(use_tui, ipc_mode)?;

    let result = orchestrator::orchestrate(cli, &progress, &decision_rx);
    let finish_result = progress.finish();

    result.and(finish_result)
}
```

- [ ] **Step 6: Build + test**

```powershell
cargo build 2>&1 | Select-Object -Last 5
cargo test 2>&1 | Select-Object -Last 10
```

Expected: clean build, all existing tests + 3 new CLI tests + 13 ipc tests pass.

- [ ] **Step 7: Manual smoke test**

```powershell
# Spawn the core in ipc mode, capture its first line.
$proc = Start-Process -FilePath "F:\repos\ipod-sync\target\debug\ipod-sync.exe" `
    -ArgumentList "--ipc-mode" `
    -RedirectStandardInput "stdin.txt" `
    -RedirectStandardOutput "stdout.txt" `
    -RedirectStandardError "stderr.txt" `
    -PassThru -NoNewWindow
Start-Sleep -Seconds 2
Stop-Process -Id $proc.Id -Force
Get-Content stdout.txt | Select-Object -First 1
# Expected: a line like {"type":"hello","protocol_version":"1.0.0","core_version":"0.0.1"}
```

Don't commit `stdout.txt`/`stderr.txt`/`stdin.txt` — they're scratch files. Clean up after.

- [ ] **Step 8: Commit**

```powershell
git -C F:\repos\ipod-sync add src\cli.rs src\progress.rs src\logging.rs src\main.rs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(progress, cli, logging): --ipc-mode JSON-over-stdio backend

New --ipc-mode flag (conflicts with --no-tui) routes Progress through
an IpcBackend alongside the existing TuiBackend and PlainBackend. The
backend serializes each ProgressEvent to a JSON line on stdout (with
explicit flush) and parses IpcCommand lines from stdin into the existing
Decision channel. First emitted event is the protocol handshake:
{"type":"hello","protocol_version":"1.0.0","core_version":"..."}.

logging::init now takes ipc_mode; when true, tracing routes to a
%LOCALAPPDATA%\ipod-sync\logs\core-{ts}.log file so stdout stays clean
for the JSON stream.

M1 limitations documented inline:
- Cancel command maps to Decision::Review(Quit) — works at Review time;
  mid-sync cancel is best-effort (UI force-kills after 5s grace).
- progress_event_to_ipc emits metadata_only=0 in Summary; the Review
  event carries the real value via ActionPlanSummary which is what the
  UI actually renders. Widening Summary is a follow-up.
EOF
)"
```

---

## Task 4: Bootstrap `ui-windows/` C# solution (parallel-safe with Tasks 1, 2)

**Files:**
- New: `F:\repos\ipod-sync\ui-windows\README.md`
- New: `F:\repos\ipod-sync\ui-windows\.gitignore`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.sln`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\IpodSync.UI.csproj`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\App.xaml`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\App.xaml.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\MainWindow.xaml`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\MainWindow.xaml.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Assets\Square150x150Logo.scale-200.png` (placeholder; can be a 1x1 png if needed)
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\SmokeTest.cs` (one trivial passing xUnit test to prove the test project runs)

Sub-skills required: `winui3-csharp-app`, `dotnet-csharp`. The implementer should load both before starting; they cover the csproj boilerplate, Windows App SDK NuGet refs, the Application target, and unpackaged-app properties this task won't repeat.

- [ ] **Step 1: Create the solution and project structure**

From `F:\repos\ipod-sync\ui-windows\` (create the directory first):

```powershell
mkdir F:\repos\ipod-sync\ui-windows
cd F:\repos\ipod-sync\ui-windows
dotnet new sln -n IpodSync.UI
```

Create `IpodSync.UI\IpodSync.UI.csproj` manually (the `winui3-csharp-app` skill's template should be the starting point). Key requirements:

- `<TargetFramework>net10.0-windows10.0.19041.0</TargetFramework>`
- `<OutputType>WinExe</OutputType>`
- `<UseWinUI>true</UseWinUI>`
- `<WindowsPackageType>None</WindowsPackageType>` (unpackaged)
- `<Nullable>enable</Nullable>`
- `<LangVersion>latest</LangVersion>`
- PackageReferences:
  - `Microsoft.WindowsAppSDK` (latest stable, 1.6+)
  - `Microsoft.Windows.SDK.BuildTools` (matching version)
  - `CommunityToolkit.Mvvm` (latest stable, 8.x)

Test project `IpodSync.UI.Tests\IpodSync.UI.Tests.csproj`:
- `<TargetFramework>net10.0-windows10.0.19041.0</TargetFramework>` (matches main project so it can reference WinUI types if needed; otherwise plain `net10.0` is fine and slightly faster)
- PackageReferences: `xunit`, `xunit.runner.visualstudio`, `Microsoft.NET.Test.Sdk`
- ProjectReference to `..\IpodSync.UI\IpodSync.UI.csproj`

Add both projects to the solution:
```powershell
dotnet sln add IpodSync.UI\IpodSync.UI.csproj
dotnet sln add IpodSync.UI.Tests\IpodSync.UI.Tests.csproj
```

- [ ] **Step 2: Minimal App.xaml + App.xaml.cs**

`App.xaml`:
```xml
<Application
    x:Class="IpodSync.UI.App"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
    <Application.Resources>
        <ResourceDictionary>
            <ResourceDictionary.MergedDictionaries>
                <XamlControlsResources xmlns="using:Microsoft.UI.Xaml.Controls" />
            </ResourceDictionary.MergedDictionaries>
        </ResourceDictionary>
    </Application.Resources>
</Application>
```

`App.xaml.cs`:
```csharp
using Microsoft.UI.Xaml;

namespace IpodSync.UI;

public partial class App : Application
{
    private Window? _window;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _window = new MainWindow();
        _window.Activate();
    }
}
```

- [ ] **Step 3: Minimal MainWindow.xaml + MainWindow.xaml.cs**

`MainWindow.xaml`:
```xml
<Window
    x:Class="IpodSync.UI.MainWindow"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    Title="ipod-sync">
    <Grid Padding="24" RowDefinitions="Auto,*">
        <StackPanel Grid.Row="0" Spacing="12">
            <TextBlock Text="ipod-sync" FontSize="32" FontWeight="SemiBold" />
            <TextBlock Text="Native Windows UI for ipod-sync — Phase 6 M1" Opacity="0.7" />
            <Button x:Name="StartButton" Content="Start sync" Click="OnStartClick" />
            <TextBlock x:Name="StatusText" Text="Idle." Margin="0,12,0,0" />
        </StackPanel>
    </Grid>
</Window>
```

`MainWindow.xaml.cs`:
```csharp
using Microsoft.UI.Xaml;

namespace IpodSync.UI;

public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
    }

    private void OnStartClick(object sender, RoutedEventArgs e)
    {
        StatusText.Text = "Start clicked. (CoreProcess wiring in Task 5.)";
    }
}
```

- [ ] **Step 4: Smoke test**

`IpodSync.UI.Tests\SmokeTest.cs`:
```csharp
namespace IpodSync.UI.Tests;

public class SmokeTest
{
    [Fact]
    public void TestProjectRuns()
    {
        Assert.Equal(2, 1 + 1);
    }
}
```

- [ ] **Step 5: README**

`ui-windows\README.md`:
```markdown
# ipod-sync Windows UI

Native WinUI 3 frontend for ipod-sync. Talks to the Rust core via JSON over stdin/stdout — see `..\docs\ipc-protocol.md`.

## Prerequisites

- Windows 10 19041+ or Windows 11
- Visual Studio 2022 17.10+ with the **Windows App SDK C# Templates** workload, OR
- .NET 10 SDK + Windows App SDK from <https://aka.ms/windowsappsdk>

## Build

```powershell
dotnet build IpodSync.UI.sln -c Release
```

## Run (dev loop)

1. Build the Rust core first: `cargo build --release` in the repo root.
2. The UI's `CoreLocator` looks for `ipod-sync.exe` in this order:
   - Sibling to `IpodSync.UI.exe` (production install)
   - `..\..\target\release\ipod-sync.exe` (dev / running from `bin\Release`)
   - `..\..\target\debug\ipod-sync.exe` (dev / debug builds)
   - On PATH
3. Run the app: `dotnet run --project IpodSync.UI -c Release`, or hit F5 in Visual Studio.

## Test

```powershell
dotnet test IpodSync.UI.sln
```

## Project layout

- `IpodSync.UI\` — WinUI 3 app
  - `Views\` — XAML pages
  - `ViewModels\` — MVVM via CommunityToolkit.Mvvm
  - `Services\` — `CoreProcess` (IPC client), `CoreLocator`
  - `Models\` — wire-type records (Events, Commands)
- `IpodSync.UI.Tests\` — xUnit tests
```

- [ ] **Step 6: `.gitignore`**

```gitignore
bin/
obj/
.vs/
*.user
*.suo
```

- [ ] **Step 7: Build + test**

```powershell
cd F:\repos\ipod-sync\ui-windows
dotnet build IpodSync.UI.sln -c Release 2>&1 | Select-Object -Last 10
dotnet test IpodSync.UI.sln 2>&1 | Select-Object -Last 10
```

Expected: clean build, 1 smoke test passes.

- [ ] **Step 8: Manual launch check**

```powershell
dotnet run --project F:\repos\ipod-sync\ui-windows\IpodSync.UI -c Release
```

Expected: an `ipod-sync` window appears, shows the title, the "Native Windows UI..." line, a "Start sync" button. Clicking it updates StatusText to "Start clicked. (CoreProcess wiring in Task 5.)". Close the window — process exits cleanly.

- [ ] **Step 9: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows\README.md ui-windows\.gitignore ui-windows\IpodSync.UI.sln ui-windows\IpodSync.UI\IpodSync.UI.csproj ui-windows\IpodSync.UI\App.xaml ui-windows\IpodSync.UI\App.xaml.cs ui-windows\IpodSync.UI\MainWindow.xaml ui-windows\IpodSync.UI\MainWindow.xaml.cs ui-windows\IpodSync.UI\Assets\Square150x150Logo.scale-200.png ui-windows\IpodSync.UI.Tests\IpodSync.UI.Tests.csproj ui-windows\IpodSync.UI.Tests\SmokeTest.cs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(ui-windows): bootstrap WinUI 3 solution (Phase 6 M1)

Sibling ui-windows/ directory with:
- IpodSync.UI.sln (.NET 10 + Windows App SDK 1.6+, unpackaged)
- IpodSync.UI/ — main app: App.xaml, MainWindow.xaml with a Start button
- IpodSync.UI.Tests/ — xUnit test project with a smoke test

README documents prereqs (VS 2022 17.10+, .NET 10 SDK, Windows App SDK)
and the dev loop. .gitignore excludes bin/obj/.vs/.

No IPC wiring yet — the Start button is a placeholder that updates a
status text; CoreProcess + ViewModels land in Tasks 5-7.
EOF
)"
```

---

## Task 5: C# IPC client `CoreProcess` + `CoreLocator` + wire-type records + unit tests (depends on Tasks 1, 4)

**Files:**
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Models\Events.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Models\Commands.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Models\ProtocolVersion.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Services\ICoreProcess.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Services\CoreProcess.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Services\CoreLocator.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\ProtocolSerializationTests.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\FakeCoreProcessTests.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI.Tests\CoreLocatorTests.cs`

Sub-skill: `dotnet-csharp` for the System.Text.Json polymorphism and System.Threading.Channels patterns.

- [ ] **Step 1: Wire-type records (`Models\Events.cs`)**

```csharp
using System.Text.Json.Serialization;

namespace IpodSync.UI.Models;

[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(HelloEvent), "hello")]
[JsonDerivedType(typeof(HeaderEvent), "header")]
[JsonDerivedType(typeof(SummaryEvent), "summary")]
[JsonDerivedType(typeof(ReviewEvent), "review")]
[JsonDerivedType(typeof(PromptEvent), "prompt")]
[JsonDerivedType(typeof(FormEvent), "form")]
[JsonDerivedType(typeof(TrackStartEvent), "track_start")]
[JsonDerivedType(typeof(TrackDoneEvent), "track_done")]
[JsonDerivedType(typeof(LogEvent), "log")]
[JsonDerivedType(typeof(ErrorEvent), "error")]
[JsonDerivedType(typeof(FinishEvent), "finish")]
public abstract record IpcEvent;

public sealed record HelloEvent(
    [property: JsonPropertyName("protocol_version")] string ProtocolVersion,
    [property: JsonPropertyName("core_version")] string CoreVersion
) : IpcEvent;

public sealed record HeaderEvent(string Source, string Ipod, string Manifest) : IpcEvent;

public sealed record SummaryEvent(
    uint Add, uint Modify,
    [property: JsonPropertyName("metadata_only")] uint MetadataOnly,
    uint Remove, uint Unchanged,
    [property: JsonPropertyName("total_planned")] uint TotalPlanned
) : IpcEvent;

public sealed record ActionPlanSummary(uint Add, uint Modify,
    [property: JsonPropertyName("metadata_only")] uint MetadataOnly,
    uint Remove, uint Unchanged);

public sealed record ReviewEvent(
    ActionPlanSummary Summary,
    [property: JsonPropertyName("no_delete")] bool NoDelete
) : IpcEvent;

public sealed record PromptEvent(ulong Id, string Message, IReadOnlyList<string> Options) : IpcEvent;

public sealed record FormEvent(ulong Id, string Label, string Initial, string Hint) : IpcEvent;

public sealed record TrackStartEvent(uint Current, uint Total, string Label) : IpcEvent;

public sealed record TrackDoneEvent : IpcEvent;

public sealed record LogEvent(string Message) : IpcEvent;

public sealed record ErrorEvent(
    string Message,
    [property: JsonPropertyName("recovery_hints")] IReadOnlyList<string>? RecoveryHints = null
) : IpcEvent;

public sealed record FinishEvent(bool Success) : IpcEvent;
```

Note: `System.Text.Json` uses PascalCase property names by default; the property attributes above force snake_case where the wire spec requires it. Single-word props (Source, Ipod, Add, Modify, etc.) match by case-insensitive default. If that proves flaky, set `JsonSerializerOptions.PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower` globally and drop the per-property attributes.

- [ ] **Step 2: Command records (`Models\Commands.cs`)**

```csharp
using System.Text.Json.Serialization;

namespace IpodSync.UI.Models;

[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(StartCommand), "start")]
[JsonDerivedType(typeof(ReviewDecisionCommand), "review_decision")]
[JsonDerivedType(typeof(PromptDecisionCommand), "prompt_decision")]
[JsonDerivedType(typeof(FormDecisionCommand), "form_decision")]
[JsonDerivedType(typeof(CancelCommand), "cancel")]
public abstract record IpcCommand;

public sealed record StartCommand : IpcCommand;

[JsonConverter(typeof(JsonStringEnumConverter<ReviewChoice>))]
public enum ReviewChoice { Apply, DryRun, Quit }

public sealed record ReviewDecisionCommand(
    ReviewChoice Choice,
    [property: JsonPropertyName("no_delete")] bool NoDelete
) : IpcCommand;

public sealed record PromptDecisionCommand(ulong Id, uint Choice) : IpcCommand;

public sealed record FormDecisionCommand(ulong Id, string? Value) : IpcCommand;

public sealed record CancelCommand : IpcCommand;
```

For `ReviewChoice` to serialize as `"apply"` / `"dry_run"` / `"quit"`, configure `JsonStringEnumConverter` with `JsonNamingPolicy.SnakeCaseLower`. Configure this once in a shared `JsonSerializerOptions` instance — see Step 5.

- [ ] **Step 3: `ProtocolVersion.cs`**

```csharp
namespace IpodSync.UI.Models;

public static class ProtocolVersion
{
    public const string Supported = "1.0.0";

    public static bool IsCompatible(string remote)
    {
        // Major must match; minor/patch are forward-compat (UI can ignore new events).
        var local = Supported.Split('.');
        var rem = remote.Split('.');
        return local.Length >= 1 && rem.Length >= 1 && local[0] == rem[0];
    }
}
```

- [ ] **Step 4: `ICoreProcess.cs`**

```csharp
using IpodSync.UI.Models;

namespace IpodSync.UI.Services;

/// <summary>
/// Abstracts the running Rust core subprocess so ViewModels can be unit-tested
/// against a fake implementation.
/// </summary>
public interface ICoreProcess : IAsyncDisposable
{
    /// <summary>Fires when a new event arrives from the core.</summary>
    event EventHandler<IpcEvent>? EventReceived;

    /// <summary>Fires once when the core process exits, regardless of cause.</summary>
    event EventHandler<int>? Exited;

    Task StartAsync(CancellationToken cancellationToken = default);
    Task SendAsync(IpcCommand command, CancellationToken cancellationToken = default);
    Task CancelAsync(TimeSpan gracePeriod, CancellationToken cancellationToken = default);
}
```

- [ ] **Step 5: `CoreProcess.cs`**

```csharp
using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Threading.Channels;
using IpodSync.UI.Models;

namespace IpodSync.UI.Services;

public sealed class CoreProcess : ICoreProcess
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        Converters = { new JsonStringEnumConverter(JsonNamingPolicy.SnakeCaseLower) },
    };

    private readonly string _exePath;
    private Process? _process;
    private CancellationTokenSource? _readerCts;
    private readonly Channel<IpcCommand> _writeQueue =
        Channel.CreateBounded<IpcCommand>(new BoundedChannelOptions(64)
        {
            SingleReader = true, SingleWriter = false,
            FullMode = BoundedChannelFullMode.Wait,
        });
    private Task? _writerLoop;

    public event EventHandler<IpcEvent>? EventReceived;
    public event EventHandler<int>? Exited;

    public CoreProcess(string exePath) => _exePath = exePath;

    public Task StartAsync(CancellationToken cancellationToken = default)
    {
        if (_process is not null) throw new InvalidOperationException("already started");

        var psi = new ProcessStartInfo
        {
            FileName = _exePath,
            Arguments = "--ipc-mode",
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            StandardInputEncoding = new UTF8Encoding(false),
            StandardOutputEncoding = new UTF8Encoding(false),
        };

        _process = new Process { StartInfo = psi, EnableRaisingEvents = true };
        _process.Exited += (s, e) =>
            Exited?.Invoke(this, _process?.ExitCode ?? -1);

        if (!_process.Start())
            throw new InvalidOperationException($"failed to start {_exePath}");

        _readerCts = new CancellationTokenSource();
        _ = Task.Run(() => ReadLoopAsync(_process.StandardOutput, _readerCts.Token));
        _writerLoop = Task.Run(() => WriteLoopAsync(_process.StandardInput, _readerCts.Token));

        return Task.CompletedTask;
    }

    private async Task ReadLoopAsync(StreamReader stdout, CancellationToken ct)
    {
        try
        {
            while (!ct.IsCancellationRequested)
            {
                var line = await stdout.ReadLineAsync(ct).ConfigureAwait(false);
                if (line is null) break; // EOF
                if (string.IsNullOrWhiteSpace(line)) continue;

                IpcEvent? evt;
                try { evt = JsonSerializer.Deserialize<IpcEvent>(line, JsonOptions); }
                catch (JsonException ex)
                {
                    // Forward as a synthetic error so the UI sees protocol corruption.
                    EventReceived?.Invoke(this, new ErrorEvent(
                        $"Could not parse core message: {ex.Message}",
                        new[] { $"Raw line: {line[..Math.Min(line.Length, 200)]}" }));
                    continue;
                }
                if (evt is not null) EventReceived?.Invoke(this, evt);
            }
        }
        catch (OperationCanceledException) { /* expected on dispose */ }
    }

    private async Task WriteLoopAsync(StreamWriter stdin, CancellationToken ct)
    {
        try
        {
            await foreach (var cmd in _writeQueue.Reader.ReadAllAsync(ct))
            {
                var line = JsonSerializer.Serialize<IpcCommand>(cmd, JsonOptions);
                await stdin.WriteLineAsync(line.AsMemory(), ct).ConfigureAwait(false);
                await stdin.FlushAsync().ConfigureAwait(false);
            }
        }
        catch (OperationCanceledException) { /* expected */ }
        catch (IOException) { /* core closed stdin */ }
    }

    public ValueTask SendAsync(IpcCommand command, CancellationToken cancellationToken = default)
    {
        return _writeQueue.Writer.WriteAsync(command, cancellationToken);
    }

    Task ICoreProcess.SendAsync(IpcCommand command, CancellationToken cancellationToken) =>
        SendAsync(command, cancellationToken).AsTask();

    public async Task CancelAsync(TimeSpan gracePeriod, CancellationToken cancellationToken = default)
    {
        if (_process is null || _process.HasExited) return;
        await SendAsync(new CancelCommand(), cancellationToken).ConfigureAwait(false);
        _writeQueue.Writer.TryComplete();
        try
        {
            using var graceCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            graceCts.CancelAfter(gracePeriod);
            await _process.WaitForExitAsync(graceCts.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            // Grace period expired or external cancel — force kill.
            try { _process.Kill(entireProcessTree: true); } catch { }
        }
    }

    public async ValueTask DisposeAsync()
    {
        _readerCts?.Cancel();
        if (_writerLoop is not null) { try { await _writerLoop; } catch { } }
        if (_process is not null)
        {
            if (!_process.HasExited)
            {
                try { _process.Kill(entireProcessTree: true); } catch { }
            }
            _process.Dispose();
        }
    }
}
```

- [ ] **Step 6: `CoreLocator.cs`**

```csharp
namespace IpodSync.UI.Services;

/// <summary>
/// Finds ipod-sync.exe. Search order:
///   1. Sibling to the UI .exe (production install).
///   2. ../../target/release/ipod-sync.exe (running from bin/Release/...).
///   3. ../../target/debug/ipod-sync.exe (running from bin/Debug/...).
///   4. On PATH (Process resolves it).
/// Returns the resolved absolute path, or null if nothing matched.
/// </summary>
public static class CoreLocator
{
    public static string? Locate()
    {
        var uiExeDir = AppContext.BaseDirectory;

        var candidates = new[]
        {
            Path.Combine(uiExeDir, "ipod-sync.exe"),
            Path.GetFullPath(Path.Combine(uiExeDir, "..", "..", "..", "..", "..", "target", "release", "ipod-sync.exe")),
            Path.GetFullPath(Path.Combine(uiExeDir, "..", "..", "..", "..", "..", "target", "debug", "ipod-sync.exe")),
        };

        foreach (var c in candidates)
        {
            if (File.Exists(c)) return c;
        }

        // Last resort: PATH lookup.
        var pathEnv = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        foreach (var dir in pathEnv.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            try
            {
                var c = Path.Combine(dir, "ipod-sync.exe");
                if (File.Exists(c)) return c;
            }
            catch { /* ignore bad PATH entries */ }
        }

        return null;
    }
}
```

The `..\..\..\..\..\target\` jump count assumes `IpodSync.UI\bin\Release\net10.0-windows10.0.19041.0\win-x64\` (5 levels up to the repo root). Verify the actual output path during testing and adjust if needed.

- [ ] **Step 7: Tests**

`ProtocolSerializationTests.cs` — round-trip every message type:

```csharp
using System.Text.Json;
using System.Text.Json.Serialization;
using IpodSync.UI.Models;

namespace IpodSync.UI.Tests;

public class ProtocolSerializationTests
{
    private static readonly JsonSerializerOptions Opts = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        Converters = { new JsonStringEnumConverter(JsonNamingPolicy.SnakeCaseLower) },
    };

    [Fact]
    public void Deserializes_HelloEvent()
    {
        var json = """{"type":"hello","protocol_version":"1.0.0","core_version":"0.0.1"}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json, Opts);
        var hello = Assert.IsType<HelloEvent>(evt);
        Assert.Equal("1.0.0", hello.ProtocolVersion);
        Assert.Equal("0.0.1", hello.CoreVersion);
    }

    [Fact]
    public void Deserializes_SummaryEvent()
    {
        var json = """{"type":"summary","add":12,"modify":3,"metadata_only":0,"remove":0,"unchanged":1260,"total_planned":15}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json, Opts);
        var s = Assert.IsType<SummaryEvent>(evt);
        Assert.Equal(12u, s.Add);
        Assert.Equal(15u, s.TotalPlanned);
    }

    [Fact]
    public void Deserializes_PromptEvent_WithOptions()
    {
        var json = """{"type":"prompt","id":7,"message":"Retry?","options":["Retry","Skip","Abort"]}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json, Opts);
        var p = Assert.IsType<PromptEvent>(evt);
        Assert.Equal(7ul, p.Id);
        Assert.Equal(3, p.Options.Count);
    }

    [Fact]
    public void Deserializes_TrackDoneEvent_AsUnitVariant()
    {
        var json = """{"type":"track_done"}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json, Opts);
        Assert.IsType<TrackDoneEvent>(evt);
    }

    [Fact]
    public void Serializes_ReviewDecisionCommand_WithSnakeCaseChoice()
    {
        var cmd = new ReviewDecisionCommand(ReviewChoice.DryRun, NoDelete: true);
        var json = JsonSerializer.Serialize<IpcCommand>(cmd, Opts);
        Assert.Contains(@"""type"":""review_decision""", json);
        Assert.Contains(@"""choice"":""dry_run""", json);
        Assert.Contains(@"""no_delete"":true", json);
    }

    [Fact]
    public void Serializes_FormDecisionCommand_WithNullValue()
    {
        var cmd = new FormDecisionCommand(Id: 1, Value: null);
        var json = JsonSerializer.Serialize<IpcCommand>(cmd, Opts);
        Assert.Contains(@"""value"":null", json);
    }

    [Fact]
    public void Serializes_CancelCommand_AsTypeOnly()
    {
        var cmd = new CancelCommand();
        var json = JsonSerializer.Serialize<IpcCommand>(cmd, Opts);
        Assert.Equal(@"{""type"":""cancel""}", json);
    }

    [Fact]
    public void ProtocolVersion_AcceptsSameMajor()
    {
        Assert.True(ProtocolVersion.IsCompatible("1.0.0"));
        Assert.True(ProtocolVersion.IsCompatible("1.5.0"));
        Assert.False(ProtocolVersion.IsCompatible("2.0.0"));
        Assert.False(ProtocolVersion.IsCompatible("0.9.0"));
    }
}
```

`FakeCoreProcessTests.cs` — wires a fake `ICoreProcess` and verifies subscriber receives events:

```csharp
using IpodSync.UI.Models;
using IpodSync.UI.Services;

namespace IpodSync.UI.Tests;

internal sealed class FakeCoreProcess : ICoreProcess
{
    public event EventHandler<IpcEvent>? EventReceived;
    public event EventHandler<int>? Exited;
    public List<IpcCommand> Sent { get; } = new();

    public Task StartAsync(CancellationToken ct = default) => Task.CompletedTask;
    public Task SendAsync(IpcCommand cmd, CancellationToken ct = default) { Sent.Add(cmd); return Task.CompletedTask; }
    public Task CancelAsync(TimeSpan grace, CancellationToken ct = default) { Exited?.Invoke(this, 0); return Task.CompletedTask; }
    public ValueTask DisposeAsync() => ValueTask.CompletedTask;

    public void Emit(IpcEvent evt) => EventReceived?.Invoke(this, evt);
}

public class FakeCoreProcessTests
{
    [Fact]
    public async Task Subscriber_ReceivesEmittedEvents()
    {
        var fake = new FakeCoreProcess();
        var received = new List<IpcEvent>();
        fake.EventReceived += (s, e) => received.Add(e);

        await fake.StartAsync();
        fake.Emit(new HelloEvent("1.0.0", "0.0.1"));
        fake.Emit(new TrackDoneEvent());

        Assert.Equal(2, received.Count);
        Assert.IsType<HelloEvent>(received[0]);
        Assert.IsType<TrackDoneEvent>(received[1]);
    }

    [Fact]
    public async Task Sent_AccumulatesCommands()
    {
        var fake = new FakeCoreProcess();
        await fake.SendAsync(new CancelCommand());
        await fake.SendAsync(new ReviewDecisionCommand(ReviewChoice.Apply, false));
        Assert.Equal(2, fake.Sent.Count);
    }
}
```

`CoreLocatorTests.cs` — verifies the locator falls back gracefully:

```csharp
using IpodSync.UI.Services;

namespace IpodSync.UI.Tests;

public class CoreLocatorTests
{
    [Fact]
    public void Locate_DoesNotThrow()
    {
        // Test environment likely won't have ipod-sync.exe; returning null is fine.
        var result = CoreLocator.Locate();
        // No assertion on the result — just that it doesn't throw on any bad PATH entry.
        Assert.True(result is null || File.Exists(result));
    }
}
```

- [ ] **Step 8: Build + test**

```powershell
cd F:\repos\ipod-sync\ui-windows
dotnet build IpodSync.UI.sln -c Release 2>&1 | Select-Object -Last 10
dotnet test IpodSync.UI.sln 2>&1 | Select-Object -Last 20
```

Expected: clean build, ~11 tests pass (8 serialization + 2 fake + 1 locator).

- [ ] **Step 9: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows\IpodSync.UI\Models\Events.cs ui-windows\IpodSync.UI\Models\Commands.cs ui-windows\IpodSync.UI\Models\ProtocolVersion.cs ui-windows\IpodSync.UI\Services\ICoreProcess.cs ui-windows\IpodSync.UI\Services\CoreProcess.cs ui-windows\IpodSync.UI\Services\CoreLocator.cs ui-windows\IpodSync.UI.Tests\ProtocolSerializationTests.cs ui-windows\IpodSync.UI.Tests\FakeCoreProcessTests.cs ui-windows\IpodSync.UI.Tests\CoreLocatorTests.cs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(ui-windows): CoreProcess IPC client + wire-type records

- Models/Events.cs + Commands.cs: System.Text.Json polymorphic records
  matching the Rust IpcEvent / IpcCommand contract. snake_case naming
  policy + JsonStringEnumConverter for ReviewChoice values.
- Models/ProtocolVersion.cs: semver compat check (major must match).
- Services/ICoreProcess.cs: interface so ViewModels test against a fake.
- Services/CoreProcess.cs: spawns ipod-sync.exe --ipc-mode, reads NDJSON
  from stdout into typed events (raises EventReceived), writes commands
  from a System.Threading.Channels bounded queue. CancelAsync waits up
  to gracePeriod then force-kills the process tree.
- Services/CoreLocator.cs: search order is sibling exe -> target/release
  -> target/debug -> PATH.

11 unit tests: every event/command round-trips; ProtocolVersion semver
gate; CoreLocator doesn't throw; FakeCoreProcess delivers events to
subscribers and accumulates Sent commands.
EOF
)"
```

---

## Task 6: `MainViewModel` + `ReviewViewModel` + `ReviewPage` (depends on Tasks 4, 5)

**Files:**
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\MainViewModel.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\ReviewViewModel.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\ReviewPage.xaml`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\ReviewPage.xaml.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\MainWindow.xaml`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\MainWindow.xaml.cs`

Sub-skill: `winui3-csharp-app` for Page + Frame navigation patterns.

- [ ] **Step 1: `MainViewModel.cs`**

Owns the `CoreProcess` lifecycle and the top-level state machine (Idle → Spawning → Awaiting Header → Awaiting Review → Applying → Done).

```csharp
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using IpodSync.UI.Models;
using IpodSync.UI.Services;
using Microsoft.UI.Dispatching;

namespace IpodSync.UI.ViewModels;

public enum MainState { Idle, Spawning, AwaitingHeader, AwaitingReview, Applying, Done, Error }

public partial class MainViewModel : ObservableObject, IAsyncDisposable
{
    private readonly DispatcherQueue _dispatcher;
    private ICoreProcess? _core;

    [ObservableProperty] private MainState _state = MainState.Idle;
    [ObservableProperty] private string _statusText = "Idle. Click Start to begin.";
    [ObservableProperty] private string _sourcePath = "";
    [ObservableProperty] private string _ipodPath = "";
    [ObservableProperty] private string _manifestPath = "";
    [ObservableProperty] private ReviewViewModel? _review;
    [ObservableProperty] private ProgressViewModel? _progress;

    public MainViewModel(DispatcherQueue dispatcher) => _dispatcher = dispatcher;

    [RelayCommand]
    public async Task StartAsync()
    {
        if (State != MainState.Idle) return;
        State = MainState.Spawning;
        StatusText = "Locating ipod-sync.exe...";

        var exe = CoreLocator.Locate();
        if (exe is null)
        {
            State = MainState.Error;
            StatusText = "ipod-sync.exe not found. Build the Rust binary (cargo build --release) or set the path.";
            return;
        }

        _core = new CoreProcess(exe);
        _core.EventReceived += OnEventReceived;
        _core.Exited += OnExited;
        await _core.StartAsync().ConfigureAwait(false);
        StatusText = $"Spawned: {exe}";
    }

    private void OnEventReceived(object? sender, IpcEvent evt)
    {
        // Marshal to UI thread.
        _dispatcher.TryEnqueue(() => HandleEvent(evt));
    }

    private void HandleEvent(IpcEvent evt)
    {
        switch (evt)
        {
            case HelloEvent h:
                if (!ProtocolVersion.IsCompatible(h.ProtocolVersion))
                {
                    State = MainState.Error;
                    StatusText = $"Protocol mismatch: UI supports {ProtocolVersion.Supported}, core sent {h.ProtocolVersion}.";
                }
                else
                {
                    State = MainState.AwaitingHeader;
                    StatusText = $"Core connected (v{h.CoreVersion}).";
                }
                break;
            case HeaderEvent h:
                SourcePath = h.Source;
                IpodPath = h.Ipod;
                ManifestPath = h.Manifest;
                StatusText = "Diffing library...";
                break;
            case SummaryEvent _:
                // Counts arrive but the Review event right after carries the same data.
                break;
            case ReviewEvent r:
                Review = new ReviewViewModel(r, async (choice, noDelete) =>
                {
                    if (_core is null) return;
                    await _core.SendAsync(new ReviewDecisionCommand(choice, noDelete));
                    if (choice == ReviewChoice.Quit)
                    {
                        State = MainState.Done;
                        StatusText = "Quit requested.";
                    }
                    else
                    {
                        State = MainState.Applying;
                        StatusText = "Applying...";
                        Progress = new ProgressViewModel();
                    }
                });
                State = MainState.AwaitingReview;
                break;
            case TrackStartEvent t:
                Progress?.OnTrackStart(t);
                break;
            case TrackDoneEvent _:
                Progress?.OnTrackDone();
                break;
            case LogEvent l:
                Progress?.AddLog(l.Message);
                break;
            case ErrorEvent e:
                Progress?.AddLog($"ERROR: {e.Message}");
                StatusText = $"Error: {e.Message}";
                break;
            case FinishEvent f:
                State = MainState.Done;
                StatusText = f.Success ? "Done." : "Finished with errors.";
                break;
        }
    }

    private void OnExited(object? sender, int exitCode)
    {
        _dispatcher.TryEnqueue(() =>
        {
            if (State != MainState.Done && State != MainState.Error)
            {
                State = MainState.Error;
                StatusText = $"Core process exited unexpectedly (code {exitCode}).";
            }
        });
    }

    public async ValueTask DisposeAsync()
    {
        if (_core is not null)
        {
            await _core.CancelAsync(TimeSpan.FromSeconds(5)).ConfigureAwait(false);
            await _core.DisposeAsync().ConfigureAwait(false);
        }
    }
}
```

- [ ] **Step 2: `ReviewViewModel.cs`**

```csharp
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using IpodSync.UI.Models;

namespace IpodSync.UI.ViewModels;

public partial class ReviewViewModel : ObservableObject
{
    private readonly Func<ReviewChoice, bool, Task> _decide;

    public uint Add { get; }
    public uint Modify { get; }
    public uint MetadataOnly { get; }
    public uint Remove { get; }
    public uint Unchanged { get; }

    [ObservableProperty] private bool _noDelete;

    public ReviewViewModel(ReviewEvent evt, Func<ReviewChoice, bool, Task> decide)
    {
        Add = evt.Summary.Add;
        Modify = evt.Summary.Modify;
        MetadataOnly = evt.Summary.MetadataOnly;
        Remove = evt.Summary.Remove;
        Unchanged = evt.Summary.Unchanged;
        NoDelete = evt.NoDelete;
        _decide = decide;
    }

    [RelayCommand]
    private Task Apply() => _decide(ReviewChoice.Apply, NoDelete);

    [RelayCommand]
    private Task DryRun() => _decide(ReviewChoice.DryRun, NoDelete);

    [RelayCommand]
    private Task Quit() => _decide(ReviewChoice.Quit, NoDelete);
}
```

- [ ] **Step 3: `ReviewPage.xaml` + code-behind**

```xml
<Page
    x:Class="IpodSync.UI.Views.ReviewPage"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    xmlns:vm="using:IpodSync.UI.ViewModels">
    <Page.DataContext>
        <vm:ReviewViewModel x:Name="Vm" />
    </Page.DataContext>
    <StackPanel Padding="24" Spacing="16">
        <TextBlock Text="Review action plan" FontSize="24" FontWeight="SemiBold" />
        <Grid ColumnDefinitions="Auto,*" RowDefinitions="Auto,Auto,Auto,Auto,Auto" ColumnSpacing="16" RowSpacing="4">
            <TextBlock Grid.Row="0" Grid.Column="0" Text="Add:" />
            <TextBlock Grid.Row="0" Grid.Column="1" Text="{Binding Add}" FontWeight="SemiBold" />
            <TextBlock Grid.Row="1" Grid.Column="0" Text="Modify:" />
            <TextBlock Grid.Row="1" Grid.Column="1" Text="{Binding Modify}" FontWeight="SemiBold" />
            <TextBlock Grid.Row="2" Grid.Column="0" Text="Metadata only:" />
            <TextBlock Grid.Row="2" Grid.Column="1" Text="{Binding MetadataOnly}" FontWeight="SemiBold" />
            <TextBlock Grid.Row="3" Grid.Column="0" Text="Remove:" />
            <TextBlock Grid.Row="3" Grid.Column="1" Text="{Binding Remove}" FontWeight="SemiBold" />
            <TextBlock Grid.Row="4" Grid.Column="0" Text="Unchanged:" />
            <TextBlock Grid.Row="4" Grid.Column="1" Text="{Binding Unchanged}" FontWeight="SemiBold" />
        </Grid>
        <CheckBox Content="Never remove tracks (--no-delete)" IsChecked="{Binding NoDelete, Mode=TwoWay}" />
        <StackPanel Orientation="Horizontal" Spacing="8">
            <Button Content="Apply" Command="{Binding ApplyCommand}" Style="{StaticResource AccentButtonStyle}" />
            <Button Content="Dry run" Command="{Binding DryRunCommand}" />
            <Button Content="Quit" Command="{Binding QuitCommand}" />
        </StackPanel>
    </StackPanel>
</Page>
```

`ReviewPage.xaml.cs`:
```csharp
using Microsoft.UI.Xaml.Controls;

namespace IpodSync.UI.Views;

public sealed partial class ReviewPage : Page
{
    public ReviewPage() { InitializeComponent(); }
}
```

The `Page.DataContext` static markup above is a placeholder so XAML compiles — the actual VM gets assigned by the host (`MainWindow`) when the page is displayed.

- [ ] **Step 4: Update `MainWindow` to host a Frame + bind**

`MainWindow.xaml`:
```xml
<Window
    x:Class="IpodSync.UI.MainWindow"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    Title="ipod-sync">
    <Grid Padding="24" RowDefinitions="Auto,Auto,*">
        <StackPanel Grid.Row="0" Spacing="8">
            <TextBlock Text="ipod-sync" FontSize="28" FontWeight="SemiBold" />
            <TextBlock Text="{x:Bind Vm.StatusText, Mode=OneWay}" Opacity="0.7" />
        </StackPanel>
        <StackPanel Grid.Row="1" Orientation="Horizontal" Spacing="12" Margin="0,12,0,12">
            <Button Content="Start sync" Command="{x:Bind Vm.StartCommand}" Style="{StaticResource AccentButtonStyle}" />
        </StackPanel>
        <Border Grid.Row="2" BorderBrush="{ThemeResource ControlElevationBorderBrush}" BorderThickness="1" CornerRadius="4">
            <Grid>
                <TextBlock Text="No review yet."
                           HorizontalAlignment="Center" VerticalAlignment="Center" Opacity="0.5"
                           Visibility="{x:Bind Vm.Review, Mode=OneWay, Converter={StaticResource NullToVisibleConverter}}" />
                <Frame x:Name="ContentFrame" />
            </Grid>
        </Border>
    </Grid>
</Window>
```

(The `NullToVisibleConverter` reference is a thin converter you can either author in `Converters\NullToVisibleConverter.cs` or skip — alternative is a simple `IsReviewVisible` computed property on the VM. Implementer's choice; the WinUI3 skill covers converters.)

`MainWindow.xaml.cs`:
```csharp
using IpodSync.UI.ViewModels;
using IpodSync.UI.Views;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace IpodSync.UI;

public sealed partial class MainWindow : Window
{
    public MainViewModel Vm { get; }

    public MainWindow()
    {
        Vm = new MainViewModel(DispatcherQueue.GetForCurrentThread());
        Vm.PropertyChanged += (s, e) =>
        {
            if (e.PropertyName == nameof(Vm.Review) && Vm.Review is not null)
            {
                var page = new ReviewPage();
                page.DataContext = Vm.Review;
                ContentFrame.Content = page;
            }
        };
        InitializeComponent();
        Closed += async (s, e) => await Vm.DisposeAsync();
    }
}
```

- [ ] **Step 5: Build + test**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.sln -c Release 2>&1 | Select-Object -Last 10
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.sln 2>&1 | Select-Object -Last 10
```

Expected: clean build, all existing tests still pass.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows\IpodSync.UI\ViewModels\MainViewModel.cs ui-windows\IpodSync.UI\ViewModels\ReviewViewModel.cs ui-windows\IpodSync.UI\Views\ReviewPage.xaml ui-windows\IpodSync.UI\Views\ReviewPage.xaml.cs ui-windows\IpodSync.UI\MainWindow.xaml ui-windows\IpodSync.UI\MainWindow.xaml.cs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(ui-windows): MainViewModel + ReviewViewModel + ReviewPage

MainViewModel owns the CoreProcess lifecycle and runs a small state machine
(Idle -> Spawning -> AwaitingHeader -> AwaitingReview -> Applying ->
Done/Error). Marshals every event to the UI thread via DispatcherQueue.

On HelloEvent it verifies protocol_version major matches; mismatch flips
State to Error. On ReviewEvent it constructs a ReviewViewModel exposing
Add/Modify/MetadataOnly/Remove/Unchanged + a TwoWay NoDelete checkbox and
three RelayCommands (Apply/DryRun/Quit). Each command sends the
corresponding ReviewDecisionCommand via CoreProcess.SendAsync.

MainWindow shows the StatusText + Start button + a Frame that hosts
ReviewPage once Review is non-null.

ProgressViewModel + ProgressPage land in Task 7.
EOF
)"
```

---

## Task 7: `ProgressViewModel` + `ProgressPage` (depends on Task 6)

**Files:**
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\ProgressViewModel.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\ProgressPage.xaml`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\ProgressPage.xaml.cs`
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\MainWindow.xaml.cs` (host ProgressPage when Progress is non-null)

- [ ] **Step 1: `ProgressViewModel.cs`**

```csharp
using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using IpodSync.UI.Models;

namespace IpodSync.UI.ViewModels;

public partial class ProgressViewModel : ObservableObject
{
    private const int MaxLogLines = 200;

    [ObservableProperty] private uint _current;
    [ObservableProperty] private uint _total;
    [ObservableProperty] private string _currentLabel = "";
    [ObservableProperty] private double _percent;

    public ObservableCollection<string> Log { get; } = new();

    public void OnTrackStart(TrackStartEvent e)
    {
        Current = e.Current;
        Total = e.Total;
        CurrentLabel = e.Label;
        Percent = Total > 0 ? (Current * 100.0 / Total) : 0;
    }

    public void OnTrackDone()
    {
        // No-op; Percent advances on next TrackStart. If the run finishes on a
        // TrackDone with no follow-up TrackStart, Percent shows 100% only after
        // FinishEvent (caller can set it).
    }

    public void AddLog(string line)
    {
        Log.Add(line);
        while (Log.Count > MaxLogLines) Log.RemoveAt(0);
    }
}
```

- [ ] **Step 2: `ProgressPage.xaml` + code-behind**

```xml
<Page
    x:Class="IpodSync.UI.Views.ProgressPage"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
    <Grid Padding="24" RowDefinitions="Auto,Auto,Auto,*" RowSpacing="12">
        <TextBlock Grid.Row="0" Text="Applying" FontSize="20" FontWeight="SemiBold" />
        <ProgressBar Grid.Row="1" Value="{Binding Percent}" Maximum="100" />
        <TextBlock Grid.Row="2">
            <Run Text="{Binding Current}" />
            <Run Text=" / " />
            <Run Text="{Binding Total}" />
            <Run Text="  -  " />
            <Run Text="{Binding CurrentLabel}" />
        </TextBlock>
        <Border Grid.Row="3" BorderBrush="{ThemeResource ControlElevationBorderBrush}" BorderThickness="1" CornerRadius="4">
            <ListView ItemsSource="{Binding Log}" SelectionMode="None" />
        </Border>
    </Grid>
</Page>
```

`ProgressPage.xaml.cs`:
```csharp
using Microsoft.UI.Xaml.Controls;

namespace IpodSync.UI.Views;

public sealed partial class ProgressPage : Page
{
    public ProgressPage() { InitializeComponent(); }
}
```

- [ ] **Step 3: Update `MainWindow.xaml.cs` to switch to ProgressPage**

```csharp
Vm.PropertyChanged += (s, e) =>
{
    if (e.PropertyName == nameof(Vm.Review) && Vm.Review is not null)
    {
        var page = new ReviewPage();
        page.DataContext = Vm.Review;
        ContentFrame.Content = page;
    }
    else if (e.PropertyName == nameof(Vm.Progress) && Vm.Progress is not null)
    {
        var page = new ProgressPage();
        page.DataContext = Vm.Progress;
        ContentFrame.Content = page;
    }
};
```

- [ ] **Step 4: Build + test**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.sln -c Release 2>&1 | Select-Object -Last 10
dotnet test F:\repos\ipod-sync\ui-windows\IpodSync.UI.sln 2>&1 | Select-Object -Last 10
```

Expected: clean build, all tests pass.

- [ ] **Step 5: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows\IpodSync.UI\ViewModels\ProgressViewModel.cs ui-windows\IpodSync.UI\Views\ProgressPage.xaml ui-windows\IpodSync.UI\Views\ProgressPage.xaml.cs ui-windows\IpodSync.UI\MainWindow.xaml.cs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(ui-windows): ProgressViewModel + ProgressPage

ProgressViewModel exposes Current/Total/CurrentLabel/Percent + an
ObservableCollection<string> Log capped at 200 lines. Driven by
MainViewModel forwarding TrackStartEvent/TrackDoneEvent/LogEvent/
ErrorEvent into the corresponding methods.

ProgressPage binds a ProgressBar to Percent, shows current track label,
and lists the log tail in a ListView.

MainWindow now switches ContentFrame to ProgressPage when Vm.Progress
becomes non-null (which happens when the user picks Apply on the
Review page).
EOF
)"
```

---

## Task 8: Path resolution + first-run error dialog (depends on Tasks 5, 6)

**Files:**
- Modify: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\ViewModels\MainViewModel.cs`
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\CoreNotFoundDialog.xaml` (optional — could be a simple ContentDialog in code)
- New: `F:\repos\ipod-sync\ui-windows\IpodSync.UI\Views\CoreNotFoundDialog.xaml.cs`

Replaces the M1 "ipod-sync.exe not found" status text fallback with an actionable ContentDialog: shows the search paths tried, links to the build instructions, has a "Choose path" button that opens a file picker.

- [ ] **Step 1: `CoreNotFoundDialog`**

```xml
<ContentDialog
    x:Class="IpodSync.UI.Views.CoreNotFoundDialog"
    xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
    Title="ipod-sync.exe not found"
    PrimaryButtonText="Choose .exe location..."
    CloseButtonText="Cancel">
    <StackPanel Spacing="12">
        <TextBlock TextWrapping="Wrap">
            ipod-sync.exe is the Rust core that does the actual syncing.
            The UI needs it to be either next to IpodSync.UI.exe, or somewhere on PATH.
        </TextBlock>
        <TextBlock Text="Searched:" FontWeight="SemiBold" />
        <TextBlock x:Name="SearchedList" FontFamily="Consolas" TextWrapping="Wrap" Opacity="0.8" />
        <TextBlock TextWrapping="Wrap">
            Build it with `cargo build --release` from the repo root.
        </TextBlock>
    </StackPanel>
</ContentDialog>
```

`CoreNotFoundDialog.xaml.cs`:
```csharp
using Microsoft.UI.Xaml.Controls;

namespace IpodSync.UI.Views;

public sealed partial class CoreNotFoundDialog : ContentDialog
{
    public CoreNotFoundDialog(string searched)
    {
        InitializeComponent();
        SearchedList.Text = searched;
    }
}
```

- [ ] **Step 2: Update `MainViewModel.StartAsync`**

Inject the XamlRoot so the dialog can be shown. Easiest: pass the MainWindow's `Content.XamlRoot` from `MainWindow` after construction.

```csharp
public XamlRoot? XamlRoot { get; set; }

[RelayCommand]
public async Task StartAsync()
{
    if (State != MainState.Idle) return;
    State = MainState.Spawning;
    StatusText = "Locating ipod-sync.exe...";

    var exe = CoreLocator.Locate();
    if (exe is null)
    {
        State = MainState.Idle; // back to idle so user can retry
        StatusText = "ipod-sync.exe not found.";
        if (XamlRoot is not null)
        {
            var dlg = new CoreNotFoundDialog(CoreLocator.LastSearchedPaths())
            {
                XamlRoot = XamlRoot,
            };
            var result = await dlg.ShowAsync();
            if (result == ContentDialogResult.Primary)
            {
                // M1: just log it; full file picker hooks land in M2.
                StatusText = "Manual path selection deferred to M2.";
            }
        }
        return;
    }
    // ... existing spawn flow ...
}
```

This requires `CoreLocator` to expose `LastSearchedPaths()`. Add a thread-local or static cache there:

```csharp
public static class CoreLocator
{
    private static string? _lastSearched;
    public static string LastSearchedPaths() => _lastSearched ?? "(no search performed yet)";

    public static string? Locate()
    {
        var paths = new List<string>();
        // ... append each candidate to `paths` whether or not it exists ...
        _lastSearched = string.Join("\n", paths);
        // ... existing logic ...
    }
}
```

- [ ] **Step 3: Pass XamlRoot from `MainWindow`**

```csharp
public MainWindow()
{
    Vm = new MainViewModel(DispatcherQueue.GetForCurrentThread());
    InitializeComponent();
    // Set XamlRoot after Activate (Content.XamlRoot may be null otherwise).
    Activated += (s, e) =>
    {
        if (Content?.XamlRoot is not null) Vm.XamlRoot = Content.XamlRoot;
    };
    // ... existing PropertyChanged handler + Closed handler ...
}
```

- [ ] **Step 4: Build + test**

```powershell
dotnet build F:\repos\ipod-sync\ui-windows\IpodSync.UI.sln -c Release 2>&1 | Select-Object -Last 10
```

- [ ] **Step 5: Manual test the missing-exe path**

Temporarily rename `F:\repos\ipod-sync\target\release\ipod-sync.exe` to `ipod-sync.exe.bak`, launch the UI, click Start. Expected: dialog appears with the searched paths. Click Cancel. Restore the .exe name.

- [ ] **Step 6: Commit**

```powershell
git -C F:\repos\ipod-sync add ui-windows\IpodSync.UI\ViewModels\MainViewModel.cs ui-windows\IpodSync.UI\Views\CoreNotFoundDialog.xaml ui-windows\IpodSync.UI\Views\CoreNotFoundDialog.xaml.cs ui-windows\IpodSync.UI\Services\CoreLocator.cs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
feat(ui-windows): actionable ContentDialog when ipod-sync.exe is missing

CoreLocator now remembers the search paths it tried; LastSearchedPaths()
exposes them. MainViewModel.StartAsync presents a CoreNotFoundDialog when
Locate() returns null, showing the searched candidates and a build hint.

Manual file picker hooks for "Choose .exe location" are deferred to M2;
the M1 dialog acknowledges the click and returns to Idle.
EOF
)"
```

---

## Task 9: Rust IPC integration test (depends on Task 3)

**Files:**
- New: `F:\repos\ipod-sync\tests\ipc_integration.rs`

End-to-end Rust test that spawns the compiled `ipod-sync.exe --ipc-mode`, reads the `hello` event, sends `cancel`, asserts the process exits. Doesn't drive a full sync (would need an iPod); validates the protocol plumbing in isolation.

- [ ] **Step 1: Write the integration test**

```rust
//! End-to-end Rust IPC test: spawn ipod-sync.exe --ipc-mode, verify handshake,
//! send cancel, assert clean exit. Doesn't run a full sync.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn ipod_sync_exe() -> std::path::PathBuf {
    // CARGO_BIN_EXE_<name> is set by cargo for integration tests.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_ipod-sync"))
}

#[test]
fn ipc_handshake_emits_hello_with_supported_protocol_version() {
    let mut child = Command::new(ipod_sync_exe())
        .arg("--ipc-mode")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ipod-sync --ipc-mode");

    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).expect("read hello line");

    let parsed: serde_json::Value = serde_json::from_str(first_line.trim())
        .expect("hello line is JSON");
    assert_eq!(parsed["type"], "hello");
    assert_eq!(parsed["protocol_version"], "1.0.0");
    assert!(parsed["core_version"].is_string());

    // Send cancel.
    let mut stdin = child.stdin.take().expect("stdin pipe");
    writeln!(stdin, r#"{{"type":"cancel"}}"#).expect("send cancel");
    drop(stdin);

    // Wait for exit within 10s.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_status) => break,
            None => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    panic!("ipod-sync did not exit within 10s after cancel");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}
```

NOTE: this test will likely fail downstream of the handshake because the orchestrator runs (and will fail without an iPod / source config). The test only validates that:
1. The hello line is emitted.
2. The process is at least responsive enough to be killed on cancel within 10s.

If the orchestrator's pre-resolve errors block reading stdin (because they're waiting on a prompt response we never send), the process won't exit on cancel — it'll need force-kill. That's fine for this test; the assertion is just "we got hello." Adjust the timeout if needed during real-run testing.

- [ ] **Step 2: Build + test**

```powershell
cargo test --test ipc_integration 2>&1 | Select-Object -Last 10
```

Expected: 1 test passes.

- [ ] **Step 3: Commit**

```powershell
git -C F:\repos\ipod-sync add tests\ipc_integration.rs
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "$(cat <<'EOF'
test(ipc): integration test spawns --ipc-mode and verifies hello handshake

Drives the compiled ipod-sync.exe via CARGO_BIN_EXE_ipod-sync, reads the
first stdout line, parses it as JSON, asserts type=hello and
protocol_version="1.0.0". Sends a cancel command and asserts the process
exits within 10s.

Doesn't run a full sync (would need an iPod). Validates protocol plumbing
in isolation.
EOF
)"
```

---

## Task 10: M1 smoke gate + LEARNINGS entry + tag

**Files:**
- Modify: `F:\repos\ipod-sync\LEARNINGS.md`

Manual smoke test against a real iPod, results recorded.

- [ ] **Scenario 1: Apply path drives a real sync**

```powershell
cd F:\repos\ipod-sync
cargo build --release
dotnet build ui-windows\IpodSync.UI.sln -c Release
# Launch the UI:
F:\repos\ipod-sync\ui-windows\IpodSync.UI\bin\Release\net10.0-windows10.0.19041.0\win-x64\IpodSync.UI.exe
# Click Start. Review page should show your library's action plan.
# Click Apply. Watch ProgressPage drive.
# Expected: same outcome as running `ipod-sync.exe --apply` from the TUI.
```

- [ ] **Scenario 2: Quit cancels cleanly**

Launch the UI, click Start, on the Review page click Quit. Expected: status shows "Quit requested", no zombie `ipod-sync.exe` in Task Manager within 10 seconds.

- [ ] **Scenario 3: Core process force-killed mid-sync**

Launch the UI, click Start, click Apply. While ProgressPage is updating, open Task Manager and end `ipod-sync.exe`. Expected: within 3 seconds the UI shows "Core process exited unexpectedly (code -1)."

- [ ] **Scenario 4: Protocol version mismatch**

In `src/ipc.rs` temporarily change `PROTOCOL_VERSION` to `"2.0.0"`. Rebuild Rust (`cargo build --release`). Launch the UI, click Start. Expected: status shows "Protocol mismatch: UI supports 1.0.0, core sent 2.0.0." Revert the change.

- [ ] **Scenario 5: ipod-sync.exe missing**

Rename `F:\repos\ipod-sync\target\release\ipod-sync.exe` to `ipod-sync.exe.bak`. Launch the UI, click Start. Expected: CoreNotFoundDialog appears with searched paths. Click Cancel, restore the name.

- [ ] **Append to `LEARNINGS.md`**

```markdown
## Phase 6 M1 gate (YYYY-MM-DD) — PASS / FAIL

- **Result:** PASS / FAIL (<one-line summary>)
- **Scenario 1 (Apply real sync):** PASS / FAIL — <observed track count, time, anything weird>
- **Scenario 2 (Quit clean):** PASS / FAIL — <zombie process? exit time?>
- **Scenario 3 (force-kill mid-sync):** PASS / FAIL — <UI response time, any hangs?>
- **Scenario 4 (protocol mismatch):** PASS / FAIL — <UI message clean?>
- **Scenario 5 (missing exe):** PASS / FAIL — <dialog appeared? searched paths shown?>
- **Build matrix:** Rust release build OK / FAIL; dotnet build Release OK / FAIL; dotnet test OK / FAIL
- **Observations:** (stdout buffering, log file location working, anything surprising)
```

- [ ] **Commit + tag**

```powershell
git -C F:\repos\ipod-sync add LEARNINGS.md
git -C F:\repos\ipod-sync -c user.email=19785650+itsmichaelwest@users.noreply.github.com commit -m "docs: Phase 6 M1 gate result"
git -C F:\repos\ipod-sync tag -a phase-6-m1-complete -m "Phase 6 M1 — IPC protocol + Rust --ipc-mode + minimal WinUI shell

- docs/ipc-protocol.md formalizes the JSON-over-stdio contract
- Rust: --ipc-mode flag + IpcBackend in progress.rs + file-routed tracing
- Rust: src/ipc.rs wire types (IpcEvent + IpcCommand) with 13 unit tests
- Rust: tests/ipc_integration.rs validates handshake + cancel end-to-end
- C#: ui-windows/ WinUI 3 solution (.NET 10, Windows App SDK 1.6+)
- C#: CoreProcess IPC client with bounded Channel-based writer queue
- C#: MainViewModel + ReviewViewModel + ProgressViewModel via CommunityToolkit.Mvvm
- C#: ReviewPage + ProgressPage + CoreNotFoundDialog
- Manual gate: real sync drives end-to-end through the WinUI app"
```

---

## Self-review

**Spec coverage check (against Phase 6 M1 acceptance criteria):**

- Build (Rust + dotnet) → Task 3 Step 6 + Task 4 Step 7 + Task 5 Step 8 + Task 6 Step 5 ✓
- Tests (cargo + dotnet) → Tasks 2, 3, 5 (Rust + C# unit tests); Task 9 (integration) ✓
- Launch → Task 4 Step 8 (initial launch); Task 10 Scenario 1 ✓
- Handshake → Task 3 Step 7 (Rust side); Task 5 (C# side); Task 6 (HelloEvent handling) ✓
- Review render + buttons → Task 6 ✓
- Apply drives sync → Task 6 + Task 7 + Task 10 Scenario 1 ✓
- Quit (graceful cancel) → Task 5 (CoreProcess.CancelAsync); Task 10 Scenario 2 ✓
- Crash recovery → Task 5 (Exited event); Task 6 (OnExited handler); Task 10 Scenario 3 ✓
- Version mismatch → Task 5 (ProtocolVersion.IsCompatible); Task 6 (HelloEvent branch); Task 10 Scenario 4 ✓
- No JSON corruption → Task 3 (explicit stdout flush after each write); manual via Task 10 ✓

**Placeholder scan:** Three documented M1 limitations, none disguised as placeholders:
1. Mid-sync Cancel maps to Decision::Review(Quit) — works at Review time only; orchestrator doesn't yet support cooperative cancel. UI force-kills after 5s grace. (Task 3 Step 4)
2. `Summary { metadata_only: 0 }` shortcut — Review event carries the real value via ActionPlanSummary which is what the UI renders. (Task 3 Step 4)
3. CoreNotFoundDialog's "Choose path" button is a status-line acknowledgment; full file picker lands in M2. (Task 8 Step 2)

**Type consistency check:**
- `IpcEvent` Rust variants ↔ `IpcEvent` C# `[JsonDerivedType]` entries — same set: hello, header, summary, review, prompt, form, track_start, track_done, log, error, finish ✓
- `IpcCommand` Rust variants ↔ `IpcCommand` C# `[JsonDerivedType]` entries — same set: start, review_decision, prompt_decision, form_decision, cancel ✓
- `ReviewChoice` Rust + C# — same variants: apply, dry_run, quit ✓
- Field names (snake_case on the wire): every Rust struct uses serde's `rename_all = "snake_case"`; every C# record uses `JsonNamingPolicy.SnakeCaseLower` or explicit `[JsonPropertyName]` for multi-word props ✓
- `protocol_version = "1.0.0"` referenced consistently in `src/ipc.rs::PROTOCOL_VERSION`, `Models/ProtocolVersion.cs::Supported`, and `docs/ipc-protocol.md` compatibility matrix ✓

**Scope check:** Phase 6 M1 only — IPC protocol + Rust backend + minimal WinUI shell. No wizard (M2), no library browser (M3), no MSIX/signing (M4). Does not modify orchestrator, apply_loop, transcoding, manifest, or any other Rust subsystem. The TUI mode stays untouched (TuiBackend dispatch unchanged).

**Staging rules baked in:** Every task lists exact files in its `git add` line. No `-A`, no `.`. Email `19785650+itsmichaelwest@users.noreply.github.com`. Branch `main`.

**Parallelism opportunities:** Task 1 (protocol docs) and Task 4 (C# scaffold) can run in parallel with each other. Task 2 (Rust wire types) and Task 4 are independent. Tasks 3 (Rust backend), 5 (C# IPC client), 6 (ViewModels), 7 (ProgressVM), 8 (path resolution dialog), 9 (integration test), 10 (gate) form a serial chain due to dependencies. Reasonable concurrent dispatch: {Task 1, Task 2, Task 4} as one wave; then {Task 3, Task 5} in sequence; then 6 → 7 → 8 → 9 → 10.
