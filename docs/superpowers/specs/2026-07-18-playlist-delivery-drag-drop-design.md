# Playlist Delivery and Native Drag-and-Drop Design

**Status:** approved

**Date:** 2026-07-18

**Scope:** iTunesDB playlist integrity, firmware-system playlist
normalization, Rockbox `.m3u8` projection, and native macOS library
drag-and-drop. This extends the approved multi-device stabilization program
after Plans 1–5.

## 1. Problem statement

Classick has one logical playlist model but must deliver it to two independent
firmwares. Apple firmware reads playlists from iTunesDB through libgpod;
Rockbox reads ordinary `.m3u8` files. The current implementation only publishes
the Apple representation.

The mounted physical iPod also exposes six empty smart playlists that all
appear as `Videos`. A removed prototype swept every empty smart playlist, which
could delete legitimate foreign playlists. Read-only inspection proved a much
narrower pattern: all six records have the same firmware-system smart-rule
payload and differ only by ID and timestamp. Classick never creates smart
playlists, while a bare libgpod parse/write does not add another copy. The
remaining causal question—whether Apple firmware adds one on every boot or
only after a libgpod-authored write—belongs in the final physical verification
gate.

The macOS Library also needs direct manipulation. A user should be able to drag
an artist, album, or genre onto an iPod to include it, or onto a manual playlist
to append it. The removed implementation inferred reply ownership from FIFO
order and could lose or misroute mutations across reconnects. Drag-and-drop
must therefore build on the serial targeting, ordered delivery, request IDs,
revisions, and durable acknowledgements from Plans 1, 4, and 5.

## 2. Product decisions

- Delivery is split into Plans 6A, 6B, and 6C: playlist integrity first,
  Rockbox projection second, and macOS drag-and-drop after Plans 4–5.
- Classick never creates a `Videos` playlist. Before publishing an iTunesDB it
  preserves at most one exact registered firmware-system instance and removes
  only older exact duplicates.
- A playlist is never classified by display name or emptiness. Unknown,
  foreign, and near-match playlists remain untouched.
- The device is authoritative for Classick-managed playlist identity; host
  state is a cache.
- Every subscribed playlist has one ordered logical membership and, when
  Rockbox compatibility is enabled, both Apple-firmware and Rockbox
  representations.
- Rockbox playlist publication is required finalization work when enabled, not
  a warning-only mirror.
- Library drag-and-drop copies intent from the Library; it never removes or
  moves source content.
- Device drops persist an additive selection mutation. General setting
  `drop_sync_behavior` is either `immediate` or `next_sync`, defaulting to
  `immediate`.
- An immediate device drop starts a sync only after persistence
  acknowledgement and only when the explicit device is connected, idle, and
  missing at least one matched track.
- A busy, paused, finalizing, or disconnected device records the addition for
  its next sync and does not queue a hidden second session.
- Manual playlists accept drops. Smart, corrupt, missing, or otherwise
  non-editable playlists reject them visibly.
- macOS 15 remains the deployment floor. Newer visual material remains
  availability-gated and is not required for drag/drop correctness.

## 3. Delivery sequence and dependencies

### Plan 6A — iPod playlist integrity

Depends on Plan 3's coordinated DB/artwork/manifest checkpoint. It fixes track
unlinking, validates managed targets, makes ownership publication recoverable,
adds playlist auditing, and normalizes exact firmware-system duplicates.

### Plan 6B — Rockbox playlist projection

Depends on 6A's trusted logical membership and device-authoritative ownership,
plus Plan 3's finalization journal. It publishes and removes only recorded
Classick `.m3u8` files.

### Plan 6C — Native macOS library drag-and-drop

Depends on Plan 1's serial-keyed device registry and snapshots, Plan 4's
ordered/durable transport, and Plan 5's persisted request acknowledgements and
acknowledged editor drafts. It must not reintroduce FIFO reply correlation or
client-side read/modify/write mutations.

## 4. Plan 6A: playlist integrity

### 4.1 Structural inventory

A read-only playlist inspector reports, for each normal iTunesDB playlist:

- ID, name, timestamp, member count, and sort order;
- master, podcast, and smart flags;
- smart preferences and every rule field/action/value;
- whether the ID is in the Classick managed record;
- structural classification and the reason for that classification.

Internal MHSD5 category records are reported separately when the vendored
libgpod interface makes them observable. They are intentionally memberless and
must not be mistaken for empty user playlists.

The inspector is available as a read-only diagnostic command and supplies
fixture serialization for regression tests. It performs no DB or device write.

### 4.2 Classification

Every playlist falls into exactly one class:

1. `Managed`: the connected device's authoritative record names its exact ID,
   and the resolved playlist is normal, non-master, non-podcast, and non-smart.
