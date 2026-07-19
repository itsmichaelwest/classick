# Device and data safety

This document collects the invariants that protect user data. Changes touching
the iTunesDB, artwork, manifests, playlists, recovery, or device discovery must
preserve them and add a regression test where practical.

## Apple initialization boundary

Classick manages an Apple-initialized iPod; it does not create the initial
Apple device structure. Ordinary mutation requires a structurally valid
`iPod_Control/iTunes/iTunesDB`. A recognizable restored device without that
database is reported as needing setup in Finder or the applicable Apple
software and receives no Classick writes.

An existing but invalid database is not an uninitialized device. Preserve it
and block ordinary sync unless a Classick-owned transaction proves an exact,
hash-validated recovery. Classick-owned initialization is deferred to a
separate compatibility design.

## Device identity

The mutation authority is the canonical 16-hex USB iSerial/FireWire GUID read
through ordinary platform USB enumeration. Mount paths, drive letters, volume
GUIDs/UUIDs, filesystem labels, display names, model guesses, and SCSI inquiry
are not substitutes. If that identity is unavailable or disagrees with a
portable authority/journal, Classick does not mutate the device.

Hardware facts retain their reported, decoded, or inferred provenance. Unknown
colour, SKU, firmware, or battery data must remain unknown rather than being
filled from a representative model.

## Apple and Classick ownership

Apple preferences, the user-visible iPod name, foreign playlists, and a
foreign `SysInfoExtended` remain foreign. Classick may mutate database,
artwork, media, and playlist records only through explicit ownership and the
coordinated transaction below. Finder/iTunes sync preferences are not Classick
settings.

Classick-created portable state lives under `iPod_Control/classick/`. Do not
scatter Classick metadata through Apple files or store credentials, absolute
source paths, or host-specific mount information on the device. Rockbox files
are created only when enabled and only where Rockbox requires them.

The portable profile contains operational reconciliation and ownership data
only. Names, model/colour/icon presentation, capacity, firmware, battery,
telemetry, timestamps, and host/install identity are runtime or host-cache data
and must not be serialized into it.

New backup, quarantine, journal, and rollback files live below the Classick
subtree. Legacy Classick files beside `iTunesDB` are migration inputs; preserve
ambiguous bytes and do not create more of them.

## `SysInfoExtended`

A Classick-generated `iPod_Control/Device/SysInfoExtended` is a stable,
versioned libgpod capability projection. It is not a raw SCSI response or the
portable Classick profile.

- Generate it only for a ready device with validated identity and an exact,
  validated capability profile.
- Preserve a foreign existing file byte-for-byte.
- Track Classick ownership by exact hash in Classick state, never by inserting
  custom keys into the plist.
- Include complete artwork/image/chapter format arrays and required stable
  libgpod capabilities. A partial present file can suppress libgpod's safer
  fallback tables.
- Exclude `rbsync`, `RentalClockBias`, rental/DRM data, live connection or
  volume state, battery fields, host/updater data, and donor identity/firmware.
- Publish or replace an owned file atomically inside the coordinated device
  transaction.

The detailed target contract is in
[Native device protocol and identity](design/2026-07-19-native-device-protocol.md).

## Source library is read-only

Classick may enumerate, stat, hash, decode, transcode from, and read tags or
artwork from the configured source. It must never rename, delete, rewrite, or
create files in that source tree. Temporary and derived output belongs in
Classick state/staging directories or on the target device.

## Apple software and concurrent mutation

On-device verification shows that a Classick-managed database is not
intrinsically unreadable to iTunes or Music. The current running-process
preflight is a conservative attempt to avoid concurrent mutation, not a
workaround for permanent format incompatibility.

Classick and Apple software must not write the same device state concurrently.
Until the proposed [device coordination architecture](device-coordination.md)
is implemented, users should close active Apple device-sync interfaces and
disable automatic syncing before Classick mutates an iPod. Apple Mobile Device
Service merely running is not proof that a write is in progress.

## Publication is coordinated

Database, artwork, playlists, ownership, and manifests are not independent
files. A successful checkpoint must represent one coherent device state.

- Snapshot `iTunesDB`, `ArtworkDB`, every managed `.ithmb`, the device
  manifest, and playlist ownership needed for rollback.
- Hash-validate snapshots and live inputs before destructive transitions.
- Reapply FirewireGuid/ModelNumStr when a fresh candidate DB is opened.
- Resolve playlist membership against the post-staging DBIDs.
- Publish device authority before refreshing warning-only host caches.
- Remove only files explicitly owned by the active journal.

Periodic checkpoints bound orphan exposure, but they do not weaken the
transaction ordering.

## Recovery precedes new work

Pending journals are recovered before diff planning or a new device mutation.
Recovery accepts only an exact mount, raw serial, session identity, schema, and
owned-path set. Corrupt, foreign, ambiguous, or escaping journals stay in place
and block mutation; they are never guessed away.

A verified mismatch rollback is terminal. It restores the exact database,
artwork, manifest bytes-or-absence, and other recorded authorities, then records
`RollbackComplete`. It must not demote the transaction into a replayable phase.
Rollback is permitted only when the live bytes match a generation the journal
proves Classick owns. An unknown external generation must be preserved and
block destructive recovery, as specified by the proposed
[device coordination architecture](device-coordination.md).

## Cancellation drains finalization

Cancel, pause, UI quit, OS signals, and daemon-parent death converge on one
bounded drain. The daemon sends one stop command, retains session admission,
consumes continuing progress, and waits for the terminal event plus EOF. The
watchdog is inactivity-based and resets on progress; a fixed total-duration cap
can kill healthy publication.

## Artwork rewrite rule

libgpod can drop existing thumbnails when rewriting a parsed database. Any path
that writes a database must either preserve the coordinated artwork snapshot or
re-thumbnail every relevant track and force the fresh artwork-build path.
Global `.ithmb` presence is not proof that a specific track has a valid artwork
record.

## Track and playlist ownership

- Delete a track only after unlinking it from every playlist.
- Reconcile disk orphans and dangling DB references together.
- Apple playlist ownership is the recorded libgpod ID plus expected structural
  kind, never a display name.
- Rockbox ownership is the exact recorded relative filename and content hash.
- If recorded bytes were externally modified, preserve them as foreign and
  settle Classick output at a deterministic alternate name.
- Never broadly sweep empty smart playlists, firmware playlists, On-The-Go,
  podcasts, or same-name foreign files.

## Filesystem publication

Use no-replace creation for new projection files. A deletion quarantine must be
derivable from durable authority and validated on recovery. After unlinking,
sync the containing directory before advancing the journal, including a retry
where the target is already missing.

Directory handles and Windows handle-bound operations narrow path races, but no
supported macOS/Linux primitive provides inode-CAS unlink. Finalization assumes
Classick is the sole cooperative writer; hostile same-user leaf swaps are
outside the supported threat model.

## Fake-device tests

Fake mounts must contain `iPod_Control/Music/F00`; libgpod selects an existing
`F##` directory and does not create the first one. Tests must use isolated
config, history, state, and IPC paths and must never discover the developer's
real daemon, source share, or device.
