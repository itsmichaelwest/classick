# Native device protocol implementation plan

**Status:** ready for implementation after review

**Design:** [Native device protocol and identity](../design/2026-07-19-native-device-protocol.md)

This is the delivery index for a high-risk cross-platform persistence and
public-wire change. The component plans are independently executable, but the
merge order below is authoritative:

- [Rust core plan](2026-07-19-native-device-protocol-rust.md)
- [Windows UI plan](2026-07-19-native-device-protocol-windows.md)
- [macOS UI plan](2026-07-19-native-device-protocol-macos.md)

## 1. Delivery principles

- Keep the application usable at every checkpoint. Do not combine the first
  portable-device write, the protocol-major switch, and client UI changes in
  one unreviewable change.
- The Rust daemon owns device discovery, readiness, reconciliation, persistence,
  sync admission, and hardware interpretation. Clients render typed facts and
  send intent; they do not inspect the iPod filesystem or reproduce hardware
  heuristics.
- The protocol 3 switch is coordinated across Rust, Swift, and C#. There is no
  mixed-major production mode and no nested legacy decoder behind the v3
  endpoint.
- `serial` is renamed to `device_id` at the new wire and persistence boundaries.
  Temporary internal adapters may retain old names only inside an explicit
  migration layer.
- An identity-unavailable mount may appear only as a connection-scoped,
  read-only observation. It is never persisted as a device and cannot receive
  device commands.
- A setting edit has two durability states: accepted into the host outbox, then
  published to the device. Auto-sync admission observes the accepted host
  intent immediately; clients do not mistake host durability for device
  delivery.
- Library identity and Classick-owned Apple initialization remain deferred.

## 2. Merge sequence

### Checkpoint A — fixtures, types, and read-only discovery

Rust owns this checkpoint.

1. Add redacted initialized/uninitialized and capability fixtures.
2. Introduce `DeviceId`, hardware facts, and readiness types.
3. Implement ordinary OS mount-to-USB identity on Windows, macOS, and Linux.
4. Publish the new facts internally while the v2 external wire remains intact.
5. Prove discovery performs no device writes and SCSI is absent from the
   production path.

Gate: Rust tests on all supported targets; physical initialized and restored
iPod smoke tests on macOS and Windows where available.

### Checkpoint B — portable profile and publication

Rust owns this checkpoint; UI behavior remains on the old wire.

1. Add the strict portable profile schema and host outbox.
2. Migrate existing per-device host files and registry records.
3. Implement reconciliation, device adoption, and config-only publication.
4. Integrate profile, manifest, playlist ownership, and rollback authorities.
5. Replace donor `SysInfoExtended` provisioning with validated typed
   capabilities and foreign-file preservation.

Gate: crash/recovery, foreign-file, ownership, two-device, and reconnect tests;
Finder/iTunes and firmware smoke tests before proceeding.

### Checkpoint C — freeze protocol 3

Rust defines the complete schema and publishes language-neutral golden JSON
vectors. Before client implementation starts, freeze:

- hello roles and capabilities;
- every command/event discriminator;
- `device_id`, `session_id`, `request_id`, and mutation correlation rules;
- readiness, hardware facts, device-config delivery state, and typed progress;
- the read-only observation shape for identity-unavailable mounts;
- unknown-additive-message behavior and explicit wrong-major failure.

The schema is complete only when every existing v1/v2 operation has a v3 home
or an explicit removal decision.

### Checkpoint D — client migrations

Windows and macOS may proceed in parallel after the v3 vectors freeze.

- Replace duplicate daemon/subprocess models with one v3 codec per client.
- Move all client state from singleton or raw-serial assumptions to
  `device_id`-keyed inventory.
- Render readiness and hardware provenance honestly.
- Keep device config editable while disconnected and display host-pending versus
  device-delivered state.
- Remove client decoding of `sync_event.line`.

Gate: each client passes the shared vectors and its reducer/view-model tests.

### Checkpoint E — coordinated major switch

1. Switch the Rust daemon and worker to v3.
2. Ship both clients with v3 in the same release set.
3. Remove old codecs and compatibility shims after migration fixtures pass.
4. Rewrite `docs/ipc-protocol.md` and archive the v1/v2 documents.
5. Update current architecture and safety docs from “target” to shipped state.

There is no silent fallback. An old client receives a clear incompatible-core
error; an old daemon is rejected before any mutation command can be sent.

## 3. End-to-end acceptance matrix

| Scenario | Rust/core result | Client result |
| --- | --- | --- |
| factory-restored, not initialized | classified without writes | Apple-setup guidance; sync disabled |
| initialized, no Classick profile | ready; safe adoption path | setup/configuration offered |
| known device disconnected | host cache and outbox available | settings remain editable |
| auto-sync disabled while disconnected | outbox wins before admission | pending until connected, then delivered |
| two connected iPods | independent identity/config/session routing | independent rows, pages, progress, actions |
| foreign `SysInfoExtended` | byte-identical preservation | no misleading repair claim |
| ambiguous exact model/colour | generic hardware presentation facts | generic family/model artwork |
| exact model fact available | deterministic decoded generation/colour | exact artwork and factual detail |
| second host, no pending edit | imports portable profile | settings visible; library decision deferred |
| optional Rockbox enabled | only owned projections are written | per-device setting and delivery state shown |

## 4. Verification and review

Run shared-resource suites sequentially:

```text
cargo test --workspace -- --test-threads=1
cd ui/macos && swift test
cd ui/windows && dotnet test Classick.UI.Tests/Classick.UI.Tests.csproj
```

Also require Linux Rust build/tests, deterministic XcodeGen regeneration after
Swift source changes, Windows x64 and ARM64 builds, and physical-device checks
for Finder/Apple Music, Windows Apple software/iTunes, firmware playback and
artwork, reconnect reconciliation, and Rockbox when enabled.

Each persistence checkpoint receives separate specification/safety and
code-quality review. Each client checkpoint receives one combined protocol,
state-management, and presentation review. Re-review only blocking fixes.

## 5. Explicitly deferred follow-ups

- Classick-owned initialization of a restored iPod.
- Portable source-library identity and the different-library replacement
  prompt.
- Automatic privileged SCSI enrichment, battery telemetry, and live device
  channels.
- User-configured model, colour, or artwork overrides.
