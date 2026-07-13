# SysInfoExtended Provisioning (embedded, per-model) — Design

**Status:** approved design, ready for implementation plan
**Date:** 2026-07-13
**Scope:** SP1 of the artwork fix. SP2 (live SCSI read via macOS IOKit) is
explicitly deferred — see "Out of scope".

## Problem & confirmed root cause

Album art never displays on the user's iPod Classic when synced by classick,
even though extraction, thumbnailing, and the sync are all correct. Confirmed
on-device this session:

- The source FLACs have embedded art; classick extracts it correctly and writes
  valid RGB565 thumbnails to the `.ithmb` blobs with `has_artwork=1`.
- The iPod firmware ignores those thumbnails because they are written in the
  wrong **format set**. On macOS, classick never provides `SysInfoExtended`, so
  libgpod falls back to a built-in per-model artwork-format guess that **omits
  `F1069`** — a cover-art correlation ID the Late-2009 iPod Classic (MC293)
  firmware actually reads.
- **Validated fix:** placing the device's real `SysInfoExtended` (with its
  `ImageSpecifications`) at `iPod_Control/Device/SysInfoExtended` *before*
  libgpod opens the DB makes libgpod emit the correct ithmb set (now including
  `F1069`), and the art displays. Proven by hand-placing a community dump +
  re-syncing COWBOY CARTER → art appeared on-device.

The artwork `ImageSpecifications` are **model-generic** (every unit of a given
model has an identical screen/format spec). The iPod hardware universe is
**frozen** (discontinued). Therefore a *comprehensive embedded per-model* set is
permanently durable and covers every syncable iPod, on all platforms — reading
the real per-device XML over SCSI buys nothing for artwork.

## Goal

