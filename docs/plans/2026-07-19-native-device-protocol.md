# Native device protocol implementation plan

**Status:** ready for implementation after review

**Design:** [Native device protocol and identity](../design/2026-07-19-native-device-protocol.md)

This is a high-risk cross-platform persistence and public-wire change. Deliver
it in reviewable vertical slices. Do not combine the protocol major switch
with the first device-filesystem mutation in one checkpoint.

## 1. Preserve research evidence safely

- Derive redacted fixtures from the two 2026-07-19 device captures. Do not
  commit the raw dumps, real USB GUID, printed serial, host name, owner name,
  volume UUID, or dynamic opaque values.
- Add a redacted initialized/uninitialized layout fixture and a redacted
  Late-2009 capability fixture containing the validated stable format arrays.
- Record the fixture provenance and which fields were deliberately removed.
- Add a parser test proving that `RentalClockBias` and `rbsync` are classified
  as excluded dynamic fields.

## 2. Introduce the device model and readiness boundary

Primary files:

- split `crates/classick/src/ipod/device.rs` into identity, catalogue,
  readiness, and platform backends;
- update `crates/classick/src/ipod/layout.rs`, daemon watcher/inventory, and
  preflight;
- extend Rust, Swift, and C# inventory models only after the Rust model is
  stable.

Work:

1. Add a validated canonical `DeviceId` type accepting optional `0x` input and
   serializing as 16 uppercase hex characters.
2. Make mount-to-USB association the production identity path on Windows,
   macOS, and Linux. Remove automatic SCSI calls and elevation-dependent
   branches from discovery and apply-time identity resolution.
3. Replace the single heuristic model string with fact values carrying source
   and confidence. Stop assigning a representative colour/SKU as though it
   were reported.
4. Add readiness states for ready, needs Apple initialization, invalid DB, and
   unavailable identity. Discovery must return uninitialized candidates rather
   than filtering them out as absent.
5. Resolve exact hardware and colour from real model/serial facts when present.
   When evidence is ambiguous, keep the exact values unknown and expose a
   generic family/model illustration. Do not add appearance settings, user
   questions, or portable appearance state.
6. Keep SCSI tooling behind explicit diagnostic commands/features; it must not
   affect normal correctness.

Tests:

- canonical ID normalization and rejection;
- mount association contract tests for each platform adapter;
- restored layout reports `needs_apple_initialization` and performs no write;
- missing/invalid DB is distinct from absence;
- ambiguous Classic PID/capacity produces unknown colour and an honest model
  range;
- exact model codes select exact client artwork; missing codes select generic
  family/model artwork;
- an ambiguous capability profile uses only a separately validated neutral
  profile or blocks the projection-dependent operation without prompting;
- mutation admission rejects missing identity and non-ready devices.

Review checkpoint: separate specification-compliance and code-quality reviews,
with special attention to false-positive mounts and accidental writes during
discovery.

## 3. Add the portable profile and reconciliation state machine

Primary files:

- `crates/classick/src/device_state.rs` and new focused profile/outbox modules;
- `crates/classick/src/daemon/device_registry.rs`;
- device-config command handling and auto-sync admission in daemon runtime;
- sync transaction authority lists and rollback snapshots.

Work:

1. Define versioned `profile.json` with canonical identity, only the resolved
   capability-profile ID, selection/settings/subscriptions sections, ownership,
   independent revisions, mutation IDs, and authority hashes. Do not create a
   separate ownership file or persist a host/install identifier.
2. Define the host cache and durable pending-intent/outbox format. Migrate the
   current serial-keyed selection/settings/subscriptions files without losing
   revisions. Keep registry `name`, `model_label`, `last_seen`, storage, and
   other presentation/runtime fields host-only; never copy them to the profile.
3. Implement one per-device reconciliation state machine:
   pending host intent wins; otherwise connected device wins. Do not compare
   unrelated host counters or timestamps as a global clock.
4. Change `save_device_config` so a disconnected change is durably accepted as
   pending and a connected change is published immediately. Preserve request
   correlation until the intended durability boundary is reached.
5. Load pending intent before connection-triggered auto-sync gating.
6. Add portable definitions for subscribed Classick playlists only. Preserve
   foreign playlists and do not serialize the entire host playlist store.
7. Include profile-embedded ownership and portable playlist authorities in
   coordinated publication and rollback.
8. Move new database backups/quarantines beneath `classick/pending`. Add a
   conservative migration/recovery path for legacy `iTunesDB.classick-backup`
   and `iTunesDB.corrupt` files without deleting ambiguous bytes.

Tests:

- disconnected auto-sync disable prevents plug-in sync;
- connected mutation publishes without waiting for a music sync;
- no-pending host imports device state;
- pending host state overwrites stale/different device state;
- repeated mutation IDs are idempotent;
- two serials never share settings or revisions;
- interrupted profile publication recovers exact bytes-or-absence;
- portable profile contains no absolute source path or credentials;
- the idle device has no appearance preference, separate ownership file, or
  redundant hardware cache;
- a golden schema-key test rejects name/model/colour/icon/capacity/firmware,
  runtime-cache, telemetry, timestamp, and host/install identity fields;
