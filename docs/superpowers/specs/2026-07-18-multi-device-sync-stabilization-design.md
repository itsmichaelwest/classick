# Multi-device Sync Stabilization Design

**Status:** approved architecture, pending written-spec review

**Date:** 2026-07-18

**Scope:** Rust daemon, sync/apply pipeline, portable device state, macOS IPC
and SwiftUI state, device-row presentation, SMB source recovery, and daemon
lifecycle. The Windows UI remains wire-compatible but does not receive a
visual redesign in this work.

## 1. Problem statement

Classick has gained per-device settings, selections, manifests, and playlists,
but the runtime and macOS app still model the connected iPod as one global
optional value. That mismatch causes a second iPod to replace the first one in
the sidebar, lets global sync history and progress appear on the wrong device,
and makes several command paths target whichever volume happens to occupy the
singleton slot.

The same global-state pattern affects sync completion. The subprocess emits a
finish before the daemon has appended history and recomputed counts, the UI
immediately exposes stale values, and a syncing status with `last_sync: null`
temporarily turns a real timestamp into “Never synced.” Error and cancellation
states are similarly overwritten by the next idle status.

Artwork loss has a separate but interacting cause. A parsed libgpod database
write drops thumbnail links for pre-existing tracks unless every retained
track's thumbnails are rehydrated before the write. The 2026-07-18 cancelled
sync rewrote the DB after 417 actions, returned `Completed` rather than a
distinct cancelled outcome, began a whole-library artwork repair, and was
force-killed by the daemon five seconds later. The six reported albums are the
95 pre-existing tracks affected by that sequence: their source artwork is
valid, but their on-device `mhii_link` is zero and thumbnail pointer is null.

This design replaces the singleton and event-order assumptions, makes DB,
artwork, and portable-manifest publication one coordinated checkpoint, and
gives cancellation an explicit finalization phase.

## 2. Product decisions

- Classick supports any number of remembered iPods and any number of detected
  mounted iPods in its inventory.
- Only one sync session is admitted at a time in this release. The admission
  policy is isolated from the per-device session state so it can later become
  “one active session per serial” without changing persistence, IPC payloads,
  views, or the apply pipeline.
- Every mutating or device-specific command names a serial. “Whichever iPod is
  connected” is not a valid command target.
- A known disconnected device remains visible in the sidebar. Attaching an
  unconfigured device adds another row; it never replaces a known row.
- “Last synced” means the latest successful sync for that device. Failed,
  cancelled, and paused attempts are retained as attempts but do not replace
  that timestamp.
- Cancelling stops admission of new albums, drains the bounded in-flight album,
  then visibly finalizes a coherent checkpoint. The UI says that Classick is
  finishing and that the iPod must remain connected.
- The user may close a window during sync, but explicit app quit gracefully
  shuts down the owned daemon and drains or terminates its active sync through
  the same cancellation path.
- The music share is read-only to Classick. Automatic remount never writes to
  the share and never persists credentials.
- macOS 15 remains the deployment floor. macOS 27 is the primary live test
  host, not a new runtime requirement.

## 3. Device inventory and future concurrency

### 3.1 Durable registry

The host stores a registry keyed by normalized serial. Each entry contains:

- serial;
- user-visible name and model label;
- model number/icon family when known;
- last-seen timestamp;
- the existing per-device paths for settings, selection, subscriptions,
  manifest cache, and managed-playlist metadata.

The existing single `ipod_identity` config value migrates into the registry on
first load and remains readable only for backward compatibility. Detecting a
new iPod upserts identity metadata but does not configure its selection or
auto-sync implicitly.

### 3.2 Live inventory

Device discovery returns a collection rather than `Option<DetectedIpod>`.
Watchers diff the previous and current maps and emit per-serial additions,
updates, and removals. Removal of serial A cannot clear serial B.

The daemon holds `HashMap<Serial, ConnectedDevice>` plus a separate sync
admission controller. In this release the controller has capacity one. A
future concurrent implementation changes that controller to a keyed session
map while reusing the same `SyncSession { serial, drive, session_id, state }`
objects and serial-targeted commands.

