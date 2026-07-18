# Native macOS Library Drag-and-Drop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a macOS user copy an artist, album, or genre from the Library onto one explicit configured iPod or editable manual playlist, with daemon-owned atomic persistence, request-correlated feedback, and optional sync-after-drop.

**Architecture:** SwiftUI exports a launch-scoped, versioned `LibraryDragPayload` through a Classick UTType and uses only native `.draggable` and `.dropDestination` modifiers. The Swift client validates the payload, sends one serial- or slug-targeted additive command through Plan 4's ordered durable queue, and reconciles Plan 5 drafts from authoritative revision acknowledgements. The Rust daemon resolves rules against its cached `LibraryIndex`, journals the additive store mutation plus its idempotency result, and decides whether an acknowledged device mutation may start the existing serial-targeted sync.

**Tech Stack:** Rust stable, serde JSON, Tokio daemon IPC, Swift 6 strict concurrency, SwiftUI, CoreTransferable, UniformTypeIdentifiers, XCTest, macOS 15 deployment floor.

## Global Constraints

- Execute only after [Plan 1](2026-07-18-device-registry-state.md), [Plan 4](2026-07-18-daemon-lifecycle-ordered-ipc.md), [Plan 5](2026-07-18-macos-state-ui-stabilization.md), [Plan 6A](2026-07-18-ipod-playlist-integrity.md), and [Plan 6B](2026-07-18-rockbox-playlist-projection.md) are complete and green. Reuse their serial-keyed state, ordered durable transport, acknowledged drafts, device-authoritative managed-playlist ownership, verified ordered membership, and recoverable Apple/Rockbox publication; never restore FIFO reply correlation or client-side read/modify/write mutations.
- Preserve the execution-index constraints: keep the source share read-only, preserve unrelated dirty work, stage named files only, and run Rust/Swift processes sequentially when they may share daemon/socket state.
- macOS 15 is the deployment floor. `Transferable`, `.draggable(_:preview:)`, and `.dropDestination(for:action:isTargeted:)` are the correctness path. Gate newer materials with `if #available(macOS 26, *)`; drag/drop must remain complete with macOS 15 materials.
- A drag is a copy. It never removes, reorders, or edits the Library source, never contains a host/device path or target identifier, and never changes selection merely by hovering.
- Use native drag sessions, copy/unavailable cursors, previews, scrolling, focus, and accessibility. Do not add mouse tracking, global event monitors, floating windows, custom drag sessions, permanent dashed boxes, oversized hit overlays, auto-navigation, or layout-shifting chrome.
- Exactly one directly hovered valid destination receives a transient system-accent treatment. No other row/card highlights; rejected targets register no drop destination and use the system snap-back behavior.
- Device drops target one configured raw serial. Manual-playlist drops target one existing, parseable manual slug. Never infer a device, use an aggregate card, or mutate smart/corrupt/missing playlists.
- Daemon persistence is authoritative. A socket write is not success. Show success only from the event carrying the matching `acknowledged_request_id` and canonical revision after the journaled mutation is durable.
- `drop_sync_behavior` is a global config enum `immediate | next_sync`, defaults to `immediate`, and is saved through Plan 5's acknowledged global-settings draft.
- `immediate` starts no hidden follow-up: connected+idle+missing may start now; busy, paused, finalizing, disconnected, or `next_sync` persists for next sync; zero missing tracks never starts a no-op sync.
- Source files and new implementation files stay below roughly 500 lines. Split wire, mutation algebra, drag payload, destination modifier, and feedback state on the boundaries below.
- Add each regression test first, run the exact focused command, and observe the stated RED before implementing. Do not weaken an assertion just to obtain GREEN.
- Apple interaction authorities: [HIG drag and drop](https://developer.apple.com/design/human-interface-guidelines/drag-and-drop), [SwiftUI drag/drop sample](https://developer.apple.com/documentation/swiftui/adopting-drag-and-drop-using-swiftui), [Core Transferable](https://developer.apple.com/documentation/coretransferable), [`draggable(_:preview:)`](https://developer.apple.com/documentation/swiftui/view/draggable(_:preview:)), and [`dropDestination(for:action:isTargeted:)`](https://developer.apple.com/documentation/swiftui/view/dropdestination(for:action:istargeted:)).

## Cross-plan interfaces

| Dependency | Plan 6C consumes | Plan 6C extends without replacing |
|---|---|---|
| Plan 1 | `DeviceRegistry`, canonical serial lookup, `DeviceInventorySnapshot`, `DeviceViewState`, serial-targeted `trigger_sync`, `selection_revision`, `settings_revision` | adds atomic selection-revision commits and drop outcomes keyed by the raw serial |
| Plan 4 | one sequential event stream, `SendDisposition`, durable ordered queue, in-flight retention until request/revision ack | adds `.deviceSelectionAddition(serial:)` and `.playlistAppend(slug:)`; neither aliases `.deviceConfig` nor `.playlist` whole-value saves |
| Plan 5 | `request_id` on mutations, persisted canonical revision acknowledgements, `AcknowledgedDraft<Value>` | reconciles drop acknowledgements into open device/playlist drafts without cleaning a later local generation |
| Plan 6A | `ManagedPlaylistOwnership`, `VerifiedPlaylistMembership`, guarded normal-playlist identity, and coordinated device-authoritative Apple playlist publication | changes only the host logical manual playlist during a drop; the next coordinated sync derives its Apple target/membership through 6A and never writes iTunesDB from the drop handler |
| Plan 6B | Rockbox projection from the same `VerifiedPlaylistMembership`, recorded-only `.m3u8` ownership, and required recoverable finalization | leaves projection work to the existing coordinated sync; an immediate drop triggers that path only through serial-targeted sync admission |
| Existing library/playlist core | `SelectionMode`, `SelectionRule`, `LibraryIndex`, `PlaylistStore`, per-device manifests | adds daemon-only additive algebra and append APIs; removes the UI's drag path from `resolve_tracks` + `save_playlist` |

---

### Task 1: Add deterministic selection and playlist mutation algebra

**Files:**
- Create: `crates/classick/src/daemon/library_drop.rs`
- Modify: `crates/classick/src/daemon/mod.rs`
- Test: `crates/classick/src/daemon/library_drop.rs` (`#[cfg(test)]` module)

**Interfaces:**

```rust
pub(crate) const MAX_DROP_RULES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceSelectionMutation {
    pub selection: Selection,
    pub matched_paths: Vec<String>,
    pub selection_changed: bool,
}

pub(crate) fn validate_drop_rules(rules: &[SelectionRule]) -> Result<Vec<SelectionRule>>;
pub(crate) fn add_rules_to_selection(
    current: &Selection,
    rules: &[SelectionRule],
    index: &LibraryIndex,
) -> Result<DeviceSelectionMutation>;
pub(crate) fn append_rules_to_manual(
    current: &ManualPlaylist,
    rules: &[SelectionRule],
    index: &LibraryIndex,
) -> Result<(ManualPlaylist, Vec<String>)>;
```

Rules are normalized by trimming every component, rejecting empty or longer-than-256-Unicode-scalar components, and deduplicating on a lowercase comparison key. Canonical output order is artist, album, genre; within a kind compare case-insensitively, then by original spelling. Reject zero rules and more than 64 rules.

`Include` unions current and dropped rules, removes rules covered by a broader artist rule, and returns deterministic canonical rules. `All` is unchanged. `Exclude` computes an additive relaxation: retain an album exclusion only when its resolved album has no dropped track; replace every intersecting broad artist/genre exclusion with album exclusions for matched albums having no dropped track; then canonicalize. This deliberately includes an entire mixed album when only some of it intersects the drop because track-level exclusions do not exist, guaranteeing every dropped track becomes included without inventing a new rule kind.

Manual append resolves against the cached index, retains existing order, removes paths already present case-insensitively, and appends the new batch in natural path order: compare digit runs numerically (`2` before `10`), then lowercase path, then original path. Paths remain source-relative, slash-separated strings.

- [ ] **Step 1: Write failing algebra tests**

```rust
#[test]
fn exclude_artist_expands_only_to_unaffected_albums() {
    let index = index_with(&[
        ("/music/Birdy/Fire/01.flac", track("Birdy", "Fire", "Pop")),
        ("/music/Birdy/Young/01.flac", track("Birdy", "Young", "Pop")),
    ]);
    let current = selection(SelectionMode::Exclude, vec![artist(" birdy ")]);
    let changed = add_rules_to_selection(&current, &[album("BIRDY", "Fire")], &index).unwrap();
    assert_eq!(changed.selection.rules, vec![album("Birdy", "Young")]);
    assert_eq!(changed.matched_paths, vec!["Birdy/Fire/01.flac"]);
}

#[test]
fn all_is_unchanged_but_still_resolves_matches() {
    let current = Selection::all();
    let changed = add_rules_to_selection(&current, &[genre("pop")], &index()).unwrap();
    assert_eq!(changed.selection, current);
    assert!(!changed.selection_changed);
    assert_eq!(changed.matched_paths.len(), 2);
}

#[test]
fn manual_append_deduplicates_and_naturally_orders_batch() {
    let current = manual(&["Birdy/Fire/01.flac"]);
    let (next, appended) = append_rules_to_manual(&current, &[artist("Birdy")], &index()).unwrap();
    assert_eq!(appended, ["Birdy/Fire/02.flac", "Birdy/Fire/10.flac"]);
    assert_eq!(next.tracks.len(), 3);
}
```

Also cover invalid empty/65-rule/blank/257-scalar input, case-insensitive union, redundant album under artist, genre expansion, mixed-album relaxation, unmatched rules, duplicate existing playlist paths, and deterministic output from shuffled indexes.

- [ ] **Step 2: Run the focused tests and verify RED**

Run: `cargo test -p classick daemon::library_drop`

Expected: compile failure `could not find library_drop in daemon`.

- [ ] **Step 3: Implement the pure functions**

Use one resolver shared by both mutations:

```rust
fn resolved_relative_paths(index: &LibraryIndex, rules: &[SelectionRule]) -> Vec<String> {
    let selection = Selection {
        version: crate::selection::SELECTION_VERSION,
        mode: SelectionMode::Include,
        rules: rules.to_vec(),
    };
    let mut paths = index.files.iter()
        .filter(|(_, track)| selection.wants(&track.facts()))
        .filter_map(|(path, _)| path.strip_prefix(&index.source_root).ok())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>();
    paths.sort_by(natural_path_cmp);
    paths.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    paths
}
```

Do not call `resolve_tracks` from a client and do not read the source filesystem.

- [ ] **Step 4: Run focused tests GREEN**

Run: `cargo test -p classick daemon::library_drop`

Expected: all `library_drop` tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/daemon/library_drop.rs crates/classick/src/daemon/mod.rs
git diff --cached
git commit -m "feat(daemon): add library drop mutation algebra"
```

---

### Task 2: Journal additive mutations and make replays idempotent

**Files:**
- Create: `crates/classick/src/daemon/mutation_ledger.rs`
- Create: `crates/classick/src/daemon/library_mutations.rs`
- Modify: `crates/classick/src/daemon/mod.rs`
- Modify: `crates/classick/src/daemon/device_registry.rs`
- Modify: `crates/classick/src/playlist.rs`
- Test: `crates/classick/tests/library_mutation_integration.rs`

**Interfaces:**

```rust
pub(crate) type MutationRequestId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum MutationTarget { DeviceSelection { serial: String }, ManualPlaylist { slug: String } }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DeviceDropOutcome {
    pub request_id: MutationRequestId,
    pub serial: String,
    pub matched_tracks: usize,
    pub missing_tracks: usize,
    pub selection_changed: bool,
    pub selection_revision: u64,
    pub selection: Selection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PlaylistDropOutcome {
    pub request_id: MutationRequestId,
    pub slug: String,
    pub appended_tracks: usize,
    pub playlist_revision: u64,
    pub playlist: ManualPlaylist,
}

impl LibraryMutationService {
    pub(crate) fn add_selection_to_device(
        &mut self, request_id: &str, serial: &str, rules: &[SelectionRule]
    ) -> Result<DeviceDropOutcome, MutationFailure>;
    pub(crate) fn append_selection_to_playlist(
        &mut self, request_id: &str, slug: &str, rules: &[SelectionRule]
    ) -> Result<PlaylistDropOutcome, MutationFailure>;
    pub(crate) fn recover_pending(&mut self) -> Result<()>;
}
```

`MutationFailure { request_id, target, code, message }` uses codes `invalid_request_id`, `invalid_rules`, `unknown_device`, `unconfigured_device`, `no_library_matches`, `missing_playlist`, `non_manual_playlist`, `corrupt_playlist`, `request_id_collision`, and `persistence_failed`. Validate request IDs as the exact 36-byte lowercase UUID form (`8-4-4-4-12` ASCII hex with hyphens at byte offsets 8, 13, 18, and 23) without adding a dependency, and validate rules again in Rust even though Swift already validates them.

Store journals at `<config>/classick/devices/library-mutations/<request-id>.json`. A schema-v1 journal contains request fingerprint (target plus canonical rules), phase (`prepared | payload_published | revision_published | ledger_published`), old/new target bytes, prior/new revision, and serialized outcome. Write+fsync journal first; atomically replace the selection/manual-playlist payload; atomically publish the registry/playlist revision; atomically add the ledger entry; remove the journal. Startup recovery rolls forward the recorded new bytes/revision/ledger and never re-resolves a possibly changed library.

The ledger lives at `<config>/classick/devices/library-mutation-acks.json`, keeps the most recent 256 outcomes per target, and compares the stored fingerprint on replay. Same request+fingerprint performs no write and returns the stored counts with the target's current canonical revision. Same request with a different target/rules is `request_id_collision`. Eviction is oldest acknowledged timestamp then request ID; an in-progress journal is never evicted.

For device missing counts, compare the resolved source-relative paths with the explicit serial's authoritative manifest: connected devices use Plan 2's mounted `ManifestStore`, disconnected devices use the serial's host cache. `matched_tracks == 0` is a correlated `no_library_matches` and makes no mutation. Playlist append loads exactly `slug`; a smart, corrupt, or absent target fails without changing bytes or revision.

- [ ] **Step 1: Write failing transaction tests**

```rust
#[test]
fn replay_is_acknowledged_without_second_revision_bump() {
    let mut service = fixture().configured_device("A").build();
    let first = service.add_selection_to_device(REQ, "A", &[artist("Birdy")]).unwrap();
    let replay = service.add_selection_to_device(REQ, "A", &[artist("birdy")]).unwrap();
    assert_eq!(replay.selection_revision, first.selection_revision);
    assert!(!replay.selection_changed);
    assert_eq!(service.selection("A").rules.len(), 1);
}

#[test]
fn recovery_finishes_payload_without_reapplying_append() {
    let mut fixture = fixture().manual_playlist("favorites", &["old.flac"]).fail_after_payload();
    assert!(fixture.service.append_selection_to_playlist(REQ, "favorites", &[genre("Pop")]).is_err());
    fixture.service.recover_pending().unwrap();
    assert_eq!(fixture.service.playlist("favorites").tracks, ["old.flac", "new.flac"]);
    assert_eq!(fixture.service.playlist_revision("favorites"), 2);
}
```

Also inject failure after every phase; cover A/B isolation, unknown/unconfigured device, connected/device-manifest and disconnected/cache missing counts, no matches, manual ordering, smart/missing/corrupt rejection, request collision, ledger eviction, and persistence-before-outcome.

- [ ] **Step 2: Run the integration test and verify RED**

Run: `cargo test -p classick --test library_mutation_integration -- --test-threads=1`

Expected: compile failure for missing `LibraryMutationService`.

- [ ] **Step 3: Implement journal, ledger, and atomic store hooks**

Add exact store hooks rather than exposing paths to the runtime:

```rust
impl DeviceRegistry {
    pub(crate) fn publish_selection_revision(&mut self, serial: &str, revision: u64) -> Result<()>;
}

impl PlaylistStore {
    pub(crate) fn encode_manual(&self, playlist: &ManualPlaylist) -> Vec<u8>;
    pub(crate) fn publish_manual_bytes(&self, slug: &str, bytes: &[u8]) -> Result<()>;
}
```

Use existing atomic-file conventions (`tmp`, file `sync_all`, rename, best-effort parent fsync). Keep the transaction/recovery code below 500 lines by leaving algebra in `library_drop.rs` and ledger serialization/eviction in `mutation_ledger.rs`.

- [ ] **Step 4: Run transaction tests GREEN**

Run: `cargo test -p classick --test library_mutation_integration -- --test-threads=1`

Expected: all library-mutation integration cases pass.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/daemon/mutation_ledger.rs crates/classick/src/daemon/library_mutations.rs crates/classick/src/daemon/mod.rs crates/classick/src/daemon/device_registry.rs crates/classick/src/playlist.rs crates/classick/tests/library_mutation_integration.rs
git diff --cached
git commit -m "feat(daemon): persist idempotent library drop mutations"
```

---

### Task 3: Add correlated drop IPC and sync-after-drop policy

**Files:**
- Modify: `crates/classick/src/config_file.rs`
- Modify: `crates/classick/src/ipc_daemon.rs`
- Modify: `crates/classick/src/daemon/command_handler.rs`
- Modify: `crates/classick/src/daemon/runtime.rs`
- Modify: `crates/classick/src/daemon/session_admission.rs`
- Modify: `docs/ipc-protocol.md`
- Modify: `crates/classick/tests/fixtures/sample-config.toml`
- Modify: `crates/classick/tests/daemon_multi_device_integration.rs`
- Modify: `crates/classick/tests/daemon_runtime_integration.rs`

**Interfaces:**

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DropSyncBehavior { #[default] Immediate, NextSync }

// Add to DaemonSettings:
#[serde(default)]
pub drop_sync_behavior: DropSyncBehavior,

pub enum DropDelivery { AddedAndSyncing, AddedForNextSync, AlreadyPresent }

pub struct ManualPlaylistPayload {
    pub slug: String,
    pub name: String,
    pub tracks: Vec<String>,
}

// Add to DaemonCommand:
AddSelectionToDevice { request_id: String, serial: String, rules: Vec<SelectionRule> },
AppendSelectionToPlaylist { request_id: String, slug: String, rules: Vec<SelectionRule> },

// Add to DaemonEvent:
DeviceSelectionAdded { acknowledged_request_id: String, serial: String,
    matched_tracks: usize, missing_tracks: usize, selection_changed: bool,
    selection_revision: u64, selection: SelectionPayload, delivery: DropDelivery },
PlaylistSelectionAppended { acknowledged_request_id: String, slug: String,
    appended_tracks: usize, playlist_revision: u64, playlist: ManualPlaylistPayload },
LibraryMutationRejected { acknowledged_request_id: String, target: MutationTarget,
    code: String, message: String },
```

After `LibraryMutationService` returns durably, choose delivery against the same serial's authoritative runtime state:

| Setting/state/result | Delivery | Sync action |
|---|---|---|
| any; `missing_tracks == 0` | `already_present` | none |
| `next_sync`; any state | `added_for_next_sync` | none |
| `immediate`; connected and idle | `added_and_syncing` | admit existing serial-targeted manual sync |
| `immediate`; disconnected, busy, paused, or finalizing | `added_for_next_sync` | none; do not enqueue |

The mutation event is the authoritative persistence acknowledgement. Enqueue it on the requesting connection before invoking/announcing the sync session so user feedback cannot observe sync progress before the ack. If admission changes between the idle check and `try_admit_device`, downgrade to `added_for_next_sync` before sending the event; never claim a sync started when it did not.

- [ ] **Step 1: Add failing JSON/config/policy tests**

```rust
#[test]
fn drop_sync_behavior_defaults_to_immediate() {
    let settings: DaemonSettings = toml::from_str("").unwrap();
    assert_eq!(settings.drop_sync_behavior, DropSyncBehavior::Immediate);
}

#[tokio::test]
async fn busy_a_persists_drop_without_queuing_second_session() {
    let daemon = daemon_with_syncing_device("A").await;
    daemon.send(add_device(REQ, "A", artist("Birdy"))).await;
    assert_event!(daemon, DeviceSelectionAdded { delivery: AddedForNextSync, .. });
    assert_eq!(daemon.spawned_sessions("A"), 1);
    daemon.finish_active().await;
    assert_eq!(daemon.spawned_sessions("A"), 1);
}
```

Cover exact old/new JSON, echoed IDs/revisions, persistence before event, immediate connected idle, next-sync, disconnected, busy, paused, finalizing, already present, admission race, device A/B isolation, malformed rules, and correlated failures.

- [ ] **Step 2: Run focused tests and verify RED**

Run: `cargo test -p classick drop_sync_behavior`

Expected: compile failures for the absent enum/commands/events.

Run: `cargo test -p classick --test daemon_multi_device_integration -- --test-threads=1`

Expected: compile failure for the absent command helper.

- [ ] **Step 3: Implement additive v2 wire/runtime handling**

Build on Plan 1's clean daemon protocol 2.0.0 cutover. Do not preserve legacy command shapes or add a compatibility decoder: these additive commands require their defined target and `request_id`, and old payloads are rejected at decode. Keep the device command serial-targeted; the playlist command is global and therefore has no serial. Document these exact examples:

```json
{"type":"add_selection_to_device","request_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8740","serial":"A","rules":[{"kind":"artist","name":"Birdy"}]}
{"type":"append_selection_to_playlist","request_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8740","slug":"favorites","rules":[{"kind":"album","artist":"Birdy","album":"Fire Within"}]}
{"type":"device_selection_added","acknowledged_request_id":"018f9d7e-2f2b-7b52-9f1d-f78bdb2f8740","serial":"A","matched_tracks":12,"missing_tracks":4,"selection_changed":true,"selection_revision":8,"selection":{"mode":"include","rules":[{"kind":"artist","name":"Birdy"}]},"delivery":"added_and_syncing"}
```

- [ ] **Step 4: Run daemon tests GREEN**

Run: `cargo test -p classick ipc_daemon`

Expected: IPC tests pass with daemon protocol 2.0.0.

Run: `cargo test -p classick --test daemon_runtime_integration -- --test-threads=1`

Expected: runtime integration tests pass.

Run: `cargo test -p classick --test daemon_multi_device_integration -- --test-threads=1`

Expected: A/B and policy cases pass.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/config_file.rs crates/classick/src/ipc_daemon.rs crates/classick/src/daemon/command_handler.rs crates/classick/src/daemon/runtime.rs crates/classick/src/daemon/session_admission.rs docs/ipc-protocol.md crates/classick/tests/fixtures/sample-config.toml crates/classick/tests/daemon_multi_device_integration.rs crates/classick/tests/daemon_runtime_integration.rs
git diff --cached
git commit -m "feat(ipc): add acknowledged library drop commands"
```

---

### Task 4: Extend the ordered Swift transport for additive intents

**Files:**
- Modify: `ui/macos/Sources/Classick/Ipc/DaemonCommand.swift`
- Modify: `ui/macos/Sources/Classick/Ipc/DaemonEvent.swift`
- Modify: `ui/macos/Sources/Classick/Ipc/DaemonClient.swift`
- Create: `ui/macos/Sources/Classick/Model/LibraryDropState.swift`
- Modify: `ui/macos/Sources/Classick/Model/AppModel.swift`
- Modify: `ui/macos/Classick.xcodeproj/project.pbxproj`
- Modify: `ui/macos/Tests/ClassickTests/WireCodecTests.swift`
- Modify: `ui/macos/Tests/ClassickTests/DaemonClientTests.swift`
- Modify: `ui/macos/Tests/ClassickTests/AppModelReducerTests.swift`

**Interfaces:**

```swift
extension DaemonCommand {
    static func addSelectionToDevice(requestID: UUID, serial: String, rules: [SelectionRule]) -> Self
    static func appendSelectionToPlaylist(requestID: UUID, slug: String, rules: [SelectionRule]) -> Self
}

extension DurableIntentKey {
    static func deviceSelectionAddition(serial: String) -> Self
    static func playlistAppend(slug: String) -> Self
}

enum LibraryDropTarget: Hashable, Sendable {
    case device(serial: String, displayName: String)
    case manualPlaylist(slug: String, displayName: String)
}

enum DropOutcome: Equatable, Sendable {
    case adding(target: LibraryDropTarget)
    case addedAndSyncing(serial: String)
    case addedForNextSync(serial: String)
    case alreadyPresent(serial: String)
    case appended(slug: String, count: Int)
    case rejected(target: LibraryDropTarget, message: String)
}
```

These durable keys are deliberately distinct from Plan 4's `.deviceConfig(serial:)` and `.playlist(slug:)`: a queued drop must not replace a whole editor save, and an editor save must not replace a drop. For an unsent additive command with the same key, merge and canonicalize rules, assign one new request ID, replace the queued command, and keep its position at the newest chronology point. If that key is already in flight, retain it and hold one merged queued successor; send the successor only after the matching acknowledgement removes the in-flight command. Different keys retain global insertion order. Reconnect resends the exact in-flight bytes/request ID.

`AppModel` stores target-scoped activity and the last accessible outcome. Reducer handling requires both the expected request ID and nondecreasing canonical revision. `device_selection_added.selection` and `playlist_selection_appended.playlist` are the canonical post-mutation values used for editor reconciliation. A stale/unexpected acknowledgement may update authoritative device/playlist state through Plan 5 but must not announce success or remove another request. A correlated `library_mutation_rejected` is terminal, removes only its exact intent, and announces its message; an uncorrelated failure removes nothing.

- [ ] **Step 1: Write failing codec/queue/reducer tests**

```swift
func testWrittenDeviceDropStaysInFlightAndSuccessorWaitsForAck() async {
    let client = makeConnectedClient()
    await client.send(.addSelectionToDevice(requestID: id1, serial: "A", rules: [.artist(name: "Birdy")]))
    await client.send(.addSelectionToDevice(requestID: id2, serial: "A", rules: [.genre(name: "Pop")]))
    XCTAssertEqual(client.writes.map(\.requestID), [id1])
    await client.receive(.deviceSelectionAdded(acknowledgedRequestID: id1, serial: "A", matchedTracks: 1,
        missingTracks: 1, selectionChanged: true, selectionRevision: 2,
        selection: SelectionState(mode: .include, rules: [.artist(name: "Birdy")]),
        delivery: .addedAndSyncing))
    XCTAssertEqual(client.writes.map(\.requestID), [id1, id2])
}
```

Also cover exact command/event JSON, same-target unsent merge, different-target chronology, failed-write retention, reconnect resend, exact ack removal, collision failure retention/removal policy, one feedback announcement, and stale revision rejection.

- [ ] **Step 2: Run focused Swift tests and verify RED**

Run: `cd ui/macos && swift test --filter WireCodecTests`

Expected: compile failure for missing drop cases.

Run: `cd ui/macos && swift test --filter DaemonClientTests`

Expected: compile failure for missing additive durable keys.

- [ ] **Step 3: Implement wire, queue, and reducer cases**

Encode UUIDs with `uuidString.lowercased()`. Do not add a reply FIFO. Map outcomes to exactly these accessible strings: `Added and syncing`, `Added for next sync`, `Already on this iPod`, `Appended N songs`, or the correlated daemon message.

- [ ] **Step 4: Run focused Swift tests GREEN**

Run: `cd ui/macos && xcodegen generate`

Run: `cd ui/macos && swift test --filter WireCodecTests`

Run: `cd ui/macos && swift test --filter DaemonClientTests`

Run: `cd ui/macos && swift test --filter AppModelReducerTests`

Expected: all three suites pass.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Ipc/DaemonCommand.swift ui/macos/Sources/Classick/Ipc/DaemonEvent.swift ui/macos/Sources/Classick/Ipc/DaemonClient.swift ui/macos/Sources/Classick/Model/LibraryDropState.swift ui/macos/Sources/Classick/Model/AppModel.swift ui/macos/Tests/ClassickTests/WireCodecTests.swift ui/macos/Tests/ClassickTests/DaemonClientTests.swift ui/macos/Tests/ClassickTests/AppModelReducerTests.swift ui/macos/Classick.xcodeproj/project.pbxproj
git diff --cached
git commit -m "feat(ipc): queue additive library drop intents"
```

---

### Task 5: Define the launch-scoped Transferable payload and native drag sources

**Files:**
- Create: `ui/macos/Sources/Classick/Model/LibraryDragPayload.swift`
- Create: `ui/macos/Sources/Classick/Views/LibraryDragPreview.swift`
- Modify: `ui/macos/Sources/Classick/Views/LibraryBrowser.swift`
- Modify: `ui/macos/Sources/Classick/Model/AppModel.swift`
- Modify: `ui/macos/Info.plist`
- Modify: `ui/macos/Classick.xcodeproj/project.pbxproj`
- Create: `ui/macos/Tests/ClassickTests/LibraryDragPayloadTests.swift`
- Modify: `ui/macos/Tests/ClassickTests/LibraryBrowserLogicTests.swift`

**Interfaces:**

```swift
import CoreTransferable
import UniformTypeIdentifiers

extension UTType {
    static let classickLibrarySelection = UTType(
        exportedAs: "st.michaelwe.classick.library-selection", conformingTo: .data)
}

struct LibraryDragPayload: Codable, Transferable, Sendable, Equatable {
    static let currentVersion: UInt16 = 1
    static let maximumRules = 64
    let version: UInt16
    let launchNonce: UUID
    let rules: [SelectionRule]
    let summary: String

    static var transferRepresentation: some TransferRepresentation {
        CodableRepresentation(contentType: .classickLibrarySelection)
    }

    func validated(expectedNonce: UUID) throws -> [SelectionRule]
}
```

Declare `UTExportedTypeDeclarations` in the existing source-controlled `Info.plist` for identifier `st.michaelwe.classick.library-selection`, description `Classick Library Selection`, and conformance `public.data`; regenerate the committed project so the new Swift sources and tests are included.

```xml
<key>UTExportedTypeDeclarations</key>
<array>
  <dict>
    <key>UTTypeConformsTo</key>
    <array><string>public.data</string></array>
    <key>UTTypeDescription</key>
    <string>Classick Library Selection</string>
    <key>UTTypeIdentifier</key>
    <string>st.michaelwe.classick.library-selection</string>
  </dict>
</array>
```

`AppModel` owns one immutable `libraryDragLaunchNonce = UUID()` for its process lifetime. Validation rejects version other than 1, nonce mismatch, zero or more than 64 rules, non-normalized/duplicate/malformed rules, and an empty or over-128-character summary. Payloads contain only version, nonce, normalized rules, and summary—never paths, serials, slugs, or playlist content.

Only `.browse` artist, album, and genre aggregate labels are drag sources. V1 creates one payload per visible aggregate row. The preview is a compact `Label`-like native card containing the existing relevant symbol/art and summary; it is not the full-row screenshot. Use `.draggable(payload) { LibraryDragPreview(...) }` directly on row content so List/ScrollView retains native scrolling.

- [ ] **Step 1: Write failing payload and source-matrix tests**

```swift
func testPayloadRejectsAnotherLaunch() throws {
    let payload = LibraryDragPayload(version: 1, launchNonce: UUID(),
        rules: [.artist(name: "Birdy")], summary: "Birdy")
    XCTAssertThrowsError(try payload.validated(expectedNonce: UUID()))
}

func testPayloadRoundTripContainsNoPathsOrTarget() throws {
    let payload = LibraryDragPayload(version: 1, launchNonce: UUID(),
        rules: [.album(artist: "Birdy", album: "Fire Within")], summary: "Fire Within")
    let data = try JSONEncoder().encode(payload)
    let json = String(decoding: data, as: UTF8.self)
    XCTAssertFalse(json.contains("/Volumes"))
    XCTAssertFalse(json.contains("serial"))
    XCTAssertEqual(try JSONDecoder().decode(LibraryDragPayload.self, from: data), payload)
}
```

Cover exact encoded JSON, UTType identifier, wrong version, empty/65 rules, blank/oversized components, duplicate/non-normalized rules, summary limit, and `LibraryBrowser.dragPayload` returning payloads for browse artist/album/genre but `nil` for selection mode/playlists.

- [ ] **Step 2: Run payload tests and verify RED**

Run: `cd ui/macos && swift test --filter LibraryDragPayloadTests`

Expected: compile failure `cannot find LibraryDragPayload in scope`.

- [ ] **Step 3: Implement payload, preview, and row modifiers**

Expose one pure helper for tests and row construction:

```swift
nonisolated static func dragPayload(
    for rule: SelectionRule, summary: String, mode: Mode, launchNonce: UUID
) -> LibraryDragPayload? {
    guard case .browse = mode else { return nil }
    return try? LibraryDragPayload.make(rule: rule, summary: summary, launchNonce: launchNonce)
}
```

Pass the launch nonce into `LibraryBrowser` from `LibraryView`; Add Songs and device-selection browser instances remain non-draggable.

- [ ] **Step 4: Regenerate and run tests GREEN**

Run: `cd ui/macos && xcodegen generate`

Expected: `Classick.xcodeproj/project.pbxproj` includes both new Swift files and the test.

Run: `cd ui/macos && swift test --filter LibraryDragPayloadTests`

Run: `cd ui/macos && swift test --filter LibraryBrowserLogicTests`

Expected: both suites pass.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Model/LibraryDragPayload.swift ui/macos/Sources/Classick/Views/LibraryDragPreview.swift ui/macos/Sources/Classick/Views/LibraryBrowser.swift ui/macos/Sources/Classick/Model/AppModel.swift ui/macos/Info.plist ui/macos/Classick.xcodeproj/project.pbxproj ui/macos/Tests/ClassickTests/LibraryDragPayloadTests.swift ui/macos/Tests/ClassickTests/LibraryBrowserLogicTests.swift
git diff --cached
git commit -m "feat(ui): make library aggregates transferable"
```

---

### Task 6: Attach the exact native destination matrix and accessible feedback

**Files:**
- Modify: `ui/macos/Sources/Classick/Model/LibraryDropState.swift`
- Create: `ui/macos/Sources/Classick/Model/LibraryDropSubmissionCoordinator.swift`
- Create: `ui/macos/Sources/Classick/Views/LibraryDropDestination.swift`
- Modify: `ui/macos/Sources/Classick/Views/Sidebar.swift`
- Modify: `ui/macos/Sources/Classick/Views/MainWindow.swift`
- Modify: `ui/macos/Sources/Classick/Views/DeviceRow.swift`
- Modify: `ui/macos/Sources/Classick/Views/DeviceMusicPage.swift`
- Modify: `ui/macos/Sources/Classick/Views/PlaylistPage.swift`
- Modify: `ui/macos/Sources/Classick/ClassickApp.swift`
- Modify: `ui/macos/Sources/Classick/PreviewFixtures.swift`
- Create: `ui/macos/Tests/ClassickTests/LibraryDropStateTests.swift`
- Modify: `ui/macos/Tests/ClassickTests/AppModelReducerTests.swift`

**Interfaces:**

```swift
struct LibraryDropEligibility: Equatable {
    static func targetForDevice(_ device: DeviceViewState) -> LibraryDropTarget?
    static func targetForCard(_ presentation: DeviceRowPresentation) -> LibraryDropTarget?
    static func targetForPlaylist(_ summary: PlaylistSummary) -> LibraryDropTarget?
}

struct LibraryDropDestination: ViewModifier {
    let target: LibraryDropTarget
    let launchNonce: UUID
    let submit: @Sendable (LibraryDropTarget, [SelectionRule], UUID) -> Void
}

func acceptLibraryDrop(_ items: [LibraryDragPayload], on target: LibraryDropTarget) -> Bool

extension AppModel {
    func markLibraryDropAdding(requestID: UUID, target: LibraryDropTarget)
    func rejectLibraryDropLocally(
        requestID: UUID, target: LibraryDropTarget, message: String
    )
}

@MainActor
final class LibraryDropSubmissionCoordinator {
    init(
        send: @escaping @Sendable (DaemonCommand) async -> SendDisposition,
        rejectLocally: @escaping @MainActor (UUID, LibraryDropTarget, String) -> Void
    )
    func submit(target: LibraryDropTarget, rules: [SelectionRule], requestID: UUID)
}
```

Apply `.dropDestination(for: LibraryDragPayload.self, action:isTargeted:)` only when these predicates succeed:

| Surface | Accept when | Reject by omitting modifier |
|---|---|---|
| configured device parent row | exact `DeviceViewState.identity.serial`, `configured == true`; connected or remembered-disconnected both accept | unconfigured or missing serial |
| device Music child | same exact configured serial | Settings child |
| persistent device card | `DeviceRowPresentation.serial` resolves exactly one configured device | aggregate/actionless/ambiguous card |
| playlist sidebar row | `kind == .manual && error == nil` | smart or corrupt row |
| playlist missing from snapshot | never rendered/registered | missing slug |

The action receives SwiftUI's `[LibraryDragPayload]`, validates every nonce/version/rule set, concatenates and canonicalizes at most 64 total rules, generates one UUID request ID, marks target `Adding…`, and calls the matching additive command. V1 creates one item, but this makes native multi-item delivery coherent. Return `false` on an empty array, validation failure, or excessive combined rules. `isTargeted` controls only `contentShape` plus a transient `.selection`-like accent background/stroke on the existing bounds; no padding/frame changes and no overlay extending the hit area. Because only the direct row/card owns its modifier, one target highlights. Do not set `selectedDestination`, disclosure state, keyboard focus, or navigation in `isTargeted`/action.

Add `.accessibilityLabel("Add \(payload.summary) to \(target.displayName)")` to the preview/destination feedback path and announce the one authoritative result through `NSAccessibility.post(element: NSApp, notification: .announcementRequested, userInfo: [.announcement: text, .priority: NSAccessibilityPriorityLevel.medium.rawValue])`. Deduplicate announcements by acknowledged request ID.

Keep **Choose Music…** and **Add Songs…** unchanged and functional. The latter may continue using `resolve_tracks` because it is a non-drag picker workflow; drag/drop itself must use the atomic append command.

- [ ] **Step 1: Write failing target-matrix/feedback tests**

```swift
func testOnlyExplicitConfiguredCardAcceptsDeviceDrop() {
    XCTAssertEqual(LibraryDropEligibility.targetForCard(explicitCard(serial: "A")), .device(serial: "A", displayName: "Michael's iPod"))
    XCTAssertNil(LibraryDropEligibility.targetForCard(aggregateCard()))
    XCTAssertNil(LibraryDropEligibility.targetForDevice(unconfiguredDevice(serial: "B")))
}

func testPlaylistMatrixRejectsSmartAndCorrupt() {
    XCTAssertNotNil(LibraryDropEligibility.targetForPlaylist(manual("favorites")))
    XCTAssertNil(LibraryDropEligibility.targetForPlaylist(smart("recent")))
    XCTAssertNil(LibraryDropEligibility.targetForPlaylist(corrupt("broken")))
}

@MainActor
func testRapidCrossTargetSubmissionsPreserveUICallbackOrder() async {
    let sender = RecordingAsyncSender(suspendFirstSend: true)
    let coordinator = LibraryDropSubmissionCoordinator(send: sender.send)
    coordinator.submit(target: .device(serial: "A", displayName: "A"), rules: [.artist(name: "Birdy")], requestID: id1)
    coordinator.submit(target: .manualPlaylist(slug: "favorites", displayName: "Favorites"), rules: [.genre(name: "Pop")], requestID: id2)
    coordinator.submit(target: .device(serial: "B", displayName: "B"), rules: [.album(artist: "B", album: "Two")], requestID: id3)
    sender.resumeFirstSend()
    await sender.waitForCount(3)
    XCTAssertEqual(sender.targets, [.device("A"), .playlist("favorites"), .device("B")])
}
```

Also cover disconnected configured acceptance, Music vs Settings, nonce rejection returning false, one `Adding…`, one-target highlight state, no selection mutation, exact VoiceOver labels, exact outcome copy, and announcement deduplication.

Add a rapid chronology regression: synchronously submit device A, playlist `favorites`, then device B before the first actor send resumes; the injected sender must observe `[A, favorites, B]` exactly.

Add a focused disposition regression:

```swift
@MainActor
func testDroppedDispositionClearsOnlyMatchingAddingState() async {
    let model = AppModel()
    let sender = RecordingAsyncSender(dispositions: [.dropped])
    let coordinator = LibraryDropSubmissionCoordinator(
        send: sender.send,
        rejectLocally: model.rejectLibraryDropLocally)
    model.markLibraryDropAdding(requestID: id1, target: .device(serial: "A", displayName: "A"))
    coordinator.submit(target: .device(serial: "A", displayName: "A"),
        rules: [.artist(name: "Birdy")], requestID: id1)
    await sender.waitForCount(1)
    XCTAssertFalse(model.isLibraryDropAdding(requestID: id1))
    XCTAssertEqual(model.dropOutcome,
        .rejected(target: .device(serial: "A", displayName: "A"),
                  message: "Couldn’t send this addition to Classick."))
    XCTAssertEqual(model.persistedDropAcknowledgements, [])
}
```

- [ ] **Step 2: Run target tests and verify RED**

Run: `cd ui/macos && swift test --filter LibraryDropStateTests`

Expected: compile failure for missing eligibility/state types.

- [ ] **Step 3: Implement destination modifier and wire closures**

Use one AppDelegate-owned `@MainActor` coordinator. `submit` synchronously appends an `Intent` to its private FIFO in UI callback chronology and starts at most one drain task. That one drain removes from the front and awaits `DaemonClient.send` serially; never create an independent `Task` per drop, because actor-call arrival order from separate unstructured tasks is not defined.

```swift
@MainActor
final class LibraryDropSubmissionCoordinator {
    private var pending: [Intent] = []
    private var drainTask: Task<Void, Never>?
    private let send: @Sendable (DaemonCommand) async -> SendDisposition
    private let rejectLocally: @MainActor (UUID, LibraryDropTarget, String) -> Void

    func submit(target: LibraryDropTarget, rules: [SelectionRule], requestID: UUID) {
        pending.append(Intent(target: target, rules: rules, requestID: requestID))
        guard drainTask == nil else { return }
        drainTask = Task { await drain() }
    }

    private func drain() async {
        while !pending.isEmpty {
            let intent = pending.removeFirst()
            switch await send(intent.command) {
            case .sent, .queued:
                break
            case .dropped:
                rejectLocally(intent.requestID, intent.target,
                    "Couldn’t send this addition to Classick.")
            }
        }
        drainTask = nil
    }
}
```

`Intent.command` switches explicit device/manual targets to the two additive commands. `.sent` and `.queued` leave `Adding…` pending until the authoritative daemon event with the same request ID/revision arrives. `.dropped` is transport-local: call `AppModel.rejectLibraryDropLocally(requestID:target:message:)`, clear only that matching request's `Adding…` state, and expose the local error without adding a persisted acknowledgement or claiming the daemon saved anything. Thread `coordinator.submit` through `MainWindow`/`Sidebar`/`DeviceRow`; do not let views access the daemon socket directly.

- [ ] **Step 4: Run focused tests GREEN**

Run: `cd ui/macos && xcodegen generate`

Run: `cd ui/macos && swift test --filter LibraryDropStateTests`

Run: `cd ui/macos && swift test --filter AppModelReducerTests`

Expected: target matrix and feedback tests pass.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Model/LibraryDropState.swift ui/macos/Sources/Classick/Model/LibraryDropSubmissionCoordinator.swift ui/macos/Sources/Classick/Views/LibraryDropDestination.swift ui/macos/Sources/Classick/Views/Sidebar.swift ui/macos/Sources/Classick/Views/MainWindow.swift ui/macos/Sources/Classick/Views/DeviceRow.swift ui/macos/Sources/Classick/Views/DeviceMusicPage.swift ui/macos/Sources/Classick/Views/PlaylistPage.swift ui/macos/Sources/Classick/ClassickApp.swift ui/macos/Sources/Classick/PreviewFixtures.swift ui/macos/Tests/ClassickTests/LibraryDropStateTests.swift ui/macos/Tests/ClassickTests/AppModelReducerTests.swift ui/macos/Classick.xcodeproj/project.pbxproj
git diff --cached
git commit -m "feat(ui): add native library drop destinations"
```

---

### Task 7: Reconcile open editors and expose the global policy setting

**Files:**
- Modify: `ui/macos/Sources/Classick/Model/AppModel.swift`
- Modify: `ui/macos/Sources/Classick/Ipc/DaemonCommand.swift`
- Modify: `ui/macos/Sources/Classick/Views/DeviceMusicPage.swift`
- Modify: `ui/macos/Sources/Classick/Views/PlaylistPage.swift`
- Modify: `ui/macos/Sources/Classick/Views/SettingsView.swift`
- Modify: `ui/macos/Tests/ClassickTests/AcknowledgedDraftTests.swift`
- Modify: `ui/macos/Tests/ClassickTests/DeviceMusicLogicTests.swift`
- Modify: `ui/macos/Tests/ClassickTests/PlaylistEditorLogicTests.swift`
- Modify: `ui/macos/Tests/ClassickTests/AppModelReducerTests.swift`
- Modify: `ui/macos/Tests/ClassickTests/WireCodecTests.swift`

**Interfaces:**

```swift
enum DropSyncBehaviorWire: String, Codable, CaseIterable, Sendable {
    case immediate
    case nextSync = "next_sync"
}

// DaemonSettings gains:
var dropSyncBehavior: DropSyncBehaviorWire
```

On `device_selection_added`, apply the event's canonical `selection` and revision to that serial's `AcknowledgedDraft`. On `playlist_selection_appended`, reconcile its canonical manual `playlist` and revision. Do not construct a whole value from the dropped rules in Swift. Plan 5's generation truth table remains decisive: an earlier drop acknowledgement may advance canonical state but cannot clear/replace a later dirty local edit; a later editor save rebases on the new canonical revision rather than silently deleting the drop.

In General Settings add a macOS-15-compatible picker:

```swift
Picker("After adding music to an iPod", selection: dropSyncBehaviorBinding) {
    Text("Sync immediately").tag(DropSyncBehaviorWire.immediate)
    Text("On next sync").tag(DropSyncBehaviorWire.nextSync)
}
```

The binding edits Plan 5's global `AcknowledgedDraft<DaemonSettings>` and sends `save_config` with a request ID. Programmatic seeds/acks do not write. The absent legacy field decodes as `.immediate` in both Rust and Swift.

- [ ] **Step 1: Add failing stale-editor and setting tests**

```swift
func testDropAckCannotEraseLaterDirtyDeviceEdit() {
    var draft = acknowledgedDeviceDraft(revision: 4)
    draft.edit(localSelection("Local"))
    draft.reconcile(canonical: droppedSelection("Birdy"), revision: 5, acknowledgedRequestID: dropID)
    XCTAssertTrue(draft.isDirty)
    XCTAssertEqual(draft.value, localSelection("Local"))
    XCTAssertEqual(draft.canonicalRevision, 5)
}

func testLegacySettingsDefaultDropBehaviorToImmediate() throws {
    let decoded = try JSONDecoder().decode(DaemonSettings.self, from: Data("{}".utf8))
    XCTAssertEqual(decoded.dropSyncBehavior, .immediate)
}
```

Also cover playlist editor open during append, ack-A after local submit-B, same canonical revision replay, setting edit/submit/ack, and device A drop not reconciling device B.

- [ ] **Step 2: Run focused tests and verify RED**

Run: `cd ui/macos && swift test --filter AcknowledgedDraftTests`

Expected: failing reconciliation assertions for external canonical mutations.

Run: `cd ui/macos && swift test --filter AppModelReducerTests`

Expected: compile failure for `dropSyncBehavior`.

- [ ] **Step 3: Implement reconciliation and acknowledged setting binding**

Do not add `seededFromModel`, `userEdited`, `isSeeding`, or an unversioned local cache. Extend `AcknowledgedDraft.reconcile` only if its existing external-canonical-mutation path cannot preserve a later generation; keep one generic implementation shared by device, playlist, and settings editors.

- [ ] **Step 4: Run editor/settings tests GREEN**

Run: `cd ui/macos && swift test --filter AcknowledgedDraftTests`

Run: `cd ui/macos && swift test --filter DeviceMusicLogicTests`

Run: `cd ui/macos && swift test --filter PlaylistEditorLogicTests`

Run: `cd ui/macos && swift test --filter AppModelReducerTests`

Run: `cd ui/macos && swift test --filter WireCodecTests`

Expected: all suites pass.

- [ ] **Step 5: Commit**

```bash
git add ui/macos/Sources/Classick/Model/AppModel.swift ui/macos/Sources/Classick/Ipc/DaemonCommand.swift ui/macos/Sources/Classick/Views/DeviceMusicPage.swift ui/macos/Sources/Classick/Views/PlaylistPage.swift ui/macos/Sources/Classick/Views/SettingsView.swift ui/macos/Tests/ClassickTests/AcknowledgedDraftTests.swift ui/macos/Tests/ClassickTests/DeviceMusicLogicTests.swift ui/macos/Tests/ClassickTests/PlaylistEditorLogicTests.swift ui/macos/Tests/ClassickTests/AppModelReducerTests.swift ui/macos/Tests/ClassickTests/WireCodecTests.swift
git diff --cached
git commit -m "fix(ui): reconcile drops with acknowledged editor drafts"
```

---

### Task 8: Run end-to-end, accessibility, macOS 15, and visual gates

**Files:**
- Create: `crates/classick/tests/library_drop_daemon_integration.rs`
- Create: `ui/macos/Tests/ClassickTests/LibraryDropFlowTests.swift`
- Modify: `ui/macos/Tests/ClassickTests/DaemonClientTests.swift`
- Modify: `ui/macos/Tests/ClassickTests/AppModelReducerTests.swift`
- Modify: `ui/macos/Classick.xcodeproj/project.pbxproj`
- Modify: `LEARNINGS.md` only if execution discovers a non-obvious reusable constraint not already recorded

**Interfaces:** End-to-end test socket helpers send real newline JSON and assert persisted files, canonical revisions, session spawns, ordered acknowledgements, reconnect behavior, and UI reducer outcomes. No live iPod is required for the automated matrix.

- [ ] **Step 1: Write failing daemon flow test**

```rust
#[tokio::test]
async fn concurrent_a_device_and_playlist_drops_remain_correlated() {
    let daemon = sandboxed_daemon_with_device_and_playlist("A", "favorites").await;
    daemon.send(add_device(REQ_A, "A", artist("Birdy"))).await;
    daemon.send(append_playlist(REQ_P, "favorites", genre("Pop"))).await;
    let events = daemon.take_mutation_events(2).await;
    assert_eq!(events.request_ids(), [REQ_A, REQ_P]);
    assert_eq!(daemon.device_selection("A").revision, 2);
    assert_eq!(daemon.playlist("favorites").tracks, ["Birdy/Fire/01.flac"]);
}
```

Add daemon cases for same-target coalescing/replay, cross-target chronology, disconnect between write and ack, busy/finalizing no-hidden-session, and exact persistence-before-ack.

- [ ] **Step 2: Write failing Swift flow test**

```swift
func testDropFlowShowsAddingThenOneAuthoritativeOutcome() async {
    let flow = DropFlowHarness()
    await flow.drop(.artist(name: "Birdy"), on: .device(serial: "A", displayName: "Michael's iPod"))
    XCTAssertEqual(flow.visibleFeedback, "Adding…")
    await flow.receiveAddedAndSyncing(requestID: flow.requestID, serial: "A", revision: 7)
    XCTAssertEqual(flow.visibleFeedback, "Added and syncing")
    XCTAssertEqual(flow.announcements, ["Added and syncing"])
}
```

Add the complete target matrix, copy operation, no hover-navigation/focus change, stale editor, VoiceOver label, unavailable rejection, and one-target highlight tests.

- [ ] **Step 3: Run focused end-to-end tests and verify RED**

Run: `cargo test -p classick --test library_drop_daemon_integration -- --test-threads=1`

Expected: new integration assertions fail until all runtime wiring is complete.

Run: `cd ui/macos && swift test --filter LibraryDropFlowTests`

Expected: new flow assertions fail until all view-model wiring is complete.

- [ ] **Step 4: Fix only integration defects and rerun focused tests GREEN**

Run: `cargo test -p classick --test library_drop_daemon_integration -- --test-threads=1`

Run: `cd ui/macos && swift test --filter LibraryDropFlowTests`

Expected: both suites pass.

- [ ] **Step 5: Run full automated gates sequentially**

Run: `cargo test -p classick -- --test-threads=1`

Expected: all Rust unit/integration tests pass.

Run: `cd ui/macos && swift test`

Expected: all Swift tests pass.

Run: `cd ui/macos && xcodegen generate && git diff --exit-code -- Classick.xcodeproj/project.pbxproj`

Expected: XcodeGen makes no uncommitted project-file change.

Run: `xcodebuild -project ui/macos/Classick.xcodeproj -scheme Classick -configuration Debug -destination 'platform=macOS' MACOSX_DEPLOYMENT_TARGET=15.0 CODE_SIGNING_ALLOWED=NO build`

Expected: `** BUILD SUCCEEDED **` with the macOS 15 floor.

Run: `ui/macos/bundle.sh`

Expected: `ui/macos/Classick.app` is produced with the embedded daemon.

- [ ] **Step 6: Perform native visual/accessibility verification on macOS 27**

Use a disposable library/index and daemon sandbox; do not write the source share or a physical iPod. Verify in light and dark appearances:

1. Artist, album, and genre rows produce a compact system preview and copy cursor.
2. Library List and Albums ScrollView continue scrolling while dragging.
3. Only the directly hovered configured device parent/Music child/explicit card/manual playlist highlights in system accent; bounds and layout do not move.
4. Settings, smart/corrupt playlist, unconfigured device, and aggregate card show unavailable cursor and snap back.
5. Hover never expands, navigates, selects, or changes keyboard focus.
6. VoiceOver reads `Add Birdy to Michael's iPod` and `Add Birdy to Favorites`, then exactly one authoritative result.
7. `Adding…`, added-and-syncing, next-sync, already-present, append count, and correlated-error states are visually legible without presenting a blocking overlay.
8. **Choose Music…** and **Add Songs…** remain complete keyboard-accessible alternatives.

- [ ] **Step 7: Perform the macOS 15 runtime gate**

On a macOS 15 VM, repeat source preview, accepted/rejected target, scrolling, highlight, snap-back, VoiceOver label, and non-drag alternatives. Confirm no newer material is invoked. If the VM is unavailable, record the gate as externally blocked; the deployment-target build is necessary but does not substitute for this runtime check.

- [ ] **Step 8: Commit the final integration coverage**

```bash
git add crates/classick/tests/library_drop_daemon_integration.rs ui/macos/Tests/ClassickTests/LibraryDropFlowTests.swift ui/macos/Tests/ClassickTests/DaemonClientTests.swift ui/macos/Tests/ClassickTests/AppModelReducerTests.swift ui/macos/Classick.xcodeproj/project.pbxproj
git diff --cached
git commit -m "test(ui): verify native library drag and drop"
```

Do not add `LEARNINGS.md` unless the execution found a durable, non-obvious constraint; if it did, stage that exact file explicitly and use a separate `docs: record native drag and drop learning` commit.
