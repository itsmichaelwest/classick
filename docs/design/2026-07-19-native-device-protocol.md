# Native device protocol and identity

**Status:** implemented; Apple initialization, library identity, and additional
device-family validation remain deferred

**Date:** 2026-07-19

**Scope:** wire unification, device identity/readiness, portable per-device state,
settings reconciliation, Apple-file ownership, and `SysInfoExtended`.

## 1. Outcome

Classick will manage an Apple-initialized iPod without claiming ownership of
the device as a whole. Apple software owns initialization and Apple-specific
preferences. Classick owns one small portable subtree, performs narrowly
scoped libgpod updates to the Apple media database, and writes a stable
`SysInfoExtended` capability projection only when one is required and no
foreign file already exists.

The desktop applications, daemon, and sync worker will use one versioned JSON
protocol. Device operations will be keyed by a cross-platform USB device ID,
not by a mount path, volume identifier, display name, or privileged SCSI
response.

The design supports multiple remembered and connected iPods, each with an
independent selection, subscriptions, settings, manifest, and ownership. The
source-library location remains global to a Classick installation.

## 2. Goals

- Preserve Finder, iTunes, Apple Music, and iPod firmware compatibility.
- Require no elevation or SCSI inquiry during ordinary discovery or sync.
- Recognize the same physical iPod on macOS, Windows, and Linux.
- Carry enough Classick state on the iPod for another Classick installation to
  understand it.
- Apply a host setting change before auto-sync admission can observe an older
  device value.
- Keep Classick-created files to the minimum needed for Classick, Apple
  firmware, and optional Rockbox operation.
- Expose hardware facts without presenting guesses as reported facts.
- Replace the two nested JSON protocols with one typed, versioned contract.

## 3. Non-goals and deferred work

### 3.1 Apple initialization

Classick does not initialize a factory-restored iPod in this work. Finder on
macOS or Apple software on Windows must create a valid `iTunesDB`. Classick may
detect an uninitialized device and explain what is required, but it must not
create the initial Apple database or preferences.

Exploring Classick-owned initialization is deferred. It would require a
separate compatibility study covering every initial file, database semantic,
device family, Apple host, and recovery path.

### 3.2 Library identity

A portable identity for the global source library is deferred. Consequently,
this work does not automatically decide whether two hosts point at the same
logical library.

The agreed future behavior is retained: when Classick can prove that a host is
using a different library, it may offer to replace the iPod library after an
explicit prompt and reuse the existing replacement path. It must not infer a
match from path spelling alone.

### 3.3 SCSI enrichment

SCSI inquiry remains an explicit diagnostic or research facility only. It is
not an identity prerequisite, a normal discovery step, or an automatic
elevation trigger. Battery telemetry and other live device channels are also
separate future capabilities.

## 4. Observed Apple baseline

The physical Late-2009 160 GB iPod Classic produced these states:

| State | Relevant observations |
| --- | --- |
| factory restored | HFS+ volume named `iPod`; empty `Device/SysInfo`; no `SysInfoExtended`; no `iTunesDB` |
| after Finder setup | same volume name and UUID; valid `iTunesDB`; device name in the master playlist; new Apple preference files; `SysInfo` still empty; `SysInfoExtended` still absent |

Two immediate live extended-inquiry reads returned different
`RentalClockBias` and opaque `rbsync` values. The response is therefore partly
dynamic and is not a stable device-description file.

These observations establish four boundaries:

1. A filesystem or an empty `SysInfo` does not prove that Apple setup is
   complete.
2. A valid `iTunesDB` is the usable-device baseline.
3. The user-visible iPod name belongs to the iTunesDB master playlist, not the
   filesystem label.
4. Neither Apple initialization nor normal Finder management requires an
   on-disk `SysInfoExtended` on this device.

## 5. Device identity and readiness

### 5.1 Canonical device ID

`device_id` is the USB iSerial/FireWire GUID normalized as exactly 16 uppercase
hexadecimal characters. The `0x` prefix is accepted at input boundaries and
removed in canonical storage and comparison.

The ID is read through ordinary platform device enumeration:

- Windows: SetupAPI and Configuration Manager association from mounted volume
  to its USB parent.
