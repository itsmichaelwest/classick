# Code-size audit

Audit date: 2026-07-19. Generated files, vendored dependencies, build output,
binary assets, device-data tables, lockfiles, and archived documentation are
excluded from maintainability findings.

The repository convention is roughly 500 lines per hand-maintained file. Line
count is a signal rather than a refactor mandate: cohesive state machines may
remain larger, while files with several unrelated responsibilities should be
split first.

## Priority 0

| File | Lines | Recommended boundaries |
| --- | ---: | --- |
| `crates/classick/src/daemon/runtime.rs` | 5,166 | bootstrap/main loop; source recovery; session lifecycle; status/query builders; config, device, playlist, and library command handlers; move inline tests beside their owning modules |
| `crates/classick/src/apply_loop.rs` | 3,027 | orchestration; staged album pipeline; replacement/deferred retry; artwork/backfill; playlist reconciliation |
| `crates/classick/src/ipod/device.rs` | 2,070 | common identity/mount selection plus Windows, macOS, and Linux backends |

`runtime.rs` contains about 3,497 production lines and 1,669 inline test lines;
`handle_client_command` alone is about 1,042 lines. `apply_loop.rs` contains
about 2,316 production lines and a roughly 527-line `run` function. These are
the first refactors to schedule.

## Priority 1

| File | Lines | Recommended boundaries |
| --- | ---: | --- |
| `crates/classick/src/ipc_daemon.rs` | 1,759 | shared payloads, commands, events, and codec tests |
| `crates/classick/src/progress.rs` | 1,305 | event/state model, plain backend, subprocess IPC backend, TUI rendering |
| `crates/classick/src/sync_transaction.rs` | 1,346 | publication, snapshot validation, rollback, cleanup, tests |
| `crates/classick/src/ipod/db.rs` | 1,284 | track CRUD/metadata, reconciliation, playlists, backup/restore, FFI helpers |
| `crates/classick/src/daemon/sync_orchestrator.rs` | 1,123 | command construction, child-wire driver, stop/finalization watchdog, summary decoding |
| `crates/classick/src/transcode.rs` | 993 | probing, classification, platform encoders, artwork extraction, temp paths |
| `ui/macos/Sources/Classick/Model/AppModel.swift` | 883 | domain reducer extensions while retaining one observable model |
| `ui/macos/Sources/Classick/ClassickApp.swift` | 678 | lifecycle, command dispatch, windows, notifications, source recovery |

The Swift wire models are also natural protocol-domain splits:
`DaemonEvent.swift` (759 lines), `DaemonCommand.swift` (734), and
`DaemonClient.swift` (518).

## Oversized tests

- `AppModelReducerTests.swift` — 1,572 lines; split inventory/sync, config,
  playlist, source-recovery, and drag/drop reducers.
- `daemon_multi_device_integration.rs` — 1,171; split targeting/admission,
  persistence, lifecycle, and recovery.
- `WireCodecTests.swift` — 958; split subprocess, daemon-core,
  playlist/device, and source-recovery codecs.
- `playlists_e2e.rs` — 739; split reconciliation, deletion, recovery, and
  projection.
- `DaemonClientTests.swift` — 578; split durable-outbox behavior from socket
  handshake/reconnection.

Moving inline tests to companion test modules also brings several production
files below 500 lines without changing runtime architecture: `daemon/library`,
`selection`, `manifest`, `playlist`, `config`, `config_file`, `manifest_store`,
`device_registry`, `library_index`, `source_availability`, `history`, and `cli`.

## Legitimate large files

Do not split generated or data-oriented files merely to meet the guideline:

- `Cargo.lock`
- `ui/macos/Classick.xcodeproj/project.pbxproj`
- `crates/classick/data/sysinfo-extended/*.plist`
- vendored libgpod headers, libraries, patches, and licences
- binary fixtures and release artefacts

## Execution order

Refactor the three Priority 0 files as separate behavior-preserving plans.
Then address `ipod/db.rs`, `progress.rs`, the wire models, and Swift app state.
Move tests with the module they verify. Avoid a repo-wide mechanical extraction
that creates many context-free `tests.rs` files at once.
