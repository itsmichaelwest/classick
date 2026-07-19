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
- [Device coordination architecture](device-coordination.md) — proposed
  standard-user locking, external-writer fencing, recovery, and per-family
  synchronization profiles for future implementation.
- [Code-size audit](code-size-audit.md) — current large-file hotspots and
  recommended refactor order.
- [Project learnings](../LEARNINGS.md) — concise operational gotchas that save
  time during implementation and debugging.

## Authority

The Rust wire types are the serialization authority. The protocol docs define
the compatibility and behavioral contract that every client must implement.
When code and documentation disagree, treat that as a defect: reconcile both
rather than silently documenting one implementation's divergence.

Current wire implementations:

- subprocess: `crates/classick/src/ipc.rs`
- daemon: `crates/classick/src/ipc_daemon.rs`
- Windows: `ui/windows/Classick.UI.Core/Ipc/`
- macOS: `ui/macos/Sources/Classick/Ipc/`

## Historical material

Completed task plans and obsolete reviews were removed because Git already
preserves them and their unchecked task lists looked like active work. Selected
design records, the original specification, SCSI research, the former protocol
changelog, and the full chronological learnings log remain under
[`archive/`](archive/README.md).
