# Daemon Lifecycle and Ordered IPC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Guarantee one reachable daemon, drain it on quit/signal/UI death, preserve ordered event delivery, and retain only the latest durable intent across reconnects.

**Architecture:** Unix listener ownership is an atomic lock-backed lease with inode-safe cleanup. All shutdown sources feed one Rust drain path. Swift reads through one sequential stream, coalesces durable intents by logical key, and uses AppKit `.terminateLater` to wait for graceful daemon EOF before an owned-process fallback.

**Tech Stack:** Rust/Tokio Unix sockets/signals, Swift actors/Darwin I/O, AppKit lifecycle, XCTest.

## Global Constraints

- Preserve Plan 1 serial/request fields and Plan 3 finalization drain. Shutdown is not a second cancellation implementation.
- Never unlink a socket before acquiring exclusive ownership. Never remove the stable lock file.
- A socket write is transport success, not persistence acknowledgement. Durable intents stay in-flight until a matching request ID/revision acknowledgement; Plan 5 owns canonical editor reconciliation.
- All behavior must work on macOS 15.

---

### Task 1: Exclusive socket ownership and unique logs

**Files:** Create `crates/classick/src/daemon/unix_socket.rs`; modify `daemon/mod.rs`, `daemon/ipc_server.rs`, `logging.rs`.

```rust
pub(crate) struct UnixSocketLease { lock_file: File, socket_path: PathBuf, device: u64, inode: u64 }
impl UnixSocketLease {
    pub(crate) fn bind(path: &Path) -> Result<(tokio::net::UnixListener, Self)>;
}
fn open_unique_ipc_log_file_in(base: &Path, now: SystemTime, pid: u32) -> io::Result<File>;
```

Acquire `flock(LOCK_EX|LOCK_NB)` on the stable sibling `.lock` first. Only its owner may remove a stale socket and bind. Move the lease with the accept task. Drop removes only its recorded socket inode. Logs use `create_new` names `core-{secs}-{nanos:09}-{pid}.log`, retrying suffix collisions.

- [ ] Add RED tests for second-lease safety, own/replacement inode cleanup, PID/subsecond names, and no truncation.
- [ ] Implement and run `cargo test -p classick daemon::unix_socket` plus logging tests GREEN.
- [ ] Commit: `git commit -m "fix(daemon): enforce exclusive socket ownership"`.

### Task 2: Unified shutdown, signals, and parent-death lease

**Files:** Create `daemon/lifecycle.rs`; modify `daemon/mod.rs`, `runtime.rs`, `cli.rs`, `main.rs`, crate manifest, and `DaemonDeps` test constructors.

```rust
pub enum ShutdownReason { Client, Signal, ParentDeath }
pub fn spawn_shutdown_monitor(parent_pid: Option<u32>) -> mpsc::UnboundedReceiver<ShutdownReason>;
// CLI: --daemon-parent-pid <PID>, hidden and requiring --daemon
```

SIGINT/SIGTERM and parent mismatch feed the same runtime select and existing bounded cancel/drain. A manually launched daemon without a parent PID has no parent lease. Inject `shutdown_rx` through `DaemonDeps` for deterministic tests.

- [ ] Add RED tests for current/dead parent plus injected signal/parent-death during fake sync using the same drain path; retain client-shutdown coverage.
- [ ] Implement with Tokio signal feature and run lifecycle plus daemon integration tests one-threaded GREEN.
- [ ] Commit: `git commit -m "fix(daemon): drain on signals and parent death"`.

### Task 3: Sequential events and durable intent coalescing

**Files:** Modify `ui/macos/Sources/Classick/Ipc/DaemonClient.swift`, `ui/macos/Sources/Classick/Ipc/DaemonCommand.swift`, `ui/macos/Sources/Classick/Ipc/DaemonEvent.swift`, `ui/macos/Tests/ClassickTests/DaemonClientTests.swift`.

```swift
enum SendDisposition { case sent, queued, dropped }
enum DurableIntentKey: Hashable { case config, selection, deviceConfig(serial: String), playlist(String), deviceRemoval(serial: String) }
@discardableResult func send(_ command: DaemonCommand) async -> SendDisposition
```

One blocking reader produces `AsyncStream<Data>`; one actor-isolated loop decodes/yields in order. Upsert durable commands by key, moving the newest to the tail. Failed writes enqueue before disconnect. Flush after handshake from the front; a full bytes-plus-newline write marks an intent in-flight, and only a matching authoritative request ID/revision acknowledgement removes it. On reconnect, resend the latest unacknowledged intent. Retry EINTR. Reads/session controls/shutdown are not durable.

- [ ] Add RED tests for a 100-event exact-order burst, same-key coalescing, cross-key chronology, failed flush retention, crash-after-write resend, and exactly-once removal after acknowledgement.
- [ ] Implement and run `swift test --filter DaemonClientTests` GREEN.
- [ ] Commit: `git commit -m "fix(ipc): serialize events and preserve durable intents"`.

### Task 4: AppKit terminate-later handshake

**Files:** Create `ui/macos/Sources/Classick/Daemon/DaemonShutdownCoordinator.swift`, `ui/macos/Tests/ClassickTests/DaemonShutdownCoordinatorTests.swift`; modify `ui/macos/Sources/Classick/Daemon/DaemonProcess.swift`, `ui/macos/Sources/Classick/ClassickApp.swift`, `ui/macos/Sources/Classick/Ipc/DaemonClient.swift`, `DaemonCommand.swift`, `ui/macos/Tests/ClassickTests/WireCodecTests.swift`, `DaemonClientTests.swift`, `ui/macos/Classick.xcodeproj/project.pbxproj`.

```swift
actor DaemonClient { func shutdownAndWait(timeout: Duration) async -> Bool }
@MainActor final class DaemonShutdownCoordinator {
    func begin(shutdown: @escaping @Sendable () async -> Bool,
               forceTerminateOwnedDaemon: @escaping @MainActor () -> Void,
               reply: @escaping @MainActor (Bool) -> Void) -> NSApplication.TerminateReply
}
```

Encode exactly `{"type":"shutdown"}`. First quit returns `.terminateLater`, disables reconnect, sends once, and observes daemon snapshot/EOF. A progressing Plan 3 finalization extends the wait; the app must never hard-kill it. Fallback is permitted only after 120 seconds without finalization progress plus a 10-second shutdown margin, and only for the spawned process. Spawn with `--daemon-parent-pid`. `applicationWillTerminate` only closes/cancels UI work. For “newest app wins,” wait for old-app termination/socket disappearance before ensure-running so the replacement cannot attach to a dying daemon.

- [ ] Add RED wire/client/coordinator tests for EOF success, bounded failure, one reply, one fallback, repeated begin, attached daemon shutdown, and fast relaunch.
- [ ] Implement and run focused then full Swift tests GREEN.
- [ ] Run full Rust/Swift/macOS-15 build gates.
- [ ] Commit: `git commit -m "fix(ui): drain the daemon before app termination"`.