2. `FirmwareSystem(profile)`: the complete semantic payload matches a
   versioned, fixture-backed system profile.
3. `Foreign`: every other playlist, regardless of name or emptiness.

A stale/corrupt managed ID that resolves to a master, podcast, smart, or other
unexpected playlist does not grant ownership. Classick preserves the suspect
playlist, drops that invalid association from the candidate managed record,
and creates a fresh normal playlist if the logical playlist remains desired.

### 4.3 Physical Videos profile

The mounted iPod's six records define profile
`ipod-classic-video-kind-v1`. The name is diagnostic only and is not matched.
Its semantic fingerprint is:

- smart, non-master, non-podcast, zero members;
- preferences: `liveupdate=1`, `checkrules=1`, `checklimits=0`,
  `limittype=3`, `limitsort=2`, `limitvalue=25`,
  `matchcheckedonly=0`;
- rules header: `unk004=65537`, `match_operator=0`;
- exactly two rules, both `ITDB_SPLFIELD_VIDEO_KIND (0x3c)`:
  - `ITDB_SPLACTION_BINARY_AND (0x400)`, value `0xc42`, units `1`;
  - `ITDB_SPLACTION_NOT_BINARY_AND (0x02000400)`, value `0x20a004`, units
    `1`;
- strings are null and dates/remaining unknown fields are zero.

Before every Classick iTunesDB publication, exact instances are grouped by
profile. Zero or one is preserved unchanged. With multiple instances, the
newest timestamp is preserved; an ID tie-break makes the choice deterministic.
Only older exact instances are removed. Unknown profiles and near matches are
audit-only.

This is normalization of an Apple-firmware system invariant, not adoption of a
foreign user playlist. Classick never creates the canonical record. If the
final physical causality gate disproves the registered profile or reveals a
second legitimate category distinction, release is blocked until the profile
is corrected.

### 4.4 Safe track removal

The vendored libgpod implementation defines
`itdb_playlist_remove_track(NULL, track)` as removal from the master playlist,
not every playlist. `itdb_track_remove` does not unlink playlist members.

Before freeing a track, Classick snapshots every normal/smart playlist pointer,
removes the track from each membership that contains it, and only then calls
`itdb_track_remove`. The same helper is used by ordinary delete, replace/wipe,
and recovery paths. A write/reparse verification proves no dangling member
remains in foreign or managed playlists.

### 4.5 Coherent managed ownership

`/iPod_Control/classick/managed_playlists.json` is authoritative while the
device is connected. Each logical slug records:

- Apple iTunesDB playlist ID;
- expected structural kind (`normal`);
- Rockbox relative filename and content hash when enabled;
- record schema version and device serial.

The host per-device file remains a cache for disconnected display and recovery.
Plan 3's checkpoint journal carries the candidate ownership record. Publication
order is:

1. stage playlist mutations and candidate ownership in the pending journal;
2. write and reopen/verify iTunesDB;
3. verify every recorded managed ID resolves to the expected normal playlist;
4. atomically publish device ownership;
5. publish Rockbox projections when enabled;
6. refresh host cache best-effort;
7. remove the completed journal.

If step 4 or 5 fails, finalization remains incomplete and the journal survives.
Recovery verifies the already-published DB and finishes ownership/projection;
it does not recreate playlists and produce new IDs. A host-cache failure never
changes device truth.

## 5. Plan 6B: Rockbox projection

### 5.1 Representation

Rockbox output lives under `/Playlists/Classick/`. Each logical playlist maps
to one readable, collision-safe filename consisting of a FAT-safe display-name
stem plus a stable short hash of the slug. The exact relative filename is
recorded in managed ownership; filenames are never rediscovered by scanning or
display-name matching.

Files are UTF-8 `.m3u8` without a BOM. Each non-comment line is an absolute,
slash-separated device path such as
`/iPod_Control/Music/F00/ABC123.m4a`. Paths come from the verified final
device-track record, never from a host source path. CR/LF in a track path is
rejected before staging.

The `.m3u8` order exactly matches the logical manual/smart playlist order used
for the Apple representation. An empty logical playlist produces a valid empty
managed file.

### 5.2 Publication and removal

Projection writes a temporary sibling, fsyncs it, then atomically renames it.
For a rename, the new file becomes durable before the recorded old file is
removed. A failed old-file removal retains the old ownership entry for retry.

Classick may write or remove only exact paths recorded in device ownership.
Every stored path must be a single filename directly below
`/Playlists/Classick`; absolute paths, separators, `.`/`..`, non-`.m3u8`
extensions, and symlink escapes are rejected. Existing unrecorded files are
foreign, including files inside the managed directory, and are never
overwritten or removed. A collision selects a new hashed filename rather than
claiming the existing file.

