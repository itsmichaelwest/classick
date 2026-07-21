# Native device protocol — Rust core plan

**Status:** implementation-ready draft

**Depends on:** [approved design](../design/2026-07-19-native-device-protocol.md)

## 1. Outcome and ownership

The Rust core becomes the sole authority for portable device identity,
readiness, hardware facts, profile reconciliation, device-file ownership, and
the unified protocol. Windows and macOS clients receive the same typed model;
Linux gets the same library behavior even though it has no first-party UI.

The work should leave these boundaries:

```text
platform discovery -> DeviceObservation -> DeviceRecord/DeviceSnapshot
                                            |
host edit -> host outbox -> reconciliation -+-> config-only device transaction
                                            |
sync admission -> coordinated media/DB/profile transaction -> typed progress
```

Neither discovery nor presentation code writes device files. All writes pass
through one per-device lease, recovery gate, external-generation fence, and
transaction publisher.

## 2. Target module layout

Split responsibilities before changing behavior:

```text
crates/classick/src/
  device/
    id.rs                 DeviceId validation and serde
    facts.rs              Fact<T>, provenance, hardware presentation
    catalogue.rs          versioned reported-ID -> capability/model decoding
    readiness.rs          filesystem/DB classification, read-only
    discovery.rs          shared observation assembly
    platform/
      windows.rs          volume -> USB parent via SetupAPI/Configuration Manager
      macos.rs            mounted media -> USB device via IOKit
      linux.rs            mount/block device -> sysfs USB parent
  portable/
    profile.rs            strict schema and validation
    host_cache.rs         last canonical device state for offline UI
    outbox.rs             durable host mutations pending device delivery
    reconcile.rs          one-device state machine
    playlists.rs          portable subscribed definitions
  ipod/
    capability.rs         typed libgpod capability profiles
    sysinfo_extended.rs   parse/validate/generate/preserve policy
  wire/
    mod.rs                shared message enum and routing metadata
    command.rs
    event.rs
    hello.rs
    compatibility.rs
```

Existing modules may be moved incrementally, but `ipc.rs`, `ipc_daemon.rs`,
`ipc_device.rs`, SCSI parsing, platform discovery, and portable persistence
must not remain entangled at the end.

## 3. Phase R1 — fixtures and pure domain types

### Evidence fixtures

- Derive redacted fixtures from the ignored factory-restored and Finder-setup
  captures. Remove the real GUID, printed serial, host/owner/device names,
  volume UUID, and dynamic opaque values.
- Add layout fixtures for no DB, valid DB, and invalid DB.
- Add one validated Late-2009 capability fixture containing complete stable
  format arrays and a manifest describing provenance and removed keys.
- Prove `RentalClockBias` and `rbsync` are excluded rather than accidentally
  normalized into the generator.

### Core types

- Add `DeviceId` as exactly 16 uppercase hexadecimal characters. Accept a
  leading `0x` only at input/migration boundaries. Remove permissive
  `sanitize_serial` behavior from authority paths; invalid identity must error,
  not become `UNKNOWN` or an underscore-mutated key.
- Add `Fact<T> { value, source, confidence }`, with `reported`, `decoded`, and
  `inferred` provenance and an explicit absent value at the containing model.
- Add `HardwareFacts` for family, generation, exact model code, colour,
  firmware, and capacity. Only family/generation/model/colour feed
  presentation; firmware/capacity remain optional detail. Live battery state
  stays out of this work.
- Add `DeviceReadiness`: `ready`, `needs_apple_initialization`,
  `invalid_database`, and `identity_unavailable`.
- Add a stable `capability_profile_id` type separate from appearance facts.

### Required tests

- canonical ID acceptance/rejection and serde;
- migration accepts the legacy `0x` spelling but no arbitrary old key;
- exact model codes decode deterministically; ambiguous PID/capacity retains
  unknown exact variant and colour;
- capability selection never consumes a cosmetic choice.

## 4. Phase R2 — cross-platform discovery and readiness

Replace `ipod/device.rs`'s current “valid DB or invisible” scan with a read-only
candidate pipeline:

1. Enumerate plausible mounted media.
2. Associate each mount with its USB parent using the platform backend.
3. obtain and validate the USB iSerial/FireWire GUID;
4. inspect the iPod layout and parse the DB when present;
5. decode hardware facts from reported evidence;
6. return a `DeviceObservation` even when initialization or identity is
   incomplete.

Platform work:

- Windows: retain native volume enumeration, then use SetupAPI and
  Configuration Manager for the volume/disk/USB-parent association. Do not
  issue SCSI IOCTLs or request elevation.
- macOS: keep the IOKit association, but make it return the canonical ID and
  reported USB properties independently of capacity/model heuristics.
