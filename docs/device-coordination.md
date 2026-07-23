# Device coordination architecture

Status: implemented for mounted iPod Classic mutations. Later device-family
profiles and the Apple mobile-device transport remain deferred.

## Purpose

Classick and Apple software can both produce valid databases for supported
iPods, but two writers must not update the same device state concurrently.
Classick therefore needs to:

- exclude every other Classick writer for the full mutation session;
- detect and preserve changes made by non-cooperating software;
- publish each device family's authoritative files as one logical generation;
- recover safely after a crash without overwriting an unknown newer state;
- operate as a standard user, without stopping Apple services or locking an
  entire mounted volume; and
- communicate contention accurately instead of claiming that iTunes cannot
  read a Classick-managed iPod.

The architecture is capability-based. An iPod Classic, a Shuffle, a later Nano,
and an iPod touch do not share one database layout or transport, so they must
not be forced through one hard-coded `iTunesDB` locking path.

## Evidence and platform constraints

The design is grounded in the following primary sources:

- libgpod's
  [`itdb_start_sync`](https://sources.debian.org/src/libgpod/0.8.3-17/src/itdb_itunesdb.c/#L6215)
  is a no-op on regular, mounted iPods. There is no hidden iPod Classic lock.
- libgpod's
  [iPhone-family implementation](https://sources.debian.org/src/libgpod/0.8.3-17/src/itdb_iphone.c/)
  uses `lockdownd`, AFC, Apple sync notifications, and an exclusive lock on
  `/com.apple.itunes.lock_sync`. That protocol is the correct coordination
  mechanism for supported iPod touch devices.
- Microsoft's [`LockFileEx`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)
  provides crash-released byte-range locking without requiring elevation, but
  it only protects a file range. It does not reserve a volume from arbitrary
  writers.
- Apple's [`flock(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/flock.2.html)
  is explicitly advisory. It is suitable for cooperating Classick processes,
  not for excluding Finder, Music, or other POSIX writers.
- Apple's [`NSFilePresenter`](https://developer.apple.com/documentation/foundation/nsfilepresenter)
  is not notified about low-level writes that bypass `NSFileCoordinator`.
- Windows [`FSCTL_LOCK_VOLUME`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_lock_volume)
  provides real volume exclusion, but a successful lock makes ordinary mounted
  path access unavailable and NTFS treats the volume as dismounted. It is
  incompatible with Classick's mounted libgpod workflow.
- Apple documents how to disable automatic device syncing in
  [iTunes](https://support.apple.com/en-gb/guide/itunes/itns5ecc4d98/windows),
  [Apple Devices](https://support.apple.com/guide/devices-windows/turn-automatic-syncing-on-or-off/mchlda9a4a0e/windows),
  and [Music/Finder](https://support.apple.com/guide/music/connect-your-device-to-your-mac-mus86d751aec/mac).

Apple does not publish an iPod Classic `iTunesDB` locking protocol for
third-party applications. The design can therefore guarantee exclusion among
Classick processes and prevent known lost updates, but it cannot claim that
Apple software is compelled to honor Classick's lease.

## Safety invariants

1. At most one Classick mutation session may own a physical device.
2. Apart from idempotently creating the dedicated lease path on first use, the
   lease is acquired before recovery or any device-state write and is held until
   verification, cleanup, and terminal reporting finish.
3. Every mutating entry point requires a live mutation-session guard. Helper
   APIs must not make bypassing the guard convenient.
4. A Classick publication may replace only the exact live generation from
   which its candidate was derived, or a generation that the same session
   previously published and verified.
5. An unknown live generation is preserved. Classick must not publish or roll
   back over it.
6. A device profile defines the complete authoritative file set. `iTunesDB`
   alone is not a universal transaction boundary.
7. Process and open-handle detection provides diagnostics only. It is never the
   correctness authority.
8. Failure to acquire or validate coordination fails closed before mutation.
9. Source-library access remains read-only and outside the device lease.
10. No coordination path requires administrator privileges, stops a service,
    kills another application, or unmounts the device.

## Core model

The Rust core uses explicit capability types rather than adding more preflight
calls to individual modes.

```rust
struct DeviceSyncProfile {
    transport: DeviceTransport,
    coordination: CoordinationKind,
    database: DatabaseFamily,
    checksum: ChecksumKind,
    authority: AuthoritySet,
}

enum DeviceTransport {
    MountedFilesystem,
    AppleFileConduit,
}

enum CoordinationKind {
    CooperativeFileLease,
    AppleMobileSync,
}

struct DeviceMutationSession {
    device: ResolvedDevice,
    profile: DeviceSyncProfile,
    lease: DeviceLease,
    expected_generation: DeviceGeneration,
}
```

`DeviceMutationSession` is the only supported gateway to device mutation. It
owns the resolved mount or AFC connection, stable identity, profile, exclusive
lease, and expected generation. Its lifetime must enclose recovery, staging on
the device, checkpoint publication, rollback, cleanup, and post-sync mirrors.

Normal sync, replace-library, Rockbox backfill, database restore, and future
mutating maintenance commands must share this gateway. Read-only scan, audit,
and verification commands do not require an exclusive mutation session.

Replace-library passes one existing `DeviceMutationSession` through wipe and
rebuild; nested acquisition is forbidden.

## Mounted-device lease

Mounted iPods use a stable Classick-owned sidecar derived from the detected
control directory, for example:

```text
<iPod_Control-or-iTunes_Control>/classick/device.lock
```

The file is never atomically replaced or deleted during normal operation. Its
continued existence does not indicate ownership; only the live OS lock does.

- Windows opens the sidecar and takes a non-blocking exclusive `LockFileEx`
  lock on byte range `[0, 1)`.
- macOS takes non-blocking `flock(LOCK_EX | LOCK_NB)` on the open sidecar.
- The owning handle remains open for the entire mutation session.
- Normal scope exit explicitly unlocks and closes the handle. Process death or
  forced termination closes it through the operating system.
- An unsupported lock operation, inaccessible sidecar, unexpected file type,
  or invalid path is `coordination_unavailable`, not permission to continue.
- The sidecar stores no secrets. Optional diagnostics may be written only after
  acquiring the lock, and their contents are never used to decide whether a
  lock is stale.

On first use, contenders may race to create the `classick` directory and stable
sidecar. Creation must be idempotent and open-or-create the same final file; no
temporary-name replacement is allowed. Neither bootstrap write belongs to the
device database generation.

The opener must reject symlinks or other path redirection where the platform
supports them, verify that the resolved file remains under the already verified
device mount, and avoid following an attacker-controlled replacement. FAT
normally lacks symlinks, but HFS+-formatted devices do not justify weakening the
check.

Lock behavior on FAT32 and HFS+ must be verified on physical devices before the
feature is declared supported. If either filesystem returns an unsupported-lock
error, the implementation may introduce a host-side kernel lease keyed by raw
device identity; it must not silently fall back to an existence-based lockfile.

## Mobile-device lease

iPod touch support is a separate transport backend. For libgpod-supported touch
generations, Classick must preserve the upstream protocol:

1. establish the trusted `lockdownd` session;
2. start AFC;
3. post the sync-will-start notification;
4. open `/com.apple.itunes.lock_sync`;
5. request the exclusive AFC lock;
6. post sync-did-start;
7. retain the AFC lock across the complete mutation;
8. unlock, close, and post sync-did-finish.

A host-side Classick lease is still required to serialize local setup and
connection work before the device-native lock is obtained. AFC publication and
recovery must be implemented behind the same `DeviceMutationSession` contract;
mounted-filesystem rename assumptions must not leak into this backend.

The current Classick build removes libimobiledevice, libplist, and SQLite
generation. Restoring and modernizing those dependencies is a prerequisite,
not part of the initial mounted-device implementation.

## Device generations and conflict fencing

`DeviceGeneration` is a deterministic manifest of the profile's authoritative
state. Each entry records:

```text
normalized relative path
existence and regular-file type
byte length
cryptographic content digest
```

Directory enumeration is sorted and path-normalized. Modification time may be
recorded as a fast diagnostic but is not a correctness token. A changed file
with a preserved timestamp must still be detected.

The initial generation is captured after acquiring the lease and completing
safe journal inspection, but before any recovery or new mutation that might
replace authoritative state. Recovery then proves whether the live generation
matches a journaled Classick generation, its verified predecessor, or neither.

Before every publication:

1. recompute the relevant live generation;
2. compare it with the session's expected generation;
3. stop with `external_generation_changed` if it differs;
4. publish the staged candidate using the profile's coordinated transaction;
5. reopen and validate the published database set;
6. compute and store the newly verified expected generation.

Content hashing should be bounded to authoritative metadata, not the entire
music payload. Media files are governed by the journal's owned-path set and
their existing content hashes.

Atomic replacement prevents a partial file, but it is not a compare-and-swap.
There remains a narrow interval between the final generation check and
publication in which a non-cooperating writer could act. No mounted, no-admin
primitive closes that interval on both supported desktop platforms. The design
must document this limitation and keep the interval small; it must not imply a
stronger guarantee in UI copy.

## Conflict-aware recovery

Recovery classifies the live generation before changing anything:

- `KnownCurrent`: exactly the last verified generation from the journal;
- `KnownPrevious`: exactly the recorded pre-publication snapshot;
- `Unknown`: neither known generation.

`KnownCurrent` may continue cleanup or a journaled rollback according to the
recorded transaction phase. `KnownPrevious` may discard an unpublished
candidate or resume safe work. `Unknown` blocks automatic recovery and preserves
the live files, journal, snapshots, and candidates for diagnosis.

This rule supersedes any unconditional rollback. Restoring a snapshot over an
unknown generation could destroy a legitimate iTunes, Music, Finder, or other
tool update and is therefore a data-loss defect.

An abandoned Windows mutex, stale journal, process crash, or persistent
sidecar file is not itself evidence that the device is corrupt. Only the
journal plus generation comparison determines recovery.

## Family profiles

### Original database profile

Applies to the original iPod generations, Photo, Mini, Video, Nano 1G-4G, and
Classic 1G-3G, subject to per-model validation. The authority set includes the
live database, artwork database and managed thumbnails where supported,
play-count inputs that libgpod consumes or renames, and Classick manifest,
playlist-ownership, and projection state.

Unsigned, hash58, and other checksum differences belong in the profile; they do
not change the lease invariant.

### Shuffle profile

Shuffle firmware consumes `iTunesSD` in addition to `iTunesDB`. The authority
set and transaction must include both databases and relevant Shuffle status or
statistics inputs. A checkpoint is not successful unless the pair represents
the same library generation.

### Compressed/SQLite Nano profile

Nano 5G and 6G use `iTunesCDB`, SQLite databases, and additional hash-dependent
state. Their authority is a database bundle rather than one file. Support
requires restoring upstream SQLite/libplist functionality, provisioning the
required per-device identity, and solving the model's supported checksum path.

Nano 6G hashAB support also depends on external model-specific material loaded
by libgpod. Distribution, provenance, and on-device correctness must be resolved
before the family can be advertised. Nano 7G is absent from libgpod 0.8.3's
model table and is not covered by this architecture alone.

### iPod touch profile

Supported historical iPod touch generations use the mobile-device lease and
AFC transaction backend. They must not be treated as mounted volumes or inherit
filesystem-sidecar assumptions.

## Process and helper detection

The current process-name preflight becomes diagnostic enrichment:

- The mere presence of Apple Mobile Device Service is not contention and must
  not block Classick.
- Classick never stops or restarts Apple services.
- Windows Restart Manager may identify processes currently holding specific
  authority files, but its result is advisory and path-limited.
- Finder is normally always running; its process existence is meaningless.
- Music or iTunes being open is not equivalent to a device write.
- Generation comparison, not process identity, decides whether publication is
  safe.

Users must be instructed to disable automatic Apple syncing before enabling
Classick automatic sync. This reduces the chance of an uncoordinated writer but
does not replace the runtime safety checks.

## Errors, IPC, and UI

Coordination failures need structured, platform-neutral reasons so both UIs can
present the correct recovery action:

| Reason | Meaning | Manual action | Automatic action |
|---|---|---|---|
| `classick_device_busy` | Another Classick session owns the lease | Retry or cancel | Defer with bounded backoff |
| `apple_device_busy` | Apple activity is positively identified | Wait, then retry | Defer |
| `external_generation_changed` | Authoritative bytes changed unexpectedly | Stop other sync activity, then re-read/retry | Stop and require attention |
| `coordination_unavailable` | Classick cannot establish a trustworthy lease | Show diagnostic; do not sync | Disable/defer and record failure |
| `unknown_recovery_generation` | Recovery cannot prove ownership of live state | Preserve everything and require attention | Never retry destructively |

The wire representation may extend an existing error/prompt payload or add a
typed field, but it must remain additive within the current protocol major and
be implemented in Rust, Swift, and C# together.

Static UI copy must say that Classick and Apple software can both manage a
compatible iPod but must not sync simultaneously. It must not say that iTunes
will inherently reject every libgpod-managed database.

## Implementation boundaries

The implemented slice covers mounted Classic devices and places every existing
device mutation path behind the reusable session guard.

Suggested later slices are:

1. structured coordination-specific failure presentation beyond the current
   typed protocol errors;
2. original, Mini, Photo, Video, Nano 1G-4G, and Shuffle profiles with physical
   verification for every claimed family;
3. Nano 5G/6G dependency, identity, checksum, and database-bundle research;
4. a separately scoped libimobiledevice/AFC backend for historical iPod touch.

Later-family support must not weaken the initial Classic guarantees or turn
unverified model detection into a claim of write support.

## Verification requirements

Automated verification must include:

- two real child processes contending for one mounted-device lease;
- different fake devices acquiring leases concurrently;
- release after normal return, error, panic/unwind, and killed child;
- a persistent unlocked sidecar not blocking a new session;
- lock survival across `iTunesDB` replacement and rename;
- fail-closed behavior when locking is unsupported or the path is redirected;
- external modification injected before publication;
- external modification injected before rollback;
- recovery preservation of an unknown generation;
- one lease spanning replace-library wipe plus rebuild;
- guarded coverage of normal sync, recovery, restore, backfill, checkpoints,
  cleanup, and post-sync mirrors;
- profile-specific authority-set tests, including `iTunesSD` for Shuffle and
  database bundles for later Nano profiles; and
- exhaustive Rust, Swift, and C# decoding of any new structured failure reason.

The bounded HFS+ mounted-core publication gate passed on 23 July 2026,
including generation-fenced stale-session abandonment, five-track
publication, persisted artwork reopen, transaction cleanup, source
byte-stability, and graceful daemon shutdown without unmounting. The same
device subsequently passed firmware playback and visible-artwork checks,
Finder Manage Storage deletion, and a generation-fenced Classick repair of the
five externally removed DB/media pairs. The remaining physical work is an
explicit clean-eject observation, Apple Music behavior beyond Finder's
management surface, and the same lock/publication behavior on a FAT-formatted
Classic and Windows. Additional families are advertised only after their own
read, write, Apple interoperability, interruption, and clean-eject tests pass.
The music share remains read-only throughout.

## Rejected alternatives

| Alternative | Reason rejected |
|---|---|
| Keep only process-name checks | Racy, overly broad, and not device-specific |
| Lock the live `iTunesDB` | Rename replaces the locked file identity and restrictive Windows sharing can block Classick itself |
| Existence-based lockfile | Leaves stale locks after crashes and requires unsafe ownership guessing |
| Stop Apple Mobile Device Service | Privileged, host-invasive, restartable, and not proof of device writes |
| `NSFileCoordinator` | Non-participating low-level writers are not coordinated |
| Whole-volume lock or unmount | Incompatible with mounted libgpod access and the no-admin requirement |
| Unconditional rollback | Can overwrite a legitimate external generation |
| One Classic-specific authority list | Incorrect for Shuffle, compressed/SQLite Nano, and AFC devices |

## Decision summary

Classick uses a capability-based mutation session. Mounted iPods receive a
crash-safe cooperative Classick lease plus optimistic generation fencing and
coordinated publication. Historical iPod touch support, if restored, uses
libgpod's Apple mobile-device handshake and AFC lock behind the same session
contract. Unknown external generations are preserved, never overwritten.

This is the strongest architecture compatible with standard-user operation and
mounted iPod support. It deliberately distinguishes guarantees Classick can
enforce from behavior Apple software is not documented to honor.