### 3.3 Snapshot-oriented IPC

The daemon sends full inventory/status snapshots after handshake and after any
device or session change. Edge events may remain as additive hints, but clients
do not reconstruct authoritative inventory from them.

Each device snapshot includes:

- durable identity;
- connected state and current mount when connected;
- per-device phase and active session ID;
- storage and synced/library counts;
- latest successful sync and latest attempt;
- last terminal error or cancellation summary;
- per-device selection/settings/subscriptions revision.

Commands for sync, preview, configuration, replace, backfill, eject, and forget
carry `serial`; request/reply operations also carry a `request_id`. Replies echo
both fields. This removes the Swift FIFO correlation queues.

## 4. Portable device state

### 4.1 Manifest v2

`/iPod_Control/classick/manifest.json` becomes the mounted device's authority.
`~/Library/Application Support/classick/devices/<serial>/manifest.json` remains
a host cache for disconnected display and recovery, not the authority during a
connected sync.

Manifest v2 keys source tracks by a validated, `/`-separated path relative to
the configured library root. It rejects absolute paths, drive/UNC prefixes,
empty components, and `.` or `..`. Runtime code resolves a relative key under
the host's current source root. Native absolute `PathBuf` values are never
persisted in v2, so the same device manifest can resolve beneath both
`/Volumes/data/media/music` and `\\jupiter\data\media\music`.

Migration precedence is:

1. valid device manifest v2;
2. host per-device v1, relativized against its recorded source root;
3. legacy flat host v1;
4. rebuild from the iTunesDB.

Migration never deletes or overwrites the source artifact until the v2 device
manifest and host cache have both been validated. Entries outside the recorded
root are preserved as source-unknown rather than converted unsafely.

### 4.2 Portable source identity

The manifest records an optional logical source identity separate from a host
mount path. For SMB this is host, share, and relative subpath, normalized
case-insensitively for comparison. It contains no username or password. Local
folders may use an opaque library ID created by Classick.

The source-change safeguard compares logical identity first. A different mount
path for the same SMB share is not a destructive library change.

### 4.3 Publication ordering

At every successful checkpoint:

1. publish the iTunesDB and ArtworkDB coherently;
2. atomically write the device manifest;
3. refresh the host manifest cache best-effort;
4. update playlist/subscription mirrors best-effort;
5. remove the pending-session journal and obsolete orphan files.

A DB/artwork failure does not advance either manifest. A device-manifest
failure fails finalization and leaves the pending journal for recovery. A host
cache or playlist-mirror failure is warning-only because the device remains the
authority.

## 5. Artwork-safe sync transaction

### 5.1 Why “art first, then tracks” is not literal

iPod artwork records are linked to iTunesDB track records, so an ArtworkDB
cannot safely publish artwork for a track that does not yet exist. Classick
instead stages audio and thumbnail inputs independently, then publishes the
artwork links before the DB checkpoint becomes visible. From the device's
perspective, tracks and their art appear together.

### 5.2 Prepare and stage

The planner groups mutating actions by album and creates an on-device pending
session journal under `iPod_Control/classick/pending/`. Add and Modify actions
transcode in parallel as today, but the single committer stages new files and
records their intended DB entries without deleting the currently-published
track/file. Remove actions similarly record deferred cleanup rather than
deleting the published file before commit.

The journal is updated atomically at album boundaries and contains enough
identity to distinguish staged files from foreign files. After a crash or
unplug, reconciliation can remove uncommitted staged files safely while the
last published DB and manifest remain authoritative. Normal cancellation does
not discard staged work; it proceeds to finalization.

### 5.3 Artwork cache and checkpoint

During prepare/staging, Classick extracts and normalizes one image per distinct
artwork hash into a host cache. The source library remains untouched. Before
any `itdb_write`, the checkpoint coordinator:

1. applies the staged Add/Modify/Remove records to the in-memory DB;
2. rehydrates thumbnails for every retained source-known track from the
   normalized cache, loading missing cache entries from the source as needed;