Before libgpod opens an iPod DB for a sync, ensure
`iPod_Control/Device/SysInfoExtended` contains the correct per-model
`ImageSpecifications` (with the device's real FireWireGUID), so libgpod
generates the correct ithmb format set and cover art displays. Cross-platform
(fixes macOS, Windows, and Linux — removes the dependency on libgpod's built-in
guess entirely, making artwork correct and *deterministic* everywhere).

## Empirical safety validation (done this session)

- **Music.app is unaffected.** With the file present, macOS Music.app read the
  iPod perfectly (recognized it, listed tracks, no "Restore" dialog). With the
  file removed, Music.app read it *identically*. Conclusion: the file is **safe
  and inert** to Music.app — its presence changes nothing. (This also means the
  documented "iTunes rejects libgpod iPods" lore does not apply to modern
  Music.app for this device.)
- **iPod firmware unaffected.** The firmware read the DB and displayed art with
  the file present — the DB remained valid/signed.

These validations justify a **persistent** (write-and-leave) design rather than
a transient write-then-delete. No cleanup logic is needed.

**Known gap (accepted):** this safety validation is **macOS-only**. iTunes on
Windows with a persistent `SysInfoExtended` present is **untested** (no Windows
hardware available this session). Decision: keep persistent anyway — macOS app
stability is the current priority — and re-test the Windows/iTunes case later.
If Windows iTunes turns out to object, the fallback is trivial: switch to the
transient variant (write before open → delete at sync end), since libgpod only
needs the file at DB-open time. No architectural change required to pivot.

## Architecture

A single provisioning step, invoked at sync start immediately **before**
`OwnedDb::open`:

```
sync start
  → resolve_libgpod_identity(mount) -> { firewire_guid, model_num_str }   [existing]
  → select embedded SysInfoExtended template for model_num_str            [new]
  → inject firewire_guid into the template                                [new]
  → write iPod_Control/Device/SysInfoExtended (overwrite, idempotent)     [new]
  → OwnedDb::open  (libgpod reads SysInfoExtended into device state)      [existing]
  → sync / itdb_write  (correct ithmb generated from ImageSpecifications) [existing]
```

Pure Rust; no platform-specific code. Placed in a new focused module so it can
be understood and tested in isolation.

## Components

### 1. Embedded per-model data
Vendor the authoritative per-model `SysInfoExtended` plists from
`github.com/dstaley/ipod-sysinfo` (**CC0-1.0**, public domain — free to
redistribute) into the repo (e.g. `crates/classick/data/sysinfo-extended/`),
pulled in with `include_bytes!`.

**Model set to embed** — the iPods classick can actually sync (HASH58-signable)
that have cover-art displays. Candidate set, keyed by generation:
- iPod Photo / iPod 4G (color)
- iPod Video / 5G
- iPod Classic 6G, 6.5G (120GB), Late-2009 (160GB / MC293) — **primary target**
- iPod nano 1G–4G

nano 5G+ (hash72/hashAB) cannot be synced by classick at all → excluded until
that signing work exists. Shuffle/mini have no artwork → excluded.

**Decision (best-effort bonus):** embed the full believed-HASH58 + artwork set
above. The **iPod Classic family is the only hardware-verified deliverable**
(the user's device); the other generations' templates ship **unverified on real
hardware** as best-effort bonus coverage — they use the same proven mechanism
and correct-per-generation data, but no one has confirmed art renders on a real
Photo/Video/nano. Acceptable because provisioning is non-fatal (a wrong-or-
absent template only degrades to "art may not display", never breaks a sync).
Each model is a drop-in `(ModelNumStr → generation)` row + one plist, so
promoting one from "unverified" to "verified" later costs nothing.

### 2. Model → template resolution
Reuse `resolve_libgpod_identity()`'s `model_num_str` (e.g. `MC293`). Templates
are keyed by **generation** (multiple ModelNumStr variants of one generation
share one template, since artwork specs are per-screen/generation).

**Resolution mechanism (committed):** a static, embedded table
`ModelNumStr → generation`, transcribed from libgpod's authoritative
`ipod_info_table` (the same mapping libgpod itself uses to pick fallback
formats — so our selection can never disagree with libgpod's model
understanding), then `generation → embedded template`. The table is a plain
Rust `const`/slice — self-contained and unit-testable with no device or FFI
dependency. Covers the embedded model set (below); every entry is derived from
libgpod's table, not guessed.

- **Unknown/unsupported ModelNumStr → `None`.** Provisioning is skipped; libgpod
  falls back to its built-in guess (status quo — art may not display, but
  nothing breaks). Log a `warn!` naming the ModelNumStr so we can add the dump
  later. Never map a near-model as a substitute (a wrong template is worse than
  none — see Safety constraint 2).

### 3. GUID injection
Substitute the device's real `firewire_guid` (from `resolve_libgpod_identity`,
normalized to the plist's bare-uppercase-hex form, e.g. `0x000A27002138B0A8` →
`000A27002138B0A8`) into the template's `<key>FireWireGUID</key><string>…`
value, so libgpod's device state is internally consistent. The
`ImageSpecifications` (the part that matters for artwork) pass through untouched.
Other cosmetic fields (SerialNumber, etc.) are left as-is from the template —
they do not affect artwork or signing.

### 4. Write step
Write the injected XML to `iPod_Control/Device/SysInfoExtended`, overwriting any
existing file (our model-generic version is authoritative in the no-SCSI world).
Persistent — left on the device (validated safe + inert). Idempotent: safe to
run every sync; refreshes the GUID/model each time.

## Data flow & interfaces

New module `crates/classick/src/ipod/sysinfo_provision.rs`:

```rust
/// Select the embedded SysInfoExtended template for a resolved ModelNumStr.
/// Returns None for models we don't ship a template for (caller skips + warns).
pub fn template_for_model(model_num_str: &str) -> Option<&'static [u8]>;

/// Inject the device's FireWireGUID into a template's <FireWireGUID> value.
pub fn inject_guid(template: &[u8], firewire_guid: &str) -> Result<Vec<u8>>;

/// Full provisioning step: resolve template for `identity`, inject GUID, write
/// to `<mount>/iPod_Control/Device/SysInfoExtended`. No-op (Ok) + warn when no
/// template matches. Called by the sync flow immediately before OwnedDb::open.
pub fn provision(mount: &Path, identity: &LibgpodIdentity) -> Result<()>;
```

Call site: the sync entry path that currently calls `OwnedDb::open` (apply
loop / orchestrator), just before the open. `provision` runs after identity is
resolved (identity is already resolved there today for `set_model_num`).

## Error handling

Artwork is non-fatal, matching the existing "add track without art rather than
fail the sync" philosophy (`ipod/db.rs`):
- Unknown model → skip provisioning, `warn!`, continue.
- Template parse/injection error → `warn!`, skip provisioning, continue (don't
  abort the sync over artwork).
- Write failure (read-only FS, etc.) → `warn!` with context, continue.

Never fail a sync because provisioning failed — worst case is the pre-existing
"art may not display" state.

## Safety constraints (must hold; tested)

1. **DB signature/checksum unchanged.** libgpod uses `SysInfoExtended` for
   generation/checksum dispatch; a Classic must still resolve to HASH58 or the
   iPod firmware would reject the DB. Validated on-device (firmware read the DB +
   showed art); the plan adds a guard/test that the resolved checksum type / DB
   validity is unchanged after provisioning.
2. **Correct template per model.** Writing the wrong model's `ImageSpecifications`
   could produce undisplayable art. Resolution must map to the matching
   generation; a mismatch is worse than no file. Unknown → skip, never guess a
   near-model.

## Testing

- **Unit:** `template_for_model` returns the right template for known
  ModelNumStrs (incl. MC293) and `None` for unknown.
- **Unit:** `inject_guid` replaces the FireWireGUID and leaves
  `ImageSpecifications` intact (assert the injected GUID present, a known
  ImageSpecifications FormatId still present).
- **Unit:** every embedded template parses and contains a non-empty
  `ImageSpecifications` (guards against a corrupt/empty vendored file).
- **Unit:** `provision` writes the file to the expected path (temp-dir fake
  mount) and is idempotent; unknown model → no file written, Ok returned.
- **On-device smoke (before merge):** wipe → sync an album via the normal flow
  (provisioning active) → `art-audit` shows `has_artwork=1` AND the album shows
  art on the iPod screen. (The `art-audit` example built this session stays in
  the tree for this.)

## Out of scope (deferred)

- **SP2 — live SCSI `SysInfoExtended` read (macOS IOKit).** macOS exposes no
  user-space SCSI path for type-`$00` mass-storage devices; it requires a
  DriverKit system extension (major effort, Apple entitlement, notarization) and
  adds nothing for artwork. Deferred as a separate "per-device forensics + Nano
  5G+ hash support" feature. When built, it becomes an authoritative *source*
  feeding this same write path, with the embedded set as fallback.
- **Nano 5G+ (hash72/hashAB) sync support.** Separate signing work.

## Notes / cleanups

- `docs/SCSI.md` previously recorded a decision to *not* write `SysInfoExtended`
  to disk (a Windows/iTunes-coexistence worry). This design reverses that for
  artwork; the reversal is safe (validated: firmware + Music.app both fine) and
  should be noted in `docs/SCSI.md`.
- Temporary `art-debug` tracing added to `apply_loop.rs` + `ipod/db.rs` this
  session must be reverted before/with this work. The `art-audit` example is
  kept; `art-check` example may be kept or removed.
