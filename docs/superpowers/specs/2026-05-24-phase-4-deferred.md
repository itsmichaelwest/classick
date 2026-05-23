# Phase 4 — Playlists (DEFERRED)

**Status:** deferred 2026-05-24. User has no m3u playlists in their source library and no immediate playlist source. The roadmap entry stays open; this doc captures the design intent so a future resume isn't starting from scratch.

## Why deferred

User's FLAC library is managed by Lidarr. Lidarr doesn't generate playlists. User suggested revisiting "when we can integrate with iTunes playlists" — most likely meaning: read playlists from a local iTunes/Music library file, or from beets's playlist plugin output, or from some yet-to-be-identified source. Without a concrete source, the m3u parser would be built against a hypothetical use case.

## Design intent (captured from a 2026-05-24 scoping pass)

When this phase resumes, these decisions are pre-made:

- **Source location:** TBD — depends on what kind of playlist source the user eventually adopts (m3u in source tree, separate playlist dir, iTunes XML library, beets export, etc.). The design should keep the playlist-source path independent of the music-source path (separate `playlist_source` config field).

- **Manifest schema:** separate top-level `playlists: Vec<PlaylistEntry>` section alongside `tracks`. Each entry: `{ source_path, ipod_dbid, last_modified, track_dbids }`. Diff has its own playlist diff emitting `PlaylistAdd | PlaylistModify | PlaylistRemove` actions. Reasoning: tracks and playlists are conceptually different (playlists are ordered references to tracks; mismatching dbids between iPod and manifest is fatal); conflating them under a single `Action` enum was rejected.

- **Missing-track handling:** skip silently, log a warning. Most forgiving — handles Lidarr's transient state where a track might be temporarily missing during a metadata refresh. Strict-error and TUI-prompt variants both add ceremony without proportional value.

- **Track order:** preserve m3u (or equivalent source) order. iPod libgpod supports ordered playlists. Re-syncing reorders if the source playlist changed.

## When to resume

Triggers worth picking this back up:
- User adopts a tool that generates playlists (beets's `playlist` plugin, manual m3u curation, etc.).
- iTunes/Music app integration becomes attractive (reading the Music Library `.xml`).
- iPod-side playlist preservation becomes a need (user creates an "On-The-Go" playlist on the iPod and wants it preserved across syncs — this is a different feature: playlist *read-back* rather than *write*).

## Scope notes for the eventual resume

- The CLI surface should be additive: new `--playlist-source <PATH>` flag, no breaking changes to existing flags.
- The `Manifest` schema bump can stay at `version: 1` if `playlists` is `#[serde(default)]` — Phase 3 manifests would deserialize cleanly with `playlists: vec![]`.
- libgpod's playlist API (`itdb_playlist_new`, `itdb_playlist_add_track`, `itdb_playlist_add`, `itdb_playlist_remove`) is the integration surface; sketch the FFI usage during the resume's plan-writing step.
- The iPod Classic UI shows playlists under Music → Playlists; the master playlist (`itdb_playlist_mpl`) is the "all songs" list and is separate from user playlists.