3. removes stale ArtworkDB/ithmb output only after all required thumbnail
   inputs have been prepared successfully;
4. performs one libgpod write that publishes DB and artwork links together;
5. reopens and verifies that each track expected to have art has a nonzero
   artwork link, a non-null thumbnail, and a decodable thumbnail;
6. publishes manifest v2 using the ordering in section 4.3;
7. removes files that became unreferenced only after the new DB is durable.

The existing whole-library repair-after-commit flow is removed. A checkpoint
never deliberately publishes a DB known to have lost the prior artwork and
then hopes a second pass repairs it.

Checkpoint frequency becomes album-boundary and time based. The coordinator
may coalesce multiple albums to avoid repeatedly rewriting a global ArtworkDB,
but any checkpoint it does publish must satisfy the full artwork invariant.
The existing DB backup remains the rollback source if libgpod fails during
publication.

### 5.4 Cancellation and pause

Cancel and pause are distinct terminal requests:

- stop accepting new albums;
- drain only the bounded work already admitted for the current album;
- emit `finalizing` with the session ID, reason, and committed/staged counts;
- run the same artwork-safe checkpoint;
- emit `cancelled` or `paused` only after verification and manifest
  publication complete.

The daemon does not start a force-kill timer merely because the child is
coherently finalizing. It watches explicit finalization progress and applies a
larger bounded emergency timeout only when progress has stopped. Emergency
termination preserves the journal and dirty marker, reports an interrupted
finalization, and never reports the run as completed.

## 6. Source availability and SMB remount

Host configuration stores a `SourceLocation` containing the last resolved path
and, for SMB, the logical host/share/subpath. On macOS, `ensure_source_available`
uses Apple's NetFS framework, which predates the macOS 15 floor:

1. if the resolved path exists, return it immediately;
2. coalesce concurrent requests for the same source;
3. attempt `NetFSMountURLAsync` without UI, relying on macOS/Keychain;
4. if interaction is required, send a UI-required state and retry with UI only
   while Classick is active;
5. use the mountpoint returned by NetFS, including `/Volumes/data-1`-style
   collision names, then append the stored subpath;
6. re-arm the library watcher and refresh the index once.

Credentials never enter config, command arguments, logs, or IPC. Classick does
not automatically unmount shares.

## 7. Daemon lifecycle

The macOS app and daemon have one explicit ownership protocol:

- daemon startup uses an atomic process lock; a second daemon cannot unlink a
  live daemon's socket;
- a socket guard removes only the socket inode created by its server;
- the Swift client encodes `shutdown`;
- AppKit termination uses `applicationShouldTerminate` and
  `.terminateLater`, sends shutdown, waits for the daemon's drained exit, and
  replies to AppKit within a bounded deadline;
- SIGTERM/SIGINT enter the same Rust shutdown path;
- a parent-death lease lets the daemon detect UI crash/SIGKILL and perform the
  same bounded drain;
- attaching to an existing daemon does not waive shutdown responsibility;
- log filenames include PID and subsecond uniqueness and never truncate a
  sibling daemon/sync log.

## 8. Ordered state delivery and macOS model

`DaemonClient` has one sequential read/decode/yield loop. It does not create an
unstructured task per line. Writes return success/failure; durable intents are
coalesced by logical key and removed only after a complete successful write.
Reconnect resends current intent, not every intermediate UI draft.

`AppModel` stores `[Serial: DeviceViewState]`. Each device owns identity,
connection, counts, storage, latest successful sync, latest attempt, phase,
rollups, error, config, and preview. Global library and playlist definitions
remain global. Sidebar selection remains serial-keyed.

Inner subprocess `finish` events record rollups but do not transition the UI to
idle. The daemon's post-publication device snapshot performs the atomic phase,
counts, storage, history, and timestamp transition. Failures remain visible
until retry, dismissal, disconnect, or a later successful session.

Draft editors use acknowledged intents rather than permanent `userEdited`
latches. A draft may remain dirty while its save is pending, but it reconciles
to the daemon's canonical revision after acknowledgement. Programmatic seeding
never schedules a write.

