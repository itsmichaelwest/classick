# Portable Manifest and Source Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the mounted iPod's portable manifest authoritative across macOS/Windows and automatically recover unavailable SMB sources without storing credentials or writing to the share.

**Architecture:** Persistence uses a strict portable v2 DTO resolved into the existing absolute-path runtime model. `ManifestStore` loads device-first and publishes device-then-cache. `SourceLocation` separates logical identity from the current mount. `SourceAvailabilityService` coalesces remount attempts behind a platform backend; macOS uses NetFS.

**Tech Stack:** Rust/serde, atomic files, NetFS/CoreFoundation Objective-C shim on macOS, Tokio, existing daemon IPC, Swift 6 for the minimal auth-required handoff.

## Global Constraints

- Depends on Plan 1's registry/serial targeting. Preserve legacy config and Windows wire compatibility.
- Never serialize a native `PathBuf` in manifest v2; keep absolute paths only in the runtime `Manifest`.
- Never log/store SMB credentials or write to the source. Use the mountpoint returned by NetFS, not a guessed `/Volumes` name.
- Plan 3 will consume `ManifestStore::publish`; do not duplicate checkpoint ordering here.

---

### Task 1: Portable paths and logical source configuration

**Files:** Create `crates/classick/src/portable_path.rs`, `source_location.rs`; modify `lib.rs`, `playlist.rs`, `config_file.rs`, `config.rs`, `crates/classick/tests/fixtures/sample-config.toml`.

```rust
impl PortablePath {
    pub fn parse(value: &str) -> Result<Self>;
    pub fn from_absolute(root: &Path, path: &Path) -> Result<Self>;
    pub fn resolve(&self, root: &Path) -> PathBuf;
}
pub enum SourceIdentity { Smb { host: String, share: String, subpath: Option<PortablePath> }, Local { library_id: String } }
pub struct SourceLocation { pub resolved_path: PathBuf, pub identity: SourceIdentity }
```

Reject empty/absolute/UNC/drive-prefixed/backslash/dot/dot-dot/empty-component portable paths. Normalize SMB logical comparison case-insensitively. Keep legacy `PersistedConfig.source` and add optional `source_location`; new saves keep both synchronized.

- [ ] Add RED tests for mac/Windows rebasing, hostile paths, equivalent Jupiter SMB identities, mismatches, local ID persistence, and old/new TOML round trips.
- [ ] Implement without silently tightening tolerant `.m3u8` parsing; run `cargo test -p classick portable_path`, `cargo test -p classick source_location`, and `cargo test -p classick config_file` GREEN.
- [ ] Commit: `git commit -m "feat(manifest): add portable source identities"`.

### Task 2: Manifest v2 DTO and authority store

**Files:** Create `crates/classick/src/manifest_store.rs`, `atomic_file.rs`; modify `manifest.rs`, `device_state.rs`, `lib.rs`; add v1/v2 fixtures.

```rust
impl Manifest {
    pub fn encode_v2(&self, source: &SourceLocation, serial: &str) -> Result<Vec<u8>>;
    pub fn decode_v2(bytes: &[u8], current_root: &Path) -> Result<Self>;
}
pub enum ManifestOrigin { DeviceV2, HostV2, HostV1, LegacyV1, Missing }
pub struct LoadedManifest { pub manifest: Manifest, pub origin: ManifestOrigin, pub needs_device_publish: bool }
impl ManifestStore {
    pub fn new(mount: PathBuf, serial: String, host_cache: PathBuf, legacy_flat: PathBuf, atomic_writer: AtomicFileWriter) -> Self;
    pub fn load(&self, source: &SourceLocation) -> Result<LoadedManifest>;
    pub fn publish(&self, manifest: &Manifest, source: &SourceLocation) -> Result<ManifestPublishOutcome>;
}
pub struct ManifestPublishOutcome { pub device_validated: bool, pub host_cache_warning: Option<String> }
```

Authority order when the device manifest is missing is host per-device v2/v1, legacy flat v1, then rebuild. A present but invalid connected-device manifest is `InvalidDevice`: mutating apply fails closed until reconciliation rebuilds from the live iTunesDB; it must never silently use a stale host cache. Outside-root v1 entries become source-unknown. Never delete migration inputs. Required device atomic replace precedes validated warning-only host cache; use replace-existing/write-through semantics on Windows and rename plus best-effort parent fsync on POSIX.

