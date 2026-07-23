# Classick architecture

Classick is a cross-platform iPod Classic synchronization system. A Rust core
owns device discovery, library indexing, selection, transcoding, iTunesDB
publication, recovery, and daemon state. Native Windows and macOS applications
own the daemon process and present the same `DeviceId`-keyed model over
protocol 3.

## Components

### Rust core

The `classick` binary has three entry modes:

- `--daemon`: long-lived device watcher, scheduler, state authority, and IPC
  server.
- `--ipc-mode`: one sync worker controlled over stdin/stdout.
- interactive/default: CLI/TUI execution without a desktop owner.

The library index and host configuration live below the platform config/data
directory. Device-specific authority is keyed by a canonical 16-hex
`DeviceId`, obtained from ordinary USB identity. The daemon never treats a
display name, mount path, volume identifier, or legacy configuration key as a
substitute.

### Windows application

The WinUI 3 tray application owns `classick.exe --daemon` and connects through
`\\.\pipe\classick`. It bundles the release Rust binary and the libgpod runtime
DLL closure. Ordinary mounted-device discovery uses zero-access volume queries
plus SetupAPI and Configuration Manager; it never issues a SCSI request or asks
for elevation. When explicitly invoked, the Windows-only SCSI diagnostic may
inspect and report extended data; discovery, signing, and provisioning never
call it automatically. The
[native device protocol design](design/2026-07-19-native-device-protocol.md)
defines ordinary OS USB enumeration as authoritative.

### macOS application

The SwiftUI application owns `classick --daemon` and connects through the Unix
socket returned by the daemon's platform default. It is intentionally not App
Sandboxed because the daemon needs raw removable-volume access and a shared
socket. macOS uses the system `afconvert`; it never bundles or requires ffmpeg.

## State authorities

| Concern | Authority |
| --- | --- |
| configured/remembered devices | version-2 `devices/registry.json`, keyed by canonical `DeviceId` |
| portable per-device configuration | `iPod_Control/classick/profile.json` when committed |
| disconnected configuration and pending edits | `devices/<DeviceId>/cache.json` plus `outbox.json` |
| source library contents | `library-index.json`, refreshed by the scan subprocess |
| manual/smart playlists | host playlist store |
| synced track state | device manifest when connected; serial-keyed host cache only for disconnected display |
| managed Apple playlists | device ownership record keyed by libgpod playlist ID |
| managed Rockbox projections | device ownership record keyed by filename and content hash |
| active session | daemon runtime state keyed by `DeviceId` and session ID |
| durable UI intent | client outbox until canonical correlated acknowledgement |

`config.toml`'s legacy `ipod_identity` and the old per-device selection,
settings, subscriptions, and managed-playlist files are retained migration
inputs. A one-time import writes the complete host outbox before the cache and
does not delete the legacy bytes.

Pending explicit host intent wins when the device reconnects. With no pending
intent, another host imports the portable profile. The source-library location
remains global and host-owned; portable library identity is deferred.

## Sync flow

1. The daemon observes a device and classifies identity/readiness without
   writing it.
2. For an adopted device, pending portable recovery and reconciliation run
   under the device lease before auto-sync admission.
3. Source availability is resolved without changing the configured logical
   source identity.
4. Pending host and device journals recover before a new diff is planned.
5. The library walk and selection/playlist union produce the effective sync
   set.
6. The apply loop stages album-bounded work and checkpoints periodically.
7. A coordinated transaction publishes database, artwork, playlists, device
   manifest, ownership records, and warning-only host mirrors in their required
   order.
8. The daemon retains admission through finalization, terminal history, and
   subprocess EOF.

Cancellation and pause stop admission at an album boundary. They do not kill a
subprocess during publication. Client shutdown, OS termination, and parent
death converge on the same drain path.

## Playlist model

Host playlists have stable slugs. Manual playlists store ordered
source-relative track paths; smart playlists store deterministic rules. Device
subscriptions extend the selected library set before syncing.

Classick modifies only playlists for which it has positive device authority:

- Apple playlists are owned by recorded libgpod ID, never name.
- Rockbox files are owned by exact recorded filename and bytes hash.
- firmware, On-The-Go, podcast, arbitrary smart, and foreign same-name
  playlists remain untouched.

## Source-library boundary

The source library is input-only. Scans, tag reads, artwork reads, and
transcoding open source files for reading. Generated audio, indexes, manifests,
journals, and playlists are written only to Classick state directories, staging
areas, or the target iPod.

Logical source identities are portable across mount roots. SMB credentials are
never serialized to config, logs, IPC errors, or manifests.

## Cross-platform boundaries

- Device enumeration and mount resolution have platform backends.
- Windows subprocesses that can display a console use
  `windows_proc::NoConsoleWindow`.
- macOS transcoding uses `afconvert`; Windows uses ffmpeg or optional refalac.
- Named-pipe, Unix-socket, and worker transports use protocol 3 typed messages.
- FAT/exFAT and host filesystems do not provide identical rename/unlink
  primitives, so publication uses journals, validation, no-replace creation,
  directory synchronization, and a documented single-writer finalization
  model.

## Release boundaries

The macOS app is signed, notarized, distributed with Sparkle, and embeds the
release daemon. The Windows application bundles the Rust executable and native
DLLs. Protocol compatibility is checked at connection time; release versions
do not substitute for wire protocol versions.