- macOS: IOKit association from mounted media to the USB device.
- Linux: mount/block-device association followed by the sysfs USB-parent walk.

Mount/volume identifiers, labels, names, printed serials, and model numbers are
attributes, not identity substitutes. Without a validated `device_id`,
Classick must not mutate the device.

### 5.2 Hardware facts

Discovery produces facts with provenance rather than one overconfident model
string:

```text
Fact<T> = { value: T, source: reported | decoded | inferred, confidence }
```

- `reported`: read directly through a normal OS/device interface or a valid
  pre-existing device file.
- `decoded`: a deterministic lookup from reported identifiers through a
  versioned Classick hardware catalogue.
- `inferred`: a bounded heuristic, such as capacity disambiguation.
- absent: unknown; the UI does not manufacture a value.

The runtime device record carries family, generation, model code, and colour
when they can be decoded. Capacity and firmware are secondary detail, not the
device's presentation identity. Live battery state is not a persisted hardware
fact. `BatteryPollInterval` is not battery percentage.

USB product ID and capacity can identify a family or generation, but they do
not always identify a colour or exact SKU. A representative silver model code
must never be presented as the physical device's reported model or colour.
A valid foreign `SysInfoExtended` may add reported facts, but production
correctness may not depend on it.

An exact Apple model code deterministically maps to generation and colour; for
example, `MC293` is silver and `MC297` is black. A genuine printed serial maps
to that code by product suffix. Ordinary USB does not always provide either.

For the Classic family, USB PID `0x1261` is shared. Capacity distinguishes the
80 GB and 120 GB variants, but a 160 GB device can be either the 2007 thick
Classic or the 2009 thin Classic, and colour is not reported. Classick uses a
real model code or printed serial from a valid device source when available;
otherwise the exact variant remains unknown. It does not ask the user or store
an appearance override.

Clients use exact artwork when runtime decoding succeeds and a generic
family/model illustration otherwise. If hardware ambiguity changes the safe
`SysInfoExtended` capability profile, Classick must use a separately validated
generation-neutral profile or omit the projection-dependent operation. It must
not resolve a data-safety question through a cosmetic user choice.

### 5.3 Readiness

Mounted Apple-device discovery and sync readiness are distinct:

| State | Meaning | Allowed behavior |
| --- | --- | --- |
| `ready` | `iTunesDB` exists and parses structurally | read and, after all safety gates, sync |
| `needs_apple_initialization` | recognizable mounted iPod structure but no `iTunesDB` | display guidance; no device writes |
| `invalid_database` | `iTunesDB` exists but cannot be validated | display recovery state; no ordinary sync |
| `identity_unavailable` | mounted candidate lacks a valid cross-platform `device_id` | display diagnostics where possible; no mutation |

An invalid database is never silently treated as a fresh device. Recovery from
a Classick-owned, hash-validated transaction backup remains a separate,
explicitly proven recovery path.

## 6. Ownership and on-device layout

### 6.1 Apple-owned state

Apple owns initialization and the semantics of:

- `iPod_Control/Device/Preferences` and the empty or populated `SysInfo`;
- `iPod_Control/iTunes/iTunesPrefs*`, `iTunesControl`, rentals, tones, alarms,
  clocks, and other Apple support files;
- the iPod name and foreign playlists in `iTunesDB`;
- any foreign `SysInfoExtended` already on the device.

Classick may update the iTunesDB, artwork database, media files, and explicitly
owned playlists through its coordinated publication transaction. It must
preserve Apple preferences, the master-playlist name, and foreign playlists.
Finder's auto-sync and manual-management flags are not Classick settings.

### 6.2 Classick-owned state

Classick owns only `iPod_Control/classick/` plus files it can prove it created
inside the existing Apple media layout:

```text
iPod_Control/classick/
  profile.json       config, minimal identity facts, ownership and revisions
  manifest.json      synced-track authority
  playlists/         subscribed definitions only; absent when unused
  pending/           transient journals/rollback; absent when idle
```

Ownership is small and changes with the same coordinated publication as the
profile, so it is embedded in `profile.json`; a separate `ownership.json` is
not required. No host credentials, absolute source paths, mount paths, or
secret-bearing URLs may be stored on the iPod.

Rockbox output remains opt-in and is limited to the files Rockbox needs, such
as Classick-owned `.m3u8` projections. Classick does not install Rockbox.