Deleting a playlist atomically removes its slug from every device subscription
file before broadcasting the new playlist/device snapshots. The next sync
removes the managed device playlist normally; deletion is not reported as an
unresolved subscription.

## 9. Device-row and menu-bar presentation

All device-row phases derive a pure `DeviceRowPresentation` from one device
snapshot and render through one fixed shell:

- 40-point cached per-device artwork;
- title and one secondary subtitle;
- a stable trailing action column using large controls;
- a reserved second-row meter for capacity, progress, or unavailable state;
- one stable caption slot for warnings/errors.

Idle, syncing, finalizing cancellation, paused, scanning, disconnected,
unconfigured, error, and no-known-device states share the same geometry.
Finalizing cancellation says “Finishing sync…” and “Keep the iPod connected”
with indeterminate or commit-stage progress. Error retains device identity and
offers Details plus Retry.

The menu-bar extra uses a custom label with a monochrome, medium-weight symbol
in a fixed 18-by-18-point optical frame and an accessibility label. State
changes do not alter the status item's footprint.

All controls and layout primitives are available on macOS 15. Newer material
effects remain availability-gated; the macOS 15 fallback is verified in a macOS
15 VM because a macOS 27 host cannot render that branch.

## 10. Error handling and recovery invariants

- A command never mutates a device other than its explicit serial.
- A disconnect event removes only its matching serial.
- The last durable DB and device manifest always describe the same published
  track set.
- Existing track files are not deleted before the DB that stops referencing
  them is durable.
- A checkpoint is successful only if its expected artwork links decode.
- Cancellation is never represented as completion.
- Loss of the UI, daemon, source share, or USB mount leaves a journal/backup
  sufficient to identify and reconcile Classick-owned incomplete work.
- Foreign playlists and files are never inferred from names and never deleted.
- Host-cache, UI, and mirror failures cannot silently advance device truth.

## 11. Verification

Automated coverage must include:

- discovery snapshots containing devices A and B; removing A preserves B;
- remembered A plus newly attached unconfigured B in the Swift sidebar;
- serial-targeted sync/replace/backfill rejection on mismatch;
- one-session admission today and independent session objects suitable for a
  future per-serial admission map;
- per-device latest-successful versus latest-attempt history;
- manifest v1-to-v2 migration, Windows/macOS root rebasing, validation, and
  device-authority precedence;
- DB/artwork/manifest failure injection at every publication boundary;
- parsed DB with pre-existing artwork plus new tracks, followed by cancellation;
- false-positive audit fixture with `has_artwork=1` but null thumbnail link;
- finalizing-cancellation UI sequencing and emergency-timeout behavior;
- sequential burst delivery, request-ID correlation, durable-write reconnect,
  and acknowledged-draft reconciliation;
- playlist deletion scrubbing subscriptions for multiple devices;
- SMB remount, alternate returned mountpoint, auth-required handoff, request
  coalescing, watcher re-arm, and local-folder no-op;
- duplicate daemon startup, stale socket cleanup, normal quit, quit during
  sync, and parent-death cleanup;
- `DeviceRowPresentation` truth table for every phase and long strings;
- full Rust, Swift, and Xcode builds with deployment target 15.0.

Live gates use the mounted iPod and the configured SMB share read-only. Device
writes are exercised only through the new coordinated transaction and are
followed by a read-only artwork audit plus physical eject/boot/playback checks.
The source share is never written.

## 12. Delivery sequence

Implementation is split into independently reviewable plans, in this order:

1. device registry, serial-targeted snapshots/commands, and Swift keyed state;
2. manifest v2 device authority and source-location/remount support;
3. artwork-safe transaction, cancellation finalization, and stronger audit;
4. daemon lifecycle and ordered/durable IPC hardening;
5. playlist cleanup, draft reconciliation, device-row consolidation, and full
   integration/visual verification.

The wire changes remain additive within the daemon protocol while old commands
are retained during the Windows compatibility window. Old unscoped mutating
commands are rejected when targeting would be ambiguous; they are removed only
in a future major protocol release.