- [ ] Add RED tests for precedence, missing-device migration, invalid connected authority blocking mutating apply despite a stale cache, read-only reconciliation from live DB, v1 relativization, cross-OS rebase, serial mismatch, hostile relpaths, publication failures, and retained v1 artifacts.
- [ ] Implement and run `cargo test -p classick manifest manifest_store` GREEN.
- [ ] Commit: `git commit -m "feat(manifest): make device manifest v2 authoritative"`.

### Task 3: Route every manifest consumer through `ManifestStore`

**Files:** Modify `crates/classick/src/apply_loop.rs`, `art_audit.rs`, `daemon/runtime.rs`, `daemon/library.rs`, `crates/classick/tests/playlists_e2e.rs`, `device_playlists_integration.rs`, `wipe_all_tracks_integration.rs`, `fit_retry_integration.rs`.

Connected sync/preview/count/audit use mounted authority; disconnected display uses host cache. Source safeguard compares logical identity before legacy root. Replace-library publishes DB then empty authoritative manifest. Remove all remaining preview reads of `default_manifest_path()`.

- [ ] Add RED integration tests for equal SMB identity at alternate mounts, different share safeguard, device-before-cache checkpoint, connected/device authority, disconnected/cache, and serial-correct replace/backfill/audit.
- [ ] Implement carefully around dirty files; run `cargo test -p classick --test playlists_e2e`, `cargo test -p classick --test device_playlists_integration`, `cargo test -p classick --test wipe_all_tracks_integration`, `cargo test -p classick --test fit_retry_integration`, then `cargo test -p classick` GREEN.
- [ ] Commit: `git commit -m "refactor(manifest): route sync state through device authority"`.

### Task 4: NetFS backend and source-availability service

**Files:** Create `daemon/source_availability.rs`, `daemon/macos_netfs.rs`, `daemon/netfs_shim.m`; modify `daemon/mod.rs`, `lib.rs`, `build.rs`, crate manifest/lock.

```rust
pub enum MountInteraction { SuppressUi, AllowUi }
pub struct ResolvedSource { pub root: PathBuf, pub remounted: bool }
pub enum SourceUnavailable { AuthRequired, MountFailed(String), MissingSubpath(PathBuf) }
pub trait SourceMountBackend { fn mount(&self, location: &SourceLocation, interaction: MountInteraction) -> BoxFuture<'_, Result<PathBuf, SourceUnavailable>>; }
```

Coalesce by logical identity. NetFS uses no username/password arguments, chooses returned candidate mountpoint, appends subpath, and may request read-only mount options. Non-mac local path is an existence fast path; Windows relies on established OS sessions.

- [ ] Add fake-backend RED tests for local no-op, same-source coalescing, distinct identities, `/Volumes/data-1`, auth retry, missing subpath, and credential-free diagnostics.
- [ ] Implement/link macOS-only shim and run `cargo test -p classick source_availability` GREEN.
- [ ] Commit: `git commit -m "feat(daemon): remount unavailable SMB sources on macOS"`.

### Task 5: Recovery flow, watcher re-arm, and UI handoff

**Files:** Modify `daemon/runtime.rs`, `sync_orchestrator.rs`, `library_watcher.rs`, `scan.rs`, `orchestrator.rs`, `preflight.rs`, `ipc_daemon.rs`, `docs/ipc-protocol.md`, `ui/macos/Sources/Classick/Ipc/DaemonEvent.swift`, `ui/macos/Sources/Classick/Ipc/DaemonCommand.swift`, `ui/macos/Sources/Classick/Model/AppModel.swift`, `ui/macos/Sources/Classick/Views/LibraryView.swift`, `ui/macos/Sources/Classick/Views/MenuContent.swift`, `WireCodecTests.swift`, and `AppModelReducerTests.swift`.

Add `source_availability` event with `available|remounting|auth_required|unavailable` and `retry_source_mount {allow_ui:true}`. `PendingSourceAction` coalesces triggers. On success persist both resolved fields, rewatch once, clear debounce, and scan once. `LibraryView` and `MenuContent` show “Music share needs attention” with a “Connect” action; while the app is inactive the action remains visible but does not send `allow_ui:true`. On activation/click it sends one explicit retry. Failure retains cached index/count. Add macOS 15-compatible state previews.

- [ ] Add RED tests for alternate mount persistence/re-arm/one scan, burst scan+sync coalescing with admission, auth gating, local immediate start, and failure preserving cached state; add Swift codec/reducer tests.
- [ ] Implement, run focused daemon integrations with one test thread, then full Rust/Swift tests and macOS-15-floor build.
- [ ] Commit: `git commit -m "feat(daemon): recover and rewatch source shares"`.