- Linux: implement mountinfo/block-device resolution and walk sysfs parents to
  the USB device. Keep it library-only and fixture-testable.

Move `scsi_inquiry.rs` and live extended inquiry behind an explicit diagnostic
entry point or feature. Remove SCSI-derived values from production discovery,
signing, and provisioning.

Update daemon watcher identity, debounce, registry lookup, history, storage,
and session routing to use `DeviceId`. A reconnect whose mount path changes is
the same device; two mounts claiming the same live ID are an error and neither
is writable until the ambiguity clears.

An identity-unavailable candidate cannot enter the durable registry or a
`DeviceId`-keyed session. Give it an opaque, connection-generation-scoped
`observation_id` solely so inventory snapshots can update/remove its read-only
diagnostic row. The observation ID is not accepted by mutation commands and is
never written to disk.

Readiness must not create the Classick directory, repair SysInfo, create an
iTunesDB, or seed host files merely because a device was observed.

### Required tests

- platform association contracts with injected OS observations;
- restored fixture is visible as `needs_apple_initialization` with zero writes;
- malformed DB is distinct from absent DB and from no candidate;
- missing/invalid ID is visible but mutation-ineligible;
- mount change preserves identity; duplicate live identity blocks writes;
- production discovery does not call the SCSI adapter.

## 5. Phase R3 — registry and host-state migration

Make `devices/registry.json` version 2 and key it by canonical `DeviceId`.
Retain only host-useful data:

- configured/adopted flag;
- cached name and hardware presentation facts;
- last seen, storage, and last readiness for disconnected display;
- last imported portable profile schema/revisions;
- migration status.

Do not copy those presentation/runtime fields into `profile.json`.

Replace the separate host `selection.json`, `settings.json`,
`subscriptions.json`, and `managed_playlists.json` authority model with:

- a versioned per-device host cache containing the last canonical portable
  state; and
- a durable outbox containing explicit host mutations not yet published.

Migration is one-time and crash-safe:

1. validate the old serial as `DeviceId`;
2. preserve legacy files until the new cache and outbox are durable;
3. convert existing configured values into one initial host mutation when the
   device has no portable profile yet;
4. never infer that an old donor-style `SysInfoExtended` is owned;
5. retain ambiguous legacy state and block mutation with diagnostics.

Do not add a host/install ID or wall-clock conflict timestamp.

## 6. Phase R4 — portable profile and reconciliation

### Profile schema

Implement the strict allowlist from the design:

- schema version and `device_id`;
- optional capability-profile ID;
- selection, settings, and subscriptions with independent revisions and unique
  mutation IDs;
- exact owned Apple playlist IDs/kinds and Rockbox filenames/hashes;
- companion authority schema versions/hashes;
- owned generated `SysInfoExtended` hash when applicable.

Use `deny_unknown_fields` for owned profile input and explicit schema migration.
Reject absolute paths, parent traversal, drive/UNC prefixes, credentials, and
all excluded appearance/runtime/host fields. Playlist definitions use paths
relative to the configured global library root.

### Host outbox contract

Each edit becomes a mutation with:

- globally unique mutation ID;
- `device_id`;
- complete desired value for the affected component;
- the last imported device revision for diagnostics, not cross-host ordering;
- state `pending_device` until exact device publication is confirmed.

Persist the host cache and outbox before acknowledging acceptance. The
resulting config event exposes `delivery: pending_device`. If connected, the
command handler immediately runs the config-only device transaction and emits
`delivery: device_committed` for that mutation after its hash/revision is
verified. A failure leaves the outbox intact and reports the failure without
reverting the accepted host value.

### Reconciliation algorithm

Under the per-device lease, and before auto-sync admission:

1. recover any pending Classick transaction;
2. read and validate the portable profile;
3. if a host outbox mutation exists, publish host desired state;
4. otherwise import the connected device profile into the host cache;
5. if no profile exists, adopt only from an explicit configured/migrated host
   state and create the minimal Classick subtree transactionally;
6. update inventory/config events from canonical state;
7. evaluate auto-sync using that state.

Never compare unrelated host counters or timestamps. If two hosts make offline
edits, physical publication order wins: the next host with pending intent
publishes it; a host without pending intent imports what is on the device.

Subscribed definitions are copied only for subscribed playlists. A missing
local definition does not authorize deletion of a portable foreign/unknown
file. Library identity and different-library replacement remain out of scope.

### Required tests

