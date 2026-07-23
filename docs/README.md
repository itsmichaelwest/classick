# Classick documentation

This directory contains the current reference material for Classick. Documents
under `archive/` are historical context only and may contradict the shipped
implementation.

## Current references

- [Architecture](architecture.md) — components, state ownership, data flow, and
  cross-platform boundaries.
- [IPC protocol](ipc-protocol.md) — normative entry point for both JSON wire
  protocols.
- [Device safety](device-safety.md) — invariants for iTunesDB, artwork,
  manifests, playlists, recovery, and source-library access.
- [Device coordination architecture](device-coordination.md) — implemented
  standard-user locking, external-writer fencing, recovery, and the boundary
  for later device-family synchronization profiles.
- [Device transcode profile research](research/2026-07-23-device-transcode-presets.md)
  — Apple format support, historical iTunes controls, community evidence, and
  the rationale for the V1 ALAC/AAC profile set.
- [Code-size audit](code-size-audit.md) — current large-file hotspots and
  recommended refactor order.
- [Project learnings](../LEARNINGS.md) — concise operational gotchas that save
  time during implementation and debugging.

## Implemented design and delivery record

- [Native device protocol and identity](design/2026-07-19-native-device-protocol.md)
  — implemented design for wire unification, portable device identity/state,
  initialization boundaries, and stable `SysInfoExtended` generation.
- [Native device protocol implementation plan](plans/2026-07-19-native-device-protocol.md)
  — staged code, test, review, and physical-device work needed to reach that
  target, with completed component plans for the
  [Rust core](plans/2026-07-19-native-device-protocol-rust.md),
  [Windows UI](plans/2026-07-19-native-device-protocol-windows.md), and
  [macOS UI](plans/2026-07-19-native-device-protocol-macos.md).

These documents record the decisions and delivery sequence behind the current
native device protocol. Deferred initialization, library identity, additional
device families, and physical-fixture expansion remain called out explicitly.

## Authority

The Rust wire types are the serialization authority. The protocol docs define
the compatibility and behavioral contract that every client must implement.
When code and documentation disagree, treat that as a defect: reconcile both
rather than silently documenting one implementation's divergence.

Current wire implementations:

- shared protocol: `crates/classick/src/wire/`
- daemon transport: `crates/classick/src/daemon/ipc_server.rs`
- worker transport: `crates/classick/src/worker_wire.rs`
- Windows: `ui/windows/Classick.UI.Core/Ipc/`
- macOS: `ui/macos/Sources/Classick/Ipc/`

## Historical material

Completed task plans and obsolete reviews were removed because Git already
preserves them and their unchecked task lists looked like active work. Selected
design records, the original specification, SCSI research, the former protocol
changelog, and the full chronological learnings log remain under
[`archive/`](archive/README.md).