Unsubscribing a playlist removes its Apple representation and its exact
recorded Rockbox file in the same recoverable finalization. Disabling Rockbox
compatibility removes recorded projections only after the Apple/ownership
checkpoint is safe. An unplug or filesystem error retains the journal and
reports incomplete finalization.

Rockbox's tag database is independent. Static playlists are usable through the
file/playlist catalogue without a database rebuild; Classick does not rewrite
Rockbox's database or configuration.

## 6. Plan 6C: native macOS drag-and-drop

### 6.1 Platform interaction model

Implementation uses SwiftUI `Transferable`, `.draggable(_:preview:)`, and
`.dropDestination(for:action:isTargeted:)`, all available at the macOS 15
floor. It does not use custom mouse tracking, global event monitors, floating
drop windows, or hand-built drag sessions.

Dragging is a copy operation: the Library remains unchanged. The system drag
preview is a compact native label using existing album/device imagery and the
artist, album, or genre name. It is not a screenshot of the full row.

Existing rows/cards become destinations:

- configured device parent row and Music child;
- persistent device card only when it resolves one explicit serial;
- editable manual-playlist row.

Settings children, smart playlists, corrupt/missing playlists, unconfigured
devices, and ambiguous aggregate cards do not register as accepting
destinations. The system therefore supplies its normal unavailable cursor and
snap-back behavior.

`isTargeted` drives a transient system-accent selection treatment on the
existing row/card, visible only while an acceptable item is directly over that
target. There is no permanent dashed box, oversized overlay, layout shift, or
simultaneous highlighting of multiple targets. Native `List` scrolling remains
available during the drag. Hovering does not navigate or change sidebar
selection.

The existing Choose Music controls and Add Songs picker remain non-drag
alternatives. VoiceOver receives a concise destination label such as “Add
Birdy to Michael's iPod” or “Add Birdy to Favorites.” Keyboard focus and
selection do not change merely because a pointer passes over a target.

### 6.2 Transfer payload

`LibraryDragPayload` is a versioned `Codable`, `Transferable`, `Sendable` value
under a Classick-specific exported UTType. It contains:

- schema version;
- current app-launch nonce;
- one or more normalized `SelectionRule` values;
- a short display summary used only for feedback.

It contains no host paths, device paths, playlist contents, or serial. The
receiver rejects a wrong nonce, unsupported version, empty/excessive rule set,
or malformed rule. The daemon resolves rules only against its authoritative
cached library index. The array representation future-proofs native multi-item
drag without requiring inconsistent selection UI in the first release; v1
drags one visible aggregate row at a time.

### 6.3 Atomic daemon mutations

Device drop command:

`add_selection_to_device { request_id, serial, rules }`

The daemon validates the configured serial and applies one additive mutation:

- `Include`: case-insensitive union and canonicalization;
- `Exclude`: remove exclusions covering the dropped set, expanding a broad
  artist/genre exclusion into explicit unaffected album exclusions;
- `All`: selection is unchanged.

The result reports matched tracks, currently missing tracks, whether selection
changed, canonical revision, and acknowledged request ID. A replay is
idempotent and still acknowledges the current revision.

Manual playlist command:

`append_selection_to_playlist { request_id, slug, rules }`

The daemon resolves rules, retains existing order, naturally orders the added
batch, and deduplicates against both existing and newly resolved paths in one
atomic store mutation. It rejects smart/corrupt/missing targets with a
correlated failure and no revision change. It never composes a client-side
`resolve_tracks` plus whole-playlist save.

Plan 4 adds durable intent keys per target. Same-target unsent drops coalesce;
cross-target order is preserved. Written intents remain in flight until the
matching authoritative request ID/revision acknowledgement. Reconnect resends
the latest unacknowledged idempotent mutation.

### 6.4 Sync-after-drop policy

Global config field `drop_sync_behavior` is additive on the existing config
wire and defaults to `immediate`:

- `immediate`: after the additive mutation is acknowledged, start the existing
  serial-targeted sync if the device is connected, idle, and at least one
  matched track is absent;
- `next_sync`: persist and report “Added for next sync” without starting;
- either value while disconnected/busy/paused/finalizing: persist and report
  “Added for next sync”; no hidden follow-up session is queued;
- no missing tracks: report “Already on this iPod” and do not start a no-op
  sync.

Changing the setting is an acknowledged global-config edit under Plan 5.

### 6.5 Feedback and stale-editor safety

Before drop, only valid destinations highlight. After drop, the destination
shows lightweight “Adding…” activity without claiming success. Authoritative
responses produce exactly one accessible outcome: added and syncing, added for
next sync, already present, appended track count, or a correlated error.