- disconnected auto-sync disable wins before plug-in admission;
- connected edits reach `device_committed` without a music sync;
- host acceptance survives device-publication failure;
- no-pending host imports device state;
- pending host state overwrites device state and clears only after exact commit;
- duplicate mutation ID is idempotent;
- two devices never share values, revisions, outboxes, or ownership;
- schema golden test rejects every excluded key;
- profile and playlist paths cannot escape the selected library root;
- idle adopted device has exactly the minimal footprint.

## 7. Phase R5 — coordinated publication and ownership

Generalize the current config journal and media transaction so every device
mutation uses the same safety boundary:

- one lease per `DeviceId`;
- recovery before reads used for planning;
- recorded mount and connection generation;
- exact original bytes-or-absence for every authority;
- staged hashes before rename;
- directory durability where supported;
- rollback or retained unambiguous pending state.

Config-only publication may touch `profile.json`, subscribed definitions, and
conditional `SysInfoExtended`; it must not rewrite iTunesDB merely to publish
an auto-sync setting. Music sync publication additionally coordinates media,
iTunesDB, artwork DB, manifest, Apple playlist ownership, and Rockbox
projections.

Move new backup/quarantine material below `iPod_Control/classick/pending/`.
Add conservative recovery rules for legacy `iTunesDB.classick-backup` and
`iTunesDB.corrupt`; do not delete or claim ambiguous files.

## 8. Phase R6 — typed `SysInfoExtended`

Replace `ipod/sysinfo_provision.rs` donor templates and the mixed-purpose
`sysinfo_extended.rs` parser with:

- typed capability profiles;
- foreign-file parsing and completeness validation;
- deterministic stable projection generation;
- ownership/hash policy integrated with `profile.json`.

Initially mark only independently validated profiles usable. The Late-2009
160 GB profile comes from the redacted physical capture and pinned libgpod
behavior. Other bundled donor templates do not remain production fallbacks.

Generated content includes the canonical FireWireGUID, validated
FamilyID/DBVersion, complete AlbumArt/ImageSpecifications/ChapterImageSpecs,
SupportsSparseArtwork, SQLiteDB, and any other proven stable libgpod-consumed
capability. It excludes the dynamic and private fields listed in the design,
including serial, ModelNumStr, firmware, SKU, and colour.

Set the validated model/capability in libgpod memory on every fresh DB open.
For a foreign file, preserve bytes exactly. If it is incomplete, block only the
operation that cannot be proven safe; do not replace it. A legacy template
match is a migration hint, not ownership proof.

### Required tests

- deterministic golden projection and complete per-format fields;
- prohibited-key absence;
- foreign bytes unchanged through discovery and sync;
- owned replacement requires matching device/profile/hash authority;
- legacy donor resemblance never establishes ownership;
- generated capability activates expected artwork and hash58 paths;
- physical Finder/iTunes, firmware artwork, and playback smoke tests.

## 9. Phase R7 — unified wire protocol 3

Inventory every v1 subprocess and v2 daemon message. Create one flattened
tagged enum used on both transports. The first message is `hello` with protocol
version, endpoint role (`desktop`, `daemon`, or `worker`), core/client version,
and capabilities.

Routing rules:

- device commands/events require `device_id`;
- sync progress/decisions require `device_id` and `session_id`;
- queries and mutations require `request_id`;
- prompts also carry a prompt ID scoped to the session;
- persisted configuration mutations carry their mutation ID and delivery
  state;
- the daemon rejects worker output for the wrong role/device/session.

The daemon decodes worker messages into the shared Rust type and forwards the
typed event. Delete `sync_event.line`; malformed worker output fails that
owned session and never crosses to desktop clients.

Extend inventory with readiness, hardware facts, profile/adoption status, and
config delivery summaries. Identified entries carry `device_id`; an
identity-unavailable entry carries only its ephemeral `observation_id` and no
mutating affordances. Do not expose mount paths as identity or send private
serial/model evidence merely for client icon selection.

Publish language-neutral golden JSON vectors for every command/event plus
negative vectors for missing routing, wrong role/major, and malformed IDs.
Keep additive unknown-event behavior explicit. Remove v1/v2 production codecs
only when both clients pass the frozen vectors.

## 10. Core completion gate

- `cargo fmt --check`, Clippy for all targets/configurations used in CI, and
  `cargo test --workspace -- --test-threads=1` pass.
- Linux compiles and runs domain/discovery fixture tests without UI code.
- Rust golden vectors are consumed unchanged by Swift and C# tests.
- Restored-device discovery causes no writes.
- A connected setting mutation is visible to admission before any auto-sync.
- Another host imports the portable profile with no pending edit.
- Finder/iTunes, firmware, artwork, and Rockbox checks pass on supported
  physical fixtures.
- Current protocol, architecture, safety, and learnings docs describe shipped
  behavior rather than the transitional implementation.