New database backups, corrupt-file quarantines, and rollback material belong
under `iPod_Control/classick/pending/`, not beside Apple's live database. The
legacy `iTunesDB.classick-backup` and `iTunesDB.corrupt` paths require an
explicit migration/recovery rule; they are not part of the final layout.

The minimum non-music footprint is therefore:

| Data | Required when | Why it cannot be omitted |
| --- | --- | --- |
| `profile.json` | after Classick adopts the device | portable settings/revisions and proof of exactly what Classick owns |
| `manifest.json` | after the first Classick sync | safe delta planning and track ownership |
| subscribed playlist definitions | only when subscriptions exist | another host cannot resolve a subscription from its slug alone |
| `pending/` material | only during/recovering a mutation | crash-safe coordinated publication |
| `Device/SysInfoExtended` | only for a validated profile when no foreign file exists | correct portable libgpod signing/capability and artwork behavior |
| Rockbox projections | only when enabled | Rockbox playlist operation |

There is no additional persistent cache, device marker, setup file, battery
file, library path, host identity, or duplicate hardware inventory on the
iPod. The host may cache richer UI data while disconnected.

### 6.3 Portable profile

The profile contains:

- canonical `device_id` and schema version;
- the libgpod capability-profile ID needed to reproduce safe signing/artwork
  behavior, when resolved; it identifies capabilities, not colour/SKU/artwork;
- selection, settings, and subscriptions values, revisions, and unique mutation
  IDs;
- exact Apple playlist IDs/kinds and Rockbox filenames/hashes Classick owns;
- schema versions and hashes for companion authorities such as the manifest;
- the owned hash for a generated `SysInfoExtended`, when applicable.

The manifest remains separate because it can be large and changes on sync.
Transaction state remains separate because recovery must be possible when the
profile publication itself was interrupted.

This is a strict allowlist. `profile.json` contains no iPod name, display/model
label, exact model code, family/generation label, colour, icon/artwork choice,
capacity, firmware/build, battery information, volume data, host/install ID,
timestamp, telemetry, or cached runtime discovery facts. The iPod name remains
in `iTunesDB`; presentation facts are decoded at runtime or cached only in host
UI state. There is no library-identity field while that design is deferred.

## 7. Host/device configuration reconciliation

### 7.1 Authorities

The device profile is the portable committed authority. Each host keeps a
serial-keyed cache for disconnected display plus a durable mutation outbox.
The cache alone never overrides a connected device.

A host-side UI mutation is an explicit exception: once durably accepted into
the host outbox, that desired value is authoritative until it is published to
the device or the user discards it. This is required so disabling auto-sync
takes effect before connection-time auto-sync admission.

### 7.2 When reconciliation occurs

- **UI mutation while connected:** persist the host intent first, immediately
  use it for runtime decisions, atomically publish the new device profile, then
  acknowledge the correlated UI request and clear the outbox entry.
- **UI mutation while disconnected:** persist and acknowledge the host intent,
  use it for all local presentation/admission decisions, and retain it in the
  outbox until the matching device connects.
- **Device connection:** load the host outbox before considering auto-sync.
  Publish pending host intent first. If no intent is pending, import a newer or
  different device profile into the host cache.
- **Successful sync:** verify configuration hashes/revisions as part of the
  coordinated checkpoint, but do not wait for a music sync to propagate a
  settings mutation.

Selection, settings, and subscriptions keep independent revisions for
targeted UI updates and conflict reporting, while the portable profile is
published through one per-device mutation boundary. Revisions are not used as
a global clock between unrelated hosts. A unique mutation ID settles replays;
pending explicit intent wins, otherwise the connected device wins.

Two hosts with simultaneous offline pending edits resolve by physical publish
order: the last explicitly published edit becomes the device value. The
profile retains the winning mutation ID for idempotency. Wall-clock timestamps
are not stored in the profile and never arbitrate correctness.

## 8. `SysInfoExtended` capability projection

### 8.1 Purpose

An on-disk `iPod_Control/Device/SysInfoExtended` is a stable libgpod
compatibility projection. It is not a raw firmware dump, an Apple preference
file, Classick's settings store, or the canonical device identity record.

