# Device and data safety

This document collects the invariants that protect user data. Changes touching
the iTunesDB, artwork, manifests, playlists, recovery, or device discovery must
preserve them and add a regression test where practical.

## Source library is read-only

Classick may enumerate, stat, hash, decode, transcode from, and read tags or
artwork from the configured source. It must never rename, delete, rewrite, or
create files in that source tree. Temporary and derived output belongs in
Classick state/staging directories or on the target device.

## iTunes compatibility warning

Apple iTunes rejects libgpod-managed databases because their signature does not
match Apple's stricter check, even though iPod firmware accepts them. Users must
close iTunes and must not follow its Restore prompt. The Windows preflight guard
and UI warning copy are safety controls, not optional messaging.

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