An open Device Music or Playlist editor reconciles the newer canonical
revision through `AcknowledgedDraft`; it cannot overwrite the drop with a
stale whole-value save. A later local edit remains dirty when an earlier drop
acknowledgement arrives.

## 7. Error handling and invariants

- Classick never creates a firmware-system Videos playlist.
- No playlist is deleted because of its name or emptiness.
- Unknown system signatures are audit-only.
- Foreign playlist membership is preserved except when a deleted track must be
  safely unlinked before that track is freed.
- An ownership-record failure cannot be followed by publication of an
  ownerless Classick playlist.
- Apple and Rockbox representations derive from the same verified ordered
  membership.
- Rockbox writes and deletes stay within the recorded managed directory.
- A drop never targets an implicit device or mutates a smart playlist.
- A socket write is not user-visible success; persistence acknowledgement is.
- Source shares remain read-only during resolution, drag/drop, and sync.

## 8. Verification

Automated coverage includes:

- playlist audit serialization and structural classification;
- exact Videos-profile zero/one/many normalization, newest preservation,
  deterministic tie-break, localization/name independence, and near-match
  preservation;
- master, podcast, On-The-Go, arbitrary empty smart, and foreign normal
  preservation;
- stale managed IDs pointing at smart/podcast/master records;
- delete and wipe with foreign normal/smart memberships followed by successful
  write/reparse;
- failure injection across DB, device ownership, Rockbox rename/delete, and
  host-cache publication;
- `.m3u8` encoding, path conversion, order, empty file, collision, rename,
  unsubscribe, toggle-off, symlink/path traversal, retry, and foreign-file
  preservation;
- Apple/Rockbox membership equivalence;
- exact drag payload encoding, nonce/version/rule validation, and native target
  acceptance matrix;
- device A/B isolation and unconfigured/ambiguous target rejection;
- Include/Exclude/All algebra, missing/already-present decisions, immediate and
  next-sync policies, and disconnected/busy/finalizing deferral;
- concurrent/replayed drops, same-target coalescing, cross-target chronology,
  persistence-before-ack, reconnect resend, and exact acknowledgement removal;
- manual playlist ordering/deduplication and smart/missing/corrupt rejection;
- open-editor revision reconciliation and accessibility feedback;
- full Rust tests one-threaded where real IPC is involved, Swift tests,
  XcodeGen regeneration, macOS 15-targeted Xcode build, and app bundle.

Native visual verification on macOS 27 covers light/dark appearances, sidebar
scrolling during drag, system copy/unavailable cursors, one-target accent
highlight, drag preview, snap-back on rejection, VoiceOver labels, and no
layout shift. A macOS 15 runtime verifies the same interaction without newer
material effects.

The final physical verification sequence is:

1. copy/hash the current iTunesDB and record the six Videos instances;
2. run one coordinated Classick write and inspect without booting firmware;
3. boot Apple firmware once, remount, and inspect before running Classick;
4. boot Apple firmware a second time without a Classick write, remount, and
   inspect again;
5. run Classick normalization and prove at most one exact canonical record;
6. sync a manual and smart Classick playlist, verify Apple-firmware order and
   playback;
7. boot Rockbox, load both `.m3u8` files, and verify order and playback;
8. exercise rename/unsubscribe and prove foreign Apple/Rockbox playlists remain
   untouched;
9. eject through Classick after each mutation sequence.

The source share is read-only throughout. Device writes occur only through the
coordinated transaction and are preceded by backups captured in the live gate.

## 9. Research authorities

- Apple Human Interface Guidelines, Drag and drop:
  <https://developer.apple.com/design/human-interface-guidelines/drag-and-drop>
- Apple SwiftUI sample, Adopting drag and drop using SwiftUI:
  <https://developer.apple.com/documentation/swiftui/adopting-drag-and-drop-using-swiftui>
- Apple Core Transferable documentation:
  <https://developer.apple.com/documentation/coretransferable>
- Current Rockbox iPod 6G playlist manual:
  <https://download.rockbox.org/daily/manual/rockbox-ipod6g/rockbox-buildch4.html#x11-700004.4.1>
- Rockbox playlist parser/saver source:
  <https://github.com/Rockbox/rockbox/blob/f58038b687a6bca266334f279a8611f8e1605c67/apps/playlist.c>
- libgpod playlist and smart-rule implementation:
  <https://github.com/strawberrymusicplayer/strawberry-libgpod/blob/98a0c3a108b0777919eee549155826ccc35b4a62/src/itdb_playlist.c>
- libgpod iTunesDB parser/writer and MHSD5 separation:
  <https://github.com/strawberrymusicplayer/strawberry-libgpod/blob/98a0c3a108b0777919eee549155826ccc35b4a62/src/itdb_itunesdb.c>