Its persistent value is portability: a validated artwork/capability profile
travels with the iPod and can be consumed without SCSI or host-local cache.
The FireWire GUID also allows libgpod to recover the signing identity, although
Classick must still inject the canonical identity into libgpod in memory.

### 8.2 Generation rules

Classick may generate the file only when all of these are true:

1. the iPod is `ready`;
2. `device_id` is validated;
3. the exact or generation-neutral capability profile is validated for the
   resolved hardware evidence;
4. no foreign `SysInfoExtended` exists;
5. the write participates in the device transaction and ownership record.

The generator uses typed, versioned capability data. It must not substitute a
GUID into a community donor plist or persist a live SCSI response.

Because libgpod treats the presence of `SysInfoExtended` as authoritative for
artwork formats, a generated projection must contain complete, internally
consistent format arrays. A three-key or otherwise partial file is unsafe.

### 8.3 Stable contents

The projection includes the stable fields required by the validated profile:

- `FireWireGUID`;
- `FamilyID` and `DBVersion` when validated;
- complete `AlbumArt`;
- complete `ImageSpecifications`;
- complete `ChapterImageSpecs`;
- `SupportsSparseArtwork` and `SQLiteDB`;
- any additional stable libgpod-consumed capability whose omission would
  materially change database or artwork behavior.

Every image-format record carries the complete relevant structure, including
format ID, dimensions, pixel format/order, interlace, crop, row-byte alignment,
rotation, background/colour/gamma adjustments, and associated format where the
profile defines them.

### 8.4 Excluded contents

The projection excludes:

- `rbsync`, `RentalClockBias`, rental, DRM, or other opaque mutable blobs;
- hot-plug, bus, disk-mode, corruption, recovery, or current-volume state;
- host, updater, pairing, and owner-specific fields;
- serial numbers, firmware builds, OEM values, and inferred or donor SKU/colour
  identifiers;
- battery polling/status fields;
- Classick metadata or ownership markers.

Here, the earlier phrase "model claims" meant unverified donor or inferred
exact-SKU/colour values; it did not mean that Classick refuses to identify the
device. Exact model facts are decoded at runtime when evidence permits.
`ModelNumStr` is not useful in the generated plist because the pinned parser
does not consume that key there. `SerialNumber` can let generic libgpod decode
model/colour, but Classick does not need to duplicate that private identifier:
it sets the validated model in memory. Adding a real reported serial later
would require a demonstrated interoperability need and a capability-schema
revision; a fabricated or donor serial is never allowed.

The generated plist needs no colour key; the physical live plist had none.
Colour comes from a genuine model/serial or remains unknown, in which case the
clients use generic artwork.

### 8.5 Existing-file policy

- **Absent:** generate the projection when the rules above are satisfied.
- **Present and hash-owned by Classick:** validate it and replace atomically
  only when the capability schema changes or the owned bytes are corrupt.
- **Present and foreign:** preserve it byte-for-byte. Parse it read-only for
  facts and validate its libgpod capability completeness. Do not overwrite,
  normalize, remove, or add Classick keys to it.

If a foreign file is incomplete or inconsistent, Classick preserves it and
blocks only operations that cannot be made safe with it. The implementation
must not silently replace foreign data to regain artwork support.

A donor-style file written by a pre-protocol Classick release has no durable
ownership proof. Even if its bytes match a legacy embedded template after GUID
normalization, treat that match only as a migration hint. Preserve it until the
user explicitly authorizes replacement, or until a separate transaction record
proves Classick ownership. Never classify it as owned from content resemblance
alone.

## 9. Unified JSON protocol

### 9.1 One schema, multiple transports

The current subprocess `1.x` and daemon `2.x` enums become one breaking major
protocol, initially `3.0.0`. The same Rust wire module and serialized message
types are used over:

- desktop client to daemon named pipe or Unix socket;
- daemon to owned sync worker stdin/stdout;
- CLI/TUI adapters where structured progress is required.

Endpoints advertise a role and capabilities in `hello`. A role accepts only
the command subset appropriate to it, but messages do not gain a second schema
or nested encoding.

### 9.2 Envelope

Every message uses one flattened tagged-object shape with shared routing and
correlation fields:

```json
{
  "type": "track_done",
  "device_id": "000A2700...",
  "session_id": 42,
  "result": "applied"
}
```

Fields that are meaningless for a message are omitted. `device_id` is required
for device-specific commands and events. `session_id` is required for
session-specific progress and decisions. `request_id` is required for queries
and mutation commands and is echoed by the canonical acknowledgement that
settles it. Mutation IDs remain the persisted idempotency identity and are not
replaced by connection-scoped ordering.

### 9.3 Progress forwarding

The daemon decodes worker events into the shared Rust type, validates the
worker role and session routing, and emits the typed event to clients. It no
longer embeds a JSON line inside `sync_event.line`. Unknown additive events may
be ignored according to negotiated compatibility; malformed or misrouted
events fail the owned session rather than crossing the external wire.

### 9.4 Compatibility

Protocol `3.0.0` is a coordinated Rust, Swift, and C# release. The daemon does
not accept major `1` or `2` messages on the new endpoint. Persisted-state
migrations are separate from wire compatibility and remain explicit.

## 10. Research authorities

- The pinned libgpod
  [`SysInfoExtended` parser](https://github.com/fadingred/libgpod/blob/4a8a33ef4bc58eee1baca6793618365f75a5c3fa/src/itdb_sysinfo_extended_parser.c)
  defines the scalar and complete image-format fields it consumes.
- The pinned libgpod
  [device reader and fallback selection](https://github.com/fadingred/libgpod/blob/4a8a33ef4bc58eee1baca6793618365f75a5c3fa/src/itdb_device.c)
  show that a present extended plist supplies the artwork lists in place of
  the generation fallback.
- The pinned libgpod
  [hash58 implementation](https://github.com/fadingred/libgpod/blob/4a8a33ef4bc58eee1baca6793618365f75a5c3fa/src/itdb_hash58.c#L1180-L1255)
  consumes the FireWire ID for signed Classic database publication.
- Redacted fixtures will be derived from the ignored local captures under
  `device-dumps/2026-07-19-factory-restored/` and
  `device-dumps/2026-07-19-finder-setup/`; raw private captures are research
  evidence, not repository inputs.

## 11. End-to-end connection story

1. The platform backend associates a mounted candidate with its ordinary USB
   identity and canonicalizes `device_id`.
2. Classick classifies readiness without writing.
3. For a ready device, it reads the iPod name, portable profile, manifest,
   ownership, and any pending transaction.
4. Recovery runs before configuration reconciliation or sync planning.
5. A pending host mutation is applied before auto-sync admission; otherwise
   the device profile refreshes the host cache.
6. Classick validates or provisions its stable `SysInfoExtended` projection
   without touching a foreign file.
7. The global host library, per-device selection, and subscribed playlists
   produce the desired set. Library-identity comparison is deferred.
8. The existing coordinated transaction publishes media, iTunesDB, artwork,
   playlists, portable manifest, ownership, and profile authorities.
9. Typed progress and terminal state travel over the unified protocol, keyed
   by `device_id` and `session_id`.
10. Finder/iTunes may later manage the same initialized iPod; Classick-owned
    state remains isolated and foreign Apple state is preserved.

## 12. Acceptance criteria

- A freshly restored but uninitialized iPod is recognized as needing Apple
  setup and receives no Classick writes.
- The physical initialized Classic is recognized by the same canonical ID on
  macOS and Windows without SCSI or elevation; Linux has equivalent library
  behavior and tests.
- Two iPods retain independent portable settings, selection, subscriptions,
  manifest, and ownership.
- Disabling auto-sync while disconnected prevents auto-sync when that iPod is
  next connected, before device-state import.
- Another host imports the device profile when it has no pending local edit.
- A Classick-generated `SysInfoExtended` contains only validated stable fields
  and complete format arrays; no donor or dynamic values appear.
- A foreign `SysInfoExtended` remains byte-identical after discovery and sync.
- A legacy donor-style file is preserved unless ownership is proven or the
  user explicitly authorizes replacement.
- No new Classick backup or quarantine is placed beside Apple's live
  `iTunesDB`.
- Finder/iTunes can read and manage the post-Classick iPod, and the iPod
  firmware displays music and artwork correctly.
- Swift, C#, and Rust use one protocol major and no event contains nested raw
  JSON.