- new syncs do not create Classick backup/quarantine files in Apple's iTunes
  directory.

Review checkpoint: separate persistence/safety and maintainability reviews.

## 4. Replace donor plist provisioning

Primary files:

- replace `crates/classick/src/ipod/sysinfo_provision.rs`;
- refactor `crates/classick/src/sysinfo_extended.rs` into foreign parsing,
  validation, and typed projection generation;
- replace or retire `crates/classick/data/sysinfo-extended/*.plist`;
- extend profile ownership and sync transaction modules.

Work:

1. Define typed capability profiles containing only stable, understood,
   libgpod-consumed fields and complete artwork/image/chapter arrays.
2. Seed the Late-2009 160 GB Classic profile from the redacted physical capture
   and pinned libgpod parser behavior. Mark other existing templates
   unvalidated until independently checked; do not continue broad donor
   substitution.
3. Generate deterministic plist bytes from the typed profile and canonical
   device ID. Omit serial, firmware, exact SKU/colour, volume, updater, and live
   state rather than borrowing or duplicating them. Continue setting the
   validated model in libgpod memory.
4. Detect absent, Classick-owned, and foreign files by profile ownership plus
   exact hash. Preserve foreign bytes.
   A normalized match to an old embedded donor template is only a migration
   hint; require explicit user authorization or independent transaction proof
   before replacing it.
5. Validate complete format arrays before publication. Treat a partial
   projection as a programming error, not a best-effort artwork warning.
6. Atomically publish the file and its ownership metadata within the device
   transaction. Reapply GUID and model/capability information to libgpod in
   memory on every fresh DB open.
7. Decide and implement the safe behavior for an incomplete foreign file. The
   minimum acceptable behavior is preserving it and blocking artwork mutation;
   a pinned-libgpod patch that safely merges in-memory fallback capabilities is
   preferable if it can be proven not to change foreign bytes.
8. Remove the raw `write_to_ipod` API and production descriptions of live SCSI
   XML as a persistent authority.

Tests:

- golden deterministic projection for the validated Classic;
- required complete arrays and per-format fields;
- exclusion of dynamic, host, donor, volume, battery, and opaque keys;
- no `ModelNumStr`, serial, firmware, SKU, or colour field in the generated
  baseline projection;
- foreign file remains byte-identical;
- legacy donor-style file is not claimed from content resemblance alone;
- Classick-owned file updates only on capability schema/hash change;
- mismatched device ID or profile blocks replacement;
- generated profile gives libgpod the expected artwork formats and hash58
  signing path;
- Finder/iTunes and physical-device smoke tests after a Classick sync.

Review checkpoint: separate specification/safety and code-quality reviews,
then re-review any blocking device-data fix.

## 5. Unify the wire protocol as version 3

Primary files:

- replace Rust `ipc.rs` and `ipc_daemon.rs` with a focused shared `wire/`
  module;
- update `progress.rs`, daemon IPC server, sync orchestrator, and worker
  command/event handling;
- update Swift `WireModels.swift`/daemon client/reducer and C# IPC models/client;
- rewrite `docs/ipc-protocol.md` and its linked schema only when code moves.

Work:

1. Inventory every v1 and v2 command/event and define one exhaustive message
   set with common request, device, and session routing fields.
2. Add `hello` role/capability negotiation and endpoint-specific allowed
   command subsets.
3. Make the worker and daemon serialize the same Rust types. Decode and
   validate worker events before forwarding them.
4. Replace `sync_event.line` nested JSON with typed progress events.
5. Preserve durable request acknowledgement, device revisions, prompt IDs,
   cancellation drain, terminal event plus EOF, and unknown-additive-event
   behavior.
6. Move Swift and C# together in the same release. Remove the old protocol
   decoders after migration tests pass; do not add ambiguous fallback routing.

Tests:

- Rust/Swift/C# golden vectors for every message;
- wrong role, device ID, session ID, prompt ID, and request ID are rejected;
- no protocol output contains an encoded JSON line;
- progress ordering and cancellation/finalization remain intact;
- old major versions fail explicitly at handshake;
- multi-device events cannot reduce onto the wrong device.

Review checkpoint: separate wire-spec compliance and client/runtime quality
reviews.

## 6. Complete UI and end-to-end behavior

- Show uninitialized, invalid-database, and identity-unavailable states without
  offering sync.
- Present decoded model/colour when known and generic family/model artwork when
  unknown; do not expose appearance configuration.
- Keep disconnected per-device settings editable and visibly pending when they
  have not yet reached the device.
- Explain that Apple setup is required without claiming Classick can repair or
  initialize the device.
- Validate two physical or fixture-backed iPods with independent configuration.
- Validate macOS Finder/Apple Music, Windows Apple software/iTunes, iPod
  firmware playback/artwork, Linux compilation/tests, and Rockbox projections
  when enabled.
- Validate another Classick installation imports the device profile. Defer any
  library-match conclusion and replacement prompt until library identity is
  separately designed.

Final checkpoint: run full Rust, Swift, and .NET suites sequentially where they
touch shared device/IPC resources, then perform the physical-device matrix and
update current architecture/protocol docs to describe shipped behavior.
