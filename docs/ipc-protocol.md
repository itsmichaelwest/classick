# ipod-sync IPC protocol v1.3.0

> This title's version (`1.3.0`) is the **subprocess** protocol (stdin/stdout,
> `--ipc-mode`) described below. The separate **daemon** protocol (named-pipe/
> Unix-socket, UI ↔ daemon) is currently at **`2.0.0`** — see "Daemon v2.0.0"
> further down this document for its commands/events.

Newline-delimited JSON over stdin/stdout, UTF-8, custom typed-envelope.
Every message is a single-line JSON object with a `type` discriminator
field; field names are `snake_case`. Each line is exactly one message.
**stdout** carries events from the Rust core to a UI frontend; **stdin**
carries commands from the UI back to the core. The core is spawned with
`ipod-sync.exe --ipc-mode`; the UI owns the child process exclusively.

This document is the source of truth for the wire format. Both the Rust
`IpcBackend` (in `src/progress.rs`) and the C# `CoreProcess` (in
`ui-windows/IpodSync.UI/Services/CoreProcess.cs`) implement it. For the
broader architectural context — why IPC instead of FFI, why
custom-envelope instead of JSON-RPC 2.0, why WinUI 3 — see
`docs/superpowers/specs/2026-05-24-phase-6-winui-app.md`.

---

## 1. Versioning

Protocol version follows **semver**. The current version is **`1.3.0`**.

The core **MUST** emit a `hello` event (see §4.1) as its first line of
stdout, carrying the protocol version it speaks. The UI **MUST** read
this event before sending any command and verify compatibility:

- **Major bump (e.g. `1.x.x` → `2.0.0`)** — breaking change. UI **MUST**
  refuse to proceed: show a clean error dialog ("UI is too old for this
  core" or vice versa), do not send commands, terminate the child.
- **Minor bump (e.g. `1.0.x` → `1.1.0`)** — additive. New optional
  fields, new event types. Older UIs ignore unknown events and unknown
  fields. Newer UIs treat absence of new optional fields as the
  documented default.
- **Patch bump (e.g. `1.0.0` → `1.0.1`)** — doc-only / spec wording
  clarifications. No code changes required on either side.

The UI **MUST NOT** send any command until it has read and validated the
`hello`. If the core has not emitted `hello` within a sensible deadline
(suggested: 5 s after spawn), the UI **SHOULD** treat the core as failed
to start and tear down the child.

The `hello` event also carries `core_version` (the Cargo package
version) for display in the UI's About dialog. It is **informational
only** — version negotiation uses `protocol_version` exclusively.

---

## 2. Message envelope

Both directions use the same envelope shape:

```json
{"type": "<discriminator>", ...message-specific fields}
```

Rules:

- The `type` field is required on **every** message and is the
  discriminator. Its value is `snake_case` (e.g. `track_start`,
  `review_decision`).
- All other field names are `snake_case`.
- A message occupies **exactly one line**, terminated by `\n` (LF).
  Newlines inside string values are JSON-escaped as `\n` and **MUST NOT**
  appear as literal bytes on the wire.
- Pretty-printed JSON (with embedded newlines or tabs) is forbidden on
  the wire.

### Unknown messages

- **UI receiving an unknown `type`:** log the line to its debug log and
  silently ignore it. This preserves forward-compat with newer cores
  that may emit additional event types within the same major version.
- **Core receiving an unknown `type`:** log a warning to its tracing log
  (`tracing::warn!`) and discard the command. A malformed command is
  not a fatal condition; the run continues. Persistent malformed input
  may indicate a corrupted UI; the core does not abort on its own — the
  UI is expected to be well-behaved or send `cancel`.
- **Core receiving an unparseable line (malformed JSON):** logged and
  discarded, same policy.
- **UI receiving an unparseable line from stdout:** this indicates a
  serious bug (the core must always emit valid JSON). UI logs and
  continues reading; do not abort.

---

## 3. Stream semantics

- **Framing:** one JSON object per `\n`-terminated line. The reader
  splits on `\n`, trims trailing `\r`, parses the result as a JSON
  value. Empty lines are skipped silently.
- **No trailing comma**, no JSON-arrays-of-messages, no batching.
- **No nested literal newlines** in string fields — they MUST be JSON
  escapes (`\n`, `\t`, `\r`).
- **Flushing:** the core **MUST** `flush()` stdout after every event
  write. Rust's stdout is block-buffered when attached to a pipe (not
  line-buffered), and an unflushed write would leave the UI waiting
  indefinitely. The UI must NOT assume any line-buffering by the OS.
- **Encoding:** UTF-8. The core writes UTF-8; the UI reads UTF-8. No
  BOM. Non-ASCII characters (track titles in CJK, accented filenames,
  etc.) are JSON-escaped per RFC 8259 at the serializer's discretion;
  both sides accept either escaped (`é`) or raw UTF-8 (`é`).
- **Order:** events are delivered in the order the core emits them.
  Commands are processed in the order the core reads them from stdin.
  There is **no interleaving constraint between directions** — the core
  may emit many events while waiting for a command (e.g. background
  `log` lines during a prompt), and the UI may send a `cancel` at any
  time.

---

## 4. Events (core → UI)

| `type`        | Fields                                                                                      | Purpose                                  |
|---------------|---------------------------------------------------------------------------------------------|------------------------------------------|
| `hello`       | `protocol_version`, `core_version`                                                           | Handshake; first message after spawn     |
| `header`      | `source`, `ipod`, `manifest`                                                                 | Resolved paths for display               |
| `summary`     | `add`, `modify`, `metadata_only`, `remove`, `unchanged`, `total_planned`                     | Action plan counts                        |
| `review`      | `summary` (object), `no_delete`                                                              | Request a review decision                 |
| `prompt`      | `id`, `message`, `options`                                                                   | Modal multi-choice prompt                 |
| `form`        | `id`, `label`, `initial`, `hint`                                                             | Modal text-input prompt                   |
| `track_start` | `current`, `total`, `label`, `eta_secs?`                                                     | Begin a per-track operation               |
| `track_done`  | (none)                                                                                       | Increment completed count                 |
| `log`         | `message`                                                                                    | Informational log line                    |
| `error`       | `message`, `recovery_hints?`                                                                 | Non-fatal or fatal error                  |
| `finish`      | `success`, `skipped_for_space?`, `artwork?`, `db_restored?`                                  | Run complete; core will close stdout      |
| `paused`      | (none)                                                                                       | Run gracefully paused; core will close stdout (new in **1.1.0**) |

The Rust-side enum the events derive from is `ProgressEvent` in
`src/progress.rs`. `hello` is new in IPC mode (not part of
`ProgressEvent`); the rest map 1:1.

### 4.1 `hello`

Emitted **once**, **first**, immediately after the core starts in
`--ipc-mode`. The UI reads it, validates `protocol_version`, and only
then proceeds with the rest of the protocol.

| Field              | Type     | Notes                                                       |
|--------------------|----------|-------------------------------------------------------------|
| `protocol_version` | `string` | Semver of the wire protocol the core speaks. Currently `1.3.0`. |
| `core_version`     | `string` | `CARGO_PKG_VERSION` of the core binary. Informational.       |

```json
{"type":"hello","protocol_version":"1.0.0","core_version":"0.1.0"}
```

### 4.2 `header`

Emitted once near the start of a run, after the orchestrator resolves
paths from config and CLI flags. Mirrors `ProgressEvent::Header`.

| Field      | Type     | Notes                                                       |
|------------|----------|-------------------------------------------------------------|
| `source`   | `string` | Absolute path to the source library (e.g. `\\nas\music\flac`). |
| `ipod`     | `string` | Absolute path to the iPod mount point (e.g. `G:\`).         |
| `manifest` | `string` | Absolute path to the manifest JSON for this iPod's UUID.    |

```json
{"type":"header","source":"\\\\nas\\music\\flac","ipod":"G:\\","manifest":"C:\\Users\\me\\AppData\\Roaming\\ipod-sync\\manifests\\000a1b2c3d4e5f60.json"}
```

### 4.3 `summary`

Emitted once after the action plan is computed. Counts are non-negative
integers. Mirrors `ProgressEvent::Summary`.

| Field           | Type     | Notes                                                |
|-----------------|----------|------------------------------------------------------|
| `add`           | `number` | Tracks to add to the iPod.                            |
| `modify`        | `number` | Tracks whose audio data will be re-transcoded.        |
| `metadata_only` | `number` | Tracks whose tags/art will be rewritten in place.     |
| `remove`        | `number` | Tracks present on iPod but not in source.             |
| `unchanged`     | `number` | Tracks already in sync; no work needed.               |
| `total_planned` | `number` | Sum of `add + modify + metadata_only + remove`.       |

```json
{"type":"summary","add":12,"modify":3,"metadata_only":2,"remove":0,"unchanged":1260,"total_planned":15}
```

### 4.4 `review`

Emitted when the orchestrator wants the user to confirm the action plan
before any destructive operation. The UI **MUST** reply with exactly one
`review_decision` command (§5.2); no further events fire until then.
Mirrors `ProgressEvent::Review`.

| Field       | Type                  | Notes                                       |
|-------------|-----------------------|---------------------------------------------|
| `summary`   | `ActionPlanSummary`   | Nested object; see schema below.            |
| `no_delete` | `boolean`             | Current value of the `--no-delete` toggle.  |

`ActionPlanSummary`:

| Field           | Type     | Notes                                            |
|-----------------|----------|--------------------------------------------------|
| `add`           | `number` |                                                  |
| `modify`        | `number` |                                                  |
| `metadata_only` | `number` |                                                  |
| `remove`        | `number` |                                                  |
| `unchanged`     | `number` |                                                  |

```json
{"type":"review","summary":{"add":12,"modify":3,"metadata_only":2,"remove":0,"unchanged":1260},"no_delete":false}
```

### 4.5 `prompt`

Modal multi-choice prompt. Used by `try_with_prompt` and ad-hoc error
recovery dialogs (e.g. "ffmpeg failed — retry / skip / abort?"). The UI
**MUST** reply with `prompt_decision` echoing the same `id`. Multiple
prompts with different `id`s may be in flight simultaneously; replies
match by `id`. Mirrors `ProgressEvent::Prompt` (and `PromptRequest`).

| Field     | Type       | Notes                                                                       |
|-----------|------------|-----------------------------------------------------------------------------|
| `id`      | `number`   | Unsigned 64-bit; allocated by `Progress::next_prompt_id()`. Correlation key. |
| `message` | `string`   | Free-text question shown to the user.                                       |
| `options` | `string[]` | One label per choice. UI renders as a button per entry.                     |

```json
{"type":"prompt","id":7,"message":"ffmpeg failed for track 'Boards of Canada - Rocket'. Choose:","options":["Retry","Skip this track","Abort"]}
```

Correlation: the UI's `prompt_decision` echoes the same `id` and a
zero-based `choice` index into `options` (see §5.3).

### 4.6 `form`

Modal text-input prompt. Used by the first-launch wizard, path edits,
etc. UI **MUST** reply with `form_decision` echoing the same `id`.
Mirrors `ProgressEvent::Form` (and `FormRequest`).

| Field     | Type     | Notes                                                          |
|-----------|----------|----------------------------------------------------------------|
| `id`      | `number` | Unsigned 64-bit; correlation key.                              |
| `label`   | `string` | Question shown above the input box.                            |
| `initial` | `string` | Pre-fill for the input. Empty string for a fresh entry.        |
| `hint`    | `string` | Help text shown below the input (e.g. accepted path formats).  |

```json
{"type":"form","id":1,"label":"Enter the path to your FLAC source library","initial":"","hint":"UNC paths like \\\\server\\music are supported"}
```

### 4.7 `track_start`

Emitted once per track at the start of its apply step. Mirrors
`ProgressEvent::TrackStart`.

| Field       | Type               | Notes                                              |
|-------------|--------------------|----------------------------------------------------|
| `current`   | `number`           | 1-based index of this track within the run.        |
| `total`     | `number`           | Total tracks planned (matches `summary.total_planned`). |
| `label`     | `string`           | Human-readable label, e.g. `"Aphex Twin - Selected Ambient Works II - #ATC1"`. |
| `eta_secs?` | `number` \| absent | Estimated seconds remaining, whole-run average (elapsed time since the first track divided by completed-track count, projected over remaining tracks). Omitted before the first track completes. Since **1.2.0**. |

```json
{"type":"track_start","current":1,"total":15,"label":"Aphex Twin - Selected Ambient Works II - #ATC1"}
```

```json
{"type":"track_start","current":6,"total":15,"label":"Aphex Twin - Selected Ambient Works II - #ATC2","eta_secs":47}
```

### 4.8 `track_done`

Emitted once per track at the end of its apply step. Carries no fields.
The UI increments its "completed" counter on receipt. Mirrors
`ProgressEvent::TrackDone`.

```json
{"type":"track_done"}
```

### 4.9 `log`

An informational log line emitted deliberately by the orchestrator (e.g.
`"transcoded via ffmpeg n7.0 in 6.3s"`). **Not** routed from
`tracing::*` — those go to the core's log file (see §9). Only explicit
`progress.log()` calls become `log` events. Mirrors `ProgressEvent::Log`.

| Field     | Type     | Notes               |
|-----------|----------|---------------------|
| `message` | `string` | The log line text.  |

```json
{"type":"log","message":"transcoded via ffmpeg n7.0 in 6.3s"}
```

### 4.10 `error`

Non-fatal or fatal error. Multiple `error` events MAY be emitted during
a single run; the UI **SHOULD** accumulate them into a list and surface
them in a "Sync log" panel.

A **fatal** error is followed shortly by `finish` with `success: false`
(see §4.11). A **non-fatal** error stands on its own — the run
continues. Mirrors `ProgressEvent::Error` (with optional
`recovery_hints` added on the wire that the internal variant doesn't
carry today; the core emits an empty / omitted array for M1).

| Field             | Type        | Notes                                                                                      |
|-------------------|-------------|--------------------------------------------------------------------------------------------|
| `message`         | `string`    | Human-readable error description.                                                          |
| `recovery_hints?` | `string[]`  | Optional. Short actionable next-step suggestions. Omitted when empty. Reserved for future. |

```json
{"type":"error","message":"ffmpeg failed for /path/to/track.flac","recovery_hints":["Skip this track","Verify the source file isn't corrupt"]}
```

```json
{"type":"error","message":"manifest write failed: disk full"}
```

### 4.11 `finish`

Final message of a run. The core will close stdout shortly after writing
this. The UI uses it as the signal to show a "done" state and to expect
the child process to exit with code 0 (on `success: true`) or non-zero
(on `success: false` — though current M1 wiring may exit 0 in both
cases; rely on `success` for the user-facing verdict). Mirrors
`ProgressEvent::Finish`.

| Field                 | Type      | Notes                                                |
|-----------------------|-----------|-------------------------------------------------------|
| `success`             | `boolean` | `true` for a clean run, `false` for fatal failure.   |
| `skipped_for_space?`  | `object` \| absent | **Since 1.3.0.** Fit-pass deferral rollup — whole albums that didn't fit the device's remaining space, even after the end-of-run retry (§ below). Absent when nothing was deferred. |
| `artwork?`            | `object` \| absent | **Since 1.3.0.** Cover-art rollup across this run's Add/Modify/MetadataOnly actions (Task 13; previously reserved and always absent). Absent when the run never reached the apply loop (dry-run, review-abort, or "nothing to do" with no pending artwork repair). |
| `db_restored?`        | `boolean` \| absent | **Since 1.3.0.** `true` when the core's auto-restore-from-backup path fired this run (the iTunesDB failed to parse and was replaced from the session backup before the sync proceeded). Absent (not `false`) when it didn't fire. |

`skipped_for_space`, when present, has:

| Field    | Type     | Notes                                                          |
|----------|----------|-----------------------------------------------------------------|
| `albums` | `number` | Count of distinct albums deferred.                              |
| `tracks` | `number` | Total tracks across those albums.                                |
| `bytes`  | `number` | Total source bytes across those albums (the fit pass's estimate, not necessarily the exact on-iPod transcoded size). |

`artwork`, when present, has `embedded`, `eligible`, `failed_sources` (all
`number`): `eligible` counts sources with embedded art, `embedded` counts
those successfully written to the device (Apple thumbnail and/or, under
`rockbox_compat`, the on-device file's own tags), `failed_sources` counts
those whose art extraction/decode failed (also warn-logged with the source
path).

A pre-1.3.0 core never emits `skipped_for_space`/`artwork`/`db_restored`;
a UI built against 1.3.0 must treat their absence as "nothing to report",
per the standard additive-minor-bump rule in §1.

```json
{"type":"finish","success":true}
```

```json
{"type":"finish","success":false}
```

```json
{"type":"finish","success":true,"skipped_for_space":{"albums":14,"tracks":183,"bytes":9876543210}}
```

```json
{"type":"finish","success":true,"db_restored":true}
```

### 4.12 `paused`

**New in v1.1.0.** Terminal event emitted when the core gracefully honors a
`pause` command (§5.6) instead of running to completion. By the time this
event is written, the apply loop has drained its in-flight transcode window,
committed every track it had already reached in plan order, run a final
`db.write()` + manifest save (the same checkpoint the periodic time-or-count
checkpoint uses — see `checkpoint::CheckpointClock`), and stopped. No fields.

The core closes stdout shortly after, same as `finish` — the UI should treat
`paused` as a **third terminal outcome** alongside `finish{success:true}` and
`finish{success:false}`, not as a sub-case of either. A subsequent run is a
perfectly normal invocation: the diff-based plan naturally picks up wherever
the manifest left off, so there is no separate "resume" command (see §5.6).

```json
{"type":"paused"}
```

In practice the core still emits a trailing `finish{success:true}` right
after `paused` (§4.11) — `paused` itself carries no fields, but the run's
rollup (`skipped_for_space`, `artwork`, `db_restored`) is only ever attached
to `finish`, so the daemon's history entry for a paused run reads those
fields off that trailing `finish`, not off `paused`.

---

## 5. Commands (UI → core)

| `type`             | Fields                            | Purpose                                          |
|--------------------|-----------------------------------|--------------------------------------------------|
| `review_decision`  | `decision` (object)               | Reply to a `review` event                        |
| `prompt_decision`  | `id`, `choice`                    | Reply to a `prompt` event                        |
| `form_decision`    | `id`, `value`                     | Reply to a `form` event                          |
| `cancel`           | (none)                            | Graceful shutdown request                        |
| `pause`            | (none)                            | Graceful pause request (new in **1.1.0**)        |

A `start` command (`{"type":"start"}`) is **reserved** for future
milestones — M1 begins orchestration implicitly on spawn. Older cores
silently ignore it; the wire shape is `{"type":"start"}` with no fields.

### 5.1 The `decision` nested object

`review_decision` carries the choice as a nested typed-envelope object
named `decision`. This mirrors the Rust `ReviewDecision` enum (see
`src/progress.rs`):

```rust
pub enum ReviewDecision {
    Apply { no_delete: bool },
    DryRun,
    Quit,
}
```

The wire encoding uses the same `type` discriminator pattern as the
top-level envelope, but scoped inside the `decision` field:

| Variant   | JSON shape                                            |
|-----------|-------------------------------------------------------|
| `Apply`   | `{"type": "apply", "no_delete": <bool>}`              |
| `DryRun`  | `{"type": "dry_run"}`                                 |
| `Quit`    | `{"type": "quit"}`                                    |

### 5.2 `review_decision`

Reply to a `review` event (§4.4). Sent exactly once per `review`.

| Field      | Type       | Notes                                                |
|------------|------------|------------------------------------------------------|
| `decision` | `Decision` | Nested object; one of Apply / DryRun / Quit. See §5.1. |

```json
{"type":"review_decision","decision":{"type":"apply","no_delete":false}}
```

```json
{"type":"review_decision","decision":{"type":"apply","no_delete":true}}
```

```json
{"type":"review_decision","decision":{"type":"dry_run"}}
```

```json
{"type":"review_decision","decision":{"type":"quit"}}
```

### 5.3 `prompt_decision`

Reply to a `prompt` event (§4.5). The `id` MUST echo the `id` of the
originating prompt. `choice` is a zero-based index into the prompt's
`options` array.

| Field    | Type     | Notes                                                                  |
|----------|----------|------------------------------------------------------------------------|
| `id`     | `number` | Unsigned 64-bit. MUST match a `prompt`.id the core sent.               |
| `choice` | `number` | Zero-based index into the prompt's `options[]`. Out of range = abort.  |

```json
{"type":"prompt_decision","id":7,"choice":1}
```

(Selects `"Skip this track"` from the example in §4.5.)

> **On range:** the Rust side currently maps out-of-range choices to an
> Abort outcome inside the orchestrator's `try_with_prompt` plumbing.
> The UI SHOULD validate locally and only ever send a valid index, but
> the core will not crash on a bad one.

### 5.4 `form_decision`

Reply to a `form` event (§4.6). The `id` MUST echo the originating
`form`.id.

| Field   | Type              | Notes                                                         |
|---------|-------------------|---------------------------------------------------------------|
| `id`    | `number`          | Unsigned 64-bit.                                              |
| `value` | `string \| null`  | The user's input, **trimmed**, or `null` if the user aborted. |

```json
{"type":"form_decision","id":1,"value":"\\\\nas\\music\\flac"}
```

```json
{"type":"form_decision","id":1,"value":null}
```

The core treats `null` as a user abort (equivalent to pressing Esc /
Ctrl+C in the TUI). It treats an empty-string value as if the user had
pressed Enter on an empty input — current TUI behavior is to ignore
that and keep waiting, so the UI **SHOULD** locally suppress empty
submissions or send `null` instead.

### 5.5 `cancel`

Request a graceful shutdown. No fields.

```json
{"type":"cancel"}
```

**M1 behavior:**

- If the core is currently awaiting a `review_decision`, `cancel` is
  internally mapped to `review_decision { decision: { type: "quit" } }`
  — the orchestrator unwinds cleanly without writing the manifest.
- If the core is mid-sync (between `track_start` events), `cancel` is
  **best-effort**: the orchestrator does not yet support cooperative
  cancellation, so the core finishes the current track and only then
  observes the request. The UI MUST therefore back its `cancel` with a
  bounded-wait force-kill (see §7).

Future milestones will plumb `cancel` deeper into the orchestrator so
it can interrupt mid-track work.

### 5.6 `pause`

**New in v1.1.0.** Request a graceful pause. No fields.

```json
{"type":"pause"}
```

Unlike `cancel`, `pause` is **not** a shutdown-and-discard request — it is
the "stop cleanly and let me come back later" command:

- The orchestrator polls for a queued `Decision::Pause` between actions in
  the same non-blocking `try_recv()` spot that already polls for the
  cancel-mapped `Decision::Review(ReviewDecision::Quit)`. On seeing it, the
  apply loop stops accepting new actions, drains the in-flight transcode
  pipeline window (`pipeline::OrderedTranscoder`), commits everything it
  already reached in plan order, runs a final checkpoint (`db.write()` +
  manifest save), and returns.
- The core then emits `{"type":"paused"}` (§4.12) and closes stdout — the UI
  should treat this like `finish`, not wait for a separate `finish` too.
- **There is no `resume` command.** Because sync is diff-based against the
  manifest, resuming is just starting a normal new run (a plain re-spawn of
  `ipod-sync.exe --ipc-mode --apply`, or — on the daemon-pipe wire — an
  ordinary `trigger_sync`, §"Daemon v1.2.0" below). The plan naturally picks
  up only the tracks that weren't committed before the pause.
- Like `cancel`, `pause` is **best-effort while mid-track**: the core
  finishes whatever an in-flight worker is transcoding before treating the
  slot as drained. It does not interrupt a single transcode partway through.

---

## 6. Correlation rules

1. A `prompt_decision` MUST include the same `id` as the originating
   `prompt`.
2. A `form_decision` MUST include the same `id` as the originating
   `form`.
3. Prompt and form IDs are allocated by the core (via
   `Progress::next_prompt_id`); they are unique per process run and
   monotonically increasing. They are NOT reused.
4. Multiple `prompt` events with different `id`s MAY be in flight
   simultaneously. Replies are matched by `id`, not by arrival order.
   (M1 doesn't actually issue concurrent prompts, but the wire format
   supports it.)
5. An unrecognized `id` in a `*_decision` is not fatal: the core logs a
   warning and discards the reply. The UI SHOULD NOT send decisions for
   IDs it never received.
6. There is no per-message reply for any other event type. `header`,
   `summary`, `log`, `error`, `track_start`, `track_done`, `finish` are
   one-way notifications — the UI does NOT ack them.
7. A `review_decision` does not carry an `id`: at most one `review` is
   in flight at any time, so correlation is implicit.
8. `cancel` is fire-and-forget. The core does not ack it; the UI infers
   success from the subsequent `finish` event and child-process exit.

---

## 7. Process lifecycle

### Spawn

1. The UI launches `ipod-sync.exe --ipc-mode` as a child process with
   stdin and stdout piped (stderr may be inherited or captured for
   crash diagnostics).
2. The UI starts its stdout-reader loop on a background thread.
3. The core writes `hello` and flushes stdout.
4. The UI reads the `hello`, validates `protocol_version` (§1), and
   either proceeds or tears down on mismatch.

### Normal run

5. The core runs the orchestrator. Events flow on stdout; the UI
   updates its ViewModels on each event.
6. When the orchestrator requires a decision (`review`, `prompt`,
   `form`), the core writes the event and blocks on the internal
   decision channel. The UI's reply on stdin is read by the
   `IpcBackend`'s stdin-reader thread, converted to a `Decision`, and
   delivered to the orchestrator via the same `mpsc::channel` the TUI
   uses today.
7. Per-track progress: `track_start` → optional `log`/`error` → `track_done`.
8. End of run: the core writes `finish` (with `success: true` or
   `false`), then closes stdout, then exits with code `0` (M1 unifies
   exit codes — `success: false` does not currently propagate to a
   non-zero exit; UIs rely on the `finish` payload).

### Graceful cancel

9. UI sends `{"type":"cancel"}` on stdin and starts a 5 s timer.
10. The core observes the cancel, maps it (per §5.5), and tears down.
11. The UI expects:
    - a `finish` event on stdout,
    - then stdout EOF,
    - then process exit, all within the 5 s window.
12. **If the deadline expires**, the UI calls
    `Process.Kill(entireProcessTree: true)` (or the OS equivalent on
    macOS/Linux frontends) and surfaces a warning in the log panel. No
    orphan core processes should remain.

### Graceful pause

13. UI sends `{"type":"pause"}` on stdin (§5.6).
14. The core finishes draining its in-flight transcode window, commits
    everything already reached in plan order, checkpoints, and emits
    `{"type":"paused"}` (§4.12).
15. The UI expects a `paused` event, then stdout EOF, then a clean process
    exit — no force-kill timer needed; pause is not racing a shutdown
    deadline the way `cancel` is. If the process doesn't exit within a
    generous bound (tens of seconds — draining the window can legitimately
    take as long as one transcode), fall back to the same force-kill path
    as §"Graceful cancel" and log it as unexpected.
16. A later sync is an ordinary new run: the UI spawns the core again (or,
    over the daemon-pipe wire, sends `trigger_sync`); the diff-based plan
    resumes from the manifest automatically. There is no dedicated
    "resume" message on either wire.

This mirrors the bounded-join pattern already used by
`Progress::finish` in `src/progress.rs` (5 s deadline, force-exit on
timeout).

### Crash / unexpected EOF

13. If the UI reads stdout EOF **without** having seen a `finish` event,
    treat the core as crashed.
14. The UI SHOULD show a crash dialog with the captured stderr tail and
    a "Show log file" button pointing at the core's tracing log (see
    §9).
15. The UI MUST reset its in-memory state so a subsequent run starts
    clean — no orphaned ViewModel showing stale progress.
16. **Broken pipe on stdin (UI dies first):** the core's stdin read
    loop sees EOF on stdin and exits the reader thread. The core
    treats this as a request to stop and shuts down cleanly at the next
    decision point (current M1 behavior; mid-sync interruption is
    best-effort as with cancel).

---

## 8. Stream semantics (recap)

- One JSON object per `\n`-terminated line.
- UTF-8 throughout; the writer MUST NOT emit literal control characters
  inside string values — newlines, tabs, etc. are JSON-escaped.
- The core `flush()`es stdout after every write. The UI MUST NOT rely
  on the OS to flush pipe buffers for it.
- An empty line is silently skipped on read (defensive).
- Trailing `\r` (Windows CRLF in transit) is tolerated on both sides —
  parsers trim whitespace before parsing.
- Order on a given stream is preserved. There is no ordering relation
  between events and commands beyond the prompt/form correlation
  rules — the core may emit a `log` while waiting for a
  `prompt_decision`, and the UI may send a `cancel` while the core is
  mid-emit.

---

## 9. Logging and debugging

The core's `tracing` output is **never** routed through the IPC stream
(that would flood stdout with libgpod CRITICALs and noise). Instead, in
`--ipc-mode` the core writes its tracing log to a file:

- **Windows:** `%LOCALAPPDATA%\ipod-sync\logs\core-{unix_timestamp}.log`
- **macOS (future frontends):** `~/Library/Logs/ipod-sync/core-{ts}.log`
- **Linux (future frontends):** `$XDG_STATE_HOME/ipod-sync/logs/core-{ts}.log`
  (falling back to `~/.local/state/ipod-sync/logs/`)

Only explicit `progress.log()` and `progress.error()` calls cross the
IPC boundary as `log` / `error` events. This keeps the wire low-volume
and the parser unambiguous.

The UI is encouraged to keep a parallel debug log of every line it
reads from stdout (and every line it writes to stdin), under
`%LOCALAPPDATA%\ipod-sync\logs\ui-{unix_timestamp}.log` on Windows
(equivalent on other platforms). This makes "what did the core actually
send?" trivially answerable after the fact.

### Secrets policy

- Source paths, iPod paths, manifest paths, and track titles MAY appear
  in logs. For M1 this is acceptable — the tool is single-user, local,
  no telemetry, no upload.
- Neither side should ever log credentials, OAuth tokens, or anything
  resembling one. (None are used today; this is forward-looking.)
- Revisit the log policy if/when remote sync sources land.

---

## 10. Out of scope for protocol v1

Deliberately **not** in v1 — listed so they don't accidentally creep in:

- **Streaming binary payloads.** No track audio previews, no album-art
  thumbnails on the wire. Art that needs to render in the UI is
  resolved by the UI directly from disk (the source file path is in the
  manifest the UI can read locally).
- **Bidirectional notifications during apply.** The only mid-sync UI →
  core messages are `cancel` and (since v1.1.0) `pause`, both best-effort.
  No skip-current-track command yet. There is still no dedicated `resume`
  command — see §5.6; resuming after a pause is an ordinary new run.
- **Authentication / authorization.** Single-user local tool. The
  parent process owns the child by construction; no auth needed.
- **Multiplexing multiple iPods over one IPC channel.** M1 assumes one
  iPod per core process. UI multi-iPod support would spawn multiple
  cores, one per device.
- **Config read/write.** Wizard-driven config persistence (M2) will
  either extend the protocol with `read_config` / `write_config`
  round-trips or have the UI write `config.toml` directly; the decision
  is deferred to M2 brainstorm.
- **Library browse / track listing.** Lands in M3 as a `list_tracks` /
  `tracks` event-stream addition; will bump the protocol minor version
  (additive, backwards-compatible).
- **Telemetry / crash reporting.** No.

---

## 11. Compatibility matrix

| Protocol | Core version | UI version | Status     | Notes                                  |
|----------|--------------|------------|------------|----------------------------------------|
| 1.0.0    | 0.1.x        | 0.1.x      | Initial M1 | Windows-only UI; cross-platform TUI fallback remains. |
| 1.1.0    | 0.1.x        | 0.1.x      | Superseded | Additive: `pause` command (§5.6) + terminal `paused` event (§4.12). Handshake still requires major version `1` on both sides. |
| 1.2.0    | 0.1.x        | 0.1.x      | Superseded | Additive: optional `eta_secs` field on `track_start` (§4.7), daemon-computed whole-run-average sync ETA. |
| 1.3.0    | 0.1.x        | 0.1.x      | Current    | Additive: fit engine wired into the apply loop — `finish` gains optional `skipped_for_space`, `artwork` (previously reserved and always absent; populated as of this bump), and `db_restored` fields (§4.11). |

Bumps will append rows here. Don't edit historical rows.

---

## v1.1.0 — Daemon extensions (UI ↔ daemon channel)

> **Namespace note:** the daemon-pipe protocol versions independently of the
> subprocess stdio protocol documented in §§1–11 above — they share the
> `major.minor.patch` scheme and both currently sit on major `1`, but their
> minor versions move on separate schedules and a matching number (e.g. both
> once being `1.1.0`) is coincidence, not a shared release train. As of this
> writing the daemon protocol is at **`2.0.0`** — see "Daemon v2.0.0" below;
> this section is kept as the historical record of the `1.1.0` daemon bump.

When the wire transport is the named pipe `\\.\pipe\ipod-sync` (Windows) or a
Unix domain socket (macOS/Linux), the daemon emits `hello` with
`protocol_version = "1.1.0"`. The v1.0.0 envelope shape is unchanged; v1.1.0
only adds new event and command types.

On macOS the socket is `$TMPDIR/classick.sock` — the Darwin per-user temp
directory resolved via `confstr(_CS_DARWIN_USER_TEMP_DIR)`, the same
directory `$TMPDIR` points at. Swift clients resolve the identical path via
`NSTemporaryDirectory()`. The app is **not** App-Store-sandboxed, so this
directory is shared between the daemon and the UI client — no App Group or
sandbox container entitlement is required for the socket to be visible to
both processes. (Linux falls back to `$XDG_RUNTIME_DIR`, then `$TMPDIR`,
then `/tmp`, with the same `classick.sock` file name.)

### New events (daemon → UI)

| Type | Fields |
|---|---|
| `status_update` | `state` (idle/syncing), `configured` (bool), `ipod_connected` (bool), `last_sync` (HistoryEntry?), `next_scheduled_unix_secs` (u64?) |
| `config_update` | `source` (str?), `daemon` (DaemonSettings?), `ipod` (IpodIdentity?) |
| `history_update` | `entries` (HistoryEntry[]) |
| `device_connected` | `serial` (str), `model_label` (str), `drive` (str) |
| `device_disconnected` | `serial` (str) |
| `sync_rejected` | `reason` ("already_syncing" | "no_ipod" | "not_configured") |

### New commands (UI → daemon)

| Type | Fields |
|---|---|
| `get_status` | (none) — replies with `status_update` |
| `get_config` | (none) — replies with `config_update` |
| `save_config` | `source?` (str), `daemon?` (DaemonSettings), `ipod?` (IpodIdentity) — replies with `config_update` |
| `trigger_sync` | `source` ("manual"/"scheduled"/"plug_in") — replies with `sync_rejected` or nothing (sync proceeds, sync events forwarded) |
| `get_history` | `limit` (default 10) — replies with `history_update` |
| `subscribe_device_events` | (none) — daemon starts forwarding `device_connected` events for any iPod, not just configured |
| `unsubscribe_device_events` | (none) |
| `shutdown` | (none) — daemon exits cleanly after draining current sync |

### `IpodIdentity` gains `custom_selection`

```json
{"type":"save_config","ipod":{"serial":"000A27002138B0A8","model_label":"iPod Classic 7G","custom_selection":true}}
```

| Field | Type | Notes |
|---|---|---|
| `custom_selection` | `bool` | Default `false` (shared selection). When `true`, this iPod's sync selection is read from/written to its own per-device `devices/<serial>/selection.json` instead of the shared `<config>/classick/selection.json`. Rides the existing `ipod` field on `config_update`/`save_config` (this table, above) — no new command needed to read or set it. Also governs which file the existing `get_selection`/`save_selection` commands (§"Daemon v1.4.0" below) act on. Older clients that don't send this field get the persisted default (`false`) on load, so they keep reading/writing the shared file exactly as before. |

The first time a device's `custom_selection` flips `false → true` (including
"no prior identity for this serial", e.g. a brand-new device), the daemon
seeds the per-device file by copying the current shared `selection.json` so
the user's existing choices carry over instead of resetting to "sync
everything". Flipping back to `false` leaves the per-device file in place,
untouched and dormant — flipping `true` again later re-reads it as-is (no
re-seed, since the per-device file already exists).

### Forwarded sync-subprocess events

When the daemon is running a sync, it spawns `ipod-sync --ipc-mode --apply`
and forwards every v1.0.0 IpcEvent (`header`, `summary`, `review`, `prompt`,
`form`, `track_start`, `track_done`, `log`, `error`, `finish`) verbatim to
subscribed UI clients. UI clients see daemon events and sync events on the
same pipe and pattern-match on `type`.

## M3 addendum (2026-05-25) — Device events go live, TooManyFailures reason

### Device-event flow

Starting in M3 (protocol still 1.1.0), the daemon broadcasts
`device_connected` / `device_disconnected` events to ALL connected
clients (not just those that sent `subscribe_device_events`). The
Subscribe / Unsubscribe commands remain in the protocol as
no-op handshakes; clients should still send `subscribe_device_events`
for forward-compatibility — M4 may reintroduce per-client filtering.

Production detection uses a 1.5s polling loop over Windows drive
letters; expected first-event latency is therefore 0–1.5s from physical
plug-in, +500ms debounce window. Tests that need different cadence
inject a custom `DeviceWatcher` impl (see
`src/daemon/device_watcher.rs`).

### Sync orchestration

When the daemon accepts a sync trigger (plug-in, scheduled, or
manual), it spawns `ipod-sync.exe --ipc-mode --apply --ipod <drive>`.
The subprocess speaks the M1 v1.0.0 stdio protocol; the daemon parses
each line and (M4) will forward to UI clients. Throughout the sync,
the daemon counts per-track `error` events. When
`tracks_errored * 2 > total_planned` (strict greater-than, both > 0),
the daemon sends `{"type":"cancel"}` to the subprocess stdin, starts a
5-second force-kill timer, and emits:

```json
{"type":"sync_rejected","reason":"too_many_failures"}
```

The history entry for that run records `outcome: "aborted"` with
`error_message: "too_many_failures: N of M tracks failed"`.

### New SyncRejectReason

| Reason | When |
|---|---|
| `already_syncing` | TriggerSync while state == Syncing |
| `no_ipod` | TriggerSync while no device connected |
| `not_configured` | TriggerSync while config.ipod_identity is None |
| `too_many_failures` | Auto-bail from >50% per-track failure threshold (NEW M3) |

### Mid-sync device-detach handling

When `DeviceWatcher` fires `Disconnected` for the serial currently
being synced, the daemon:
1. Records a history entry with `outcome: "aborted"`, `error_message: "device_detached"`.
2. Transitions state back to Idle.
3. Lets the orchestrator subprocess error out naturally as libgpod
   writes start failing. The subprocess's own Finish event arrives
   later and is ignored (state is already Idle).

## Daemon v1.2.0 — Pause forwarding + "X of Y synced" counts (2026-07-13)

The daemon now emits `hello` with `protocol_version = "1.2.0"`. Purely
additive over v1.1.0 above: one new command, two new `status_update` fields.
Handshake compatibility is unaffected — both sides still only refuse to
proceed on a major-version mismatch (§1).

### New command: `pause`

```json
{"type":"pause"}
```

Forwards to the running sync subprocess as `{"type":"pause"}` on its stdin
(the subprocess-protocol `pause` command, §5.6) — it does **not** force-kill
or start a grace-period timer the way `cancel_sync` does, because pause is
meant to let the subprocess drain and exit on its own. The daemon still
keeps the same bounded-kill backstop as a defensive fallback in case the
subprocess never exits. **No-op if the daemon is idle** (no sync running).
When the subprocess emits its terminal `{"type":"paused"}` line, the
orchestrator records the sync as paused (not aborted, not completed), and
the daemon broadcasts an Idle `status_update`. Resuming is not a distinct
command — it's an ordinary `trigger_sync` (manual or scheduled), which the
diff-based plan picks up from wherever the manifest left off.

### `status_update` gains `synced_count` and `library_count`

```json
{"type":"status_update","state":"idle","configured":true,"ipod_connected":true,"synced_count":812,"library_count":1500}
```

| Field | Type | Notes |
|---|---|---|
| `synced_count` | `number` | Tracks currently on the iPod per the manifest — the "X" in "X of Y synced". Always present; a fresh manifest-length read, so it's cheap and never stale. |
| `library_count` | `number \| omitted` | Source-library track count — the "Y". Omitted (not `null`) until known. The daemon doesn't walk the source library on every status tick; this is populated from the most recent sync's action-plan diff (which already walks the source) and cached from there. Older clients that don't know this field ignore it per the standard additive-field rule (§1). |

Both fields are additive to the existing `status_update` shape from v1.1.0
(`state`, `configured`, `ipod_connected`, `last_sync`,
`next_scheduled_unix_secs`, `storage`) — no existing field changed meaning.

## Daemon v1.3.0 — Rockbox compatibility: `rockbox_compat` setting + `backfill_rockbox` (2026-07-13)

The daemon now emits `hello` with `protocol_version = "1.3.0"`. Purely
additive over v1.2.0 above: one new field on an existing settings struct,
and one new command. Handshake compatibility is unaffected — both sides
still only refuse to proceed on a major-version mismatch (§1). See
`docs/superpowers/specs/2026-07-13-rockbox-compatibility-design.md` for the
full feature design.

### `DaemonSettings` gains `rockbox_compat`

```json
{"type":"save_config","daemon":{"enabled":true,"autostart_with_windows":false,"first_sync_mode":"review","subsequent_sync_mode":"auto_apply","schedule_minutes":30,"notify_on":"all","rockbox_compat":true}}
```

| Field | Type | Notes |
|---|---|---|
| `rockbox_compat` | `bool` | Default `false`. When `true`, every subsequently-transcoded `.m4a` is made self-describing (embedded ID3/MP4 tags + normalized cover art) so Rockbox firmware can read the library directly off the iPod, alongside the libgpod-managed iTunesDB. Read and written via the existing `get_config`/`save_config`/`config_update` commands (§"New commands"/"New events" above) — no new command is needed just to toggle it. Older clients that don't send this field get the persisted default (`false`) on load; older UI builds that don't know the field simply never render its toggle. |

When `rockbox_compat` is on, the daemon appends `--rockbox-compat` to the
`--ipc-mode --apply --ipod <drive>` sync-subprocess command line (§"Sync
orchestration" above) — read fresh from the persisted config at the moment
each sync is spawned, so a Settings change takes effect on the very next
sync without a daemon restart.

### New command: `backfill_rockbox`

```json
{"type":"backfill_rockbox"}
```

One-shot, user-triggered retrofit for a library that was already synced
with `rockbox_compat` off (or before this feature existed): embeds tags +
cover art into the **existing** on-iPod `.m4a` files in place, without
re-transcoding or touching the add/modify/remove plan. The daemon spawns
`classick.exe --ipc-mode --backfill-rockbox --ipod <drive>` and reports
progress through the **same forwarded-event vocabulary** as a normal sync
(`summary`, `track_start`, `track_done`, `log`, `error`, `finish` — see
"Forwarded sync-subprocess events" above) — UI clients don't need to
special-case a backfill's progress display.

`backfill_rockbox` reuses the exact same state-machine guard, cancel/pause
signaling, and prompt-decision relay as `trigger_sync`: it is **no-op if a
sync (or another backfill) is already in progress**, and no-op if no iPod
is currently connected. Because it shares the guard with `trigger_sync`, a
sync and a backfill can never run concurrently — whichever request lands
first occupies the `Syncing` state until it completes. Unlike
`trigger_sync`, it does not reply with `sync_rejected` on those no-op
paths; the daemon just logs and drops the request, matching the existing
`pause`/`cancel_sync` no-op style.

## Daemon v1.4.0 — Library selection: browse, scan, choose what syncs (2026-07-14)

The daemon now emits `hello` with `protocol_version = "1.4.0"`. Purely
additive over v1.3.0: five new commands, three new events, one new
`status_update.state` value, and a semantics clarification for
`library_count`. See
`docs/superpowers/specs/2026-07-14-library-selection-design.md`.

### New commands (UI → daemon)

| Type | Fields | Behavior |
|---|---|---|
| `get_library` | (none) | Replies `library_update` from the cached library index. Never-scanned → `scanned_at_unix_secs: null` + empty collections. |
| `scan_library` | (none) | Spawns `classick --ipc-mode --scan-library` under the same state-machine guard as `trigger_sync`/`backfill_rockbox` (no-op, log + drop, if busy or no source configured). Progress arrives as forwarded subprocess events; on finish the daemon reloads the index and broadcasts a fresh `library_update`. |
| `get_selection` | (none) | Replies `selection_update`. |
| `save_selection` | `mode`, `rules` | Persists selection.json atomically; replies `selection_update`; broadcasts a refreshed `status_update`. |
| `preview_selection` | `mode`, `rules` | Pure computation, no persistence. Replies `selection_preview`. |

`mode` is `"all" | "include" | "exclude"`. Each rule is one of:
`{"kind":"artist","name":…}`, `{"kind":"album","artist":…,"album":…}`,
`{"kind":"genre","name":…}`. Matching is case-insensitive; "artist" means
album_artist falling back to track artist; empty strings are the
Unknown-Artist / No-Genre buckets.

### New events (daemon → UI)

| Type | Fields |
|---|---|
| `library_update` | `source_root` (str?), `scanned_at_unix_secs` (u64 \| null; null = never scanned), `artists[]` — `{name, albums[]: {name, genre?, tracks, bytes}}` — `genres[]: {name, tracks, bytes}`, `total_tracks`, `total_bytes`. Aggregated, never per-track. An album's `genre` is display-only (most common among its tracks; omitted on tie/absence); genre rules match per-track. |
| `selection_update` | `mode`, `rules` — mirror of selection.json. |
| `selection_preview` | `selected_tracks`, `selected_bytes` (source bytes — an estimate of on-iPod size), `adds`, `removes` (vs the manifest). |

### `status_update.state` gains `"scanning"`

Emitted while a library scan subprocess runs. **Clients MUST treat unknown
`state` values as `idle`** — this is the standing rule for all future state
additions, matching §2's unknown-message tolerance.

### `library_count` semantics

`status_update.library_count` is now the **selected** track count — the "Y"
in "X of Y synced" is what the current selection wants on the iPod, not the
raw folder count. Under `mode: "all"` the value is unchanged.

---

## Daemon v1.5.0 — Skipped-for-space + artwork summary on history (2026-07-17)

The daemon now emits `hello` with `protocol_version = "1.5.0"`. Purely
additive over v1.4.0: one new command (`replace_library`, below); no changed
or removed fields — three new fields on the persisted `SyncSummary` shape
and one new field on `HistoryEntry`, both of which already ride the
existing `status_update.last_sync` and `history_update.entries` payloads
(see the `v1.1.0` daemon-extensions section above for those event shapes).
No separate status plumbing was needed: `last_sync: Option<HistoryEntry>`
on `status_update` carries whatever fields `HistoryEntry` has, so UIs pick
these up automatically once the daemon starts persisting them.

The daemon builds these fields from the sync subprocess's `finish` event
(§4.11 above), which since subprocess protocol `1.3.0` already carries
`skipped_for_space` (whole-album fit-pass deferral) and `artwork` (embed
rollup) — Task 8 added those wire fields; this bump is the daemon actually
reading and persisting them into `history.json`, plus `db_restored`.

### `SyncSummary` gains three fields

| Field | Type | Notes |
|---|---|---|
| `skipped_for_space_tracks` | `usize` | From the subprocess `finish` event's `skipped_for_space.tracks`. `0` when nothing was deferred, or the field was absent (older core, or an all-fit run). |
| `skipped_for_space_bytes` | `u64` | From `skipped_for_space.bytes`. |
| `artwork_failed_sources` | `usize` | From the subprocess `finish` event's `artwork.failed_sources`. |

Note `skipped_for_space.albums` (also present on the subprocess wire event)
is deliberately **not** persisted onto `SyncSummary` — only `tracks`/`bytes`
carry through. All three fields are `#[serde(default)]`: pre-existing
`history.json` entries (written before this field existed) deserialize with
them at `0`.

### `HistoryEntry` gains `db_restored`

| Field | Type | Notes |
|---|---|---|
| `db_restored` | `bool` | Mirrors the subprocess `finish` event's `db_restored` (§4.11) — `true` when Task 4's auto-restore-from-backup path fired during that sync. `#[serde(default, skip_serializing_if = "std::ops::Not::not")]`: omitted from the wire/`history.json` when `false` (old-client-compat, matching the subprocess field's own convention), and pre-existing entries without it deserialize to `false`. |

Only `OrchestratorOutcome::Completed` (a subprocess run that reached its
terminal `finish` line) ever populates a non-default `db_restored` — a
sync that's cancelled, bailed past the 50%-failure threshold, or force-
killed after a stalled pause never gets to read `finish`, so those history
entries keep `db_restored: false`.

### New command: `replace_library`

```json
{"type":"replace_library"}
```

One-shot, user-triggered "erase and start over": wipes **every** track
currently on the iPod, then falls straight through to an ordinary sync of
the current selection (`apply_loop::replace_library`, Task 11). The daemon
spawns `classick.exe --ipc-mode --replace-library --apply --ipod <drive>`
and reports progress through the **same forwarded-event vocabulary** as a
normal sync (`summary`, `track_start`, `track_done`, `log`, `error`,
`finish` — see "Forwarded sync-subprocess events" above) — UI clients
don't need to special-case a replace's progress display. `--apply` is what
makes the core skip its own interactive erase-confirmation prompt; the UI
is expected to obtain the user's confirmation itself (a typed/explicit
confirmation, not a simple yes/no) before ever sending this command.

`replace_library` reuses the exact same state-machine guard, cancel/pause
signaling, and prompt-decision relay as `trigger_sync`/`backfill_rockbox`:
it is rejected if a sync (or a backfill, or another replace) is already in
progress, and rejected if no iPod is currently connected. Because it shares
the guard, a sync/backfill/replace can never run concurrently — whichever
request lands first occupies the `Syncing` state until it completes.
Unlike `backfill_rockbox`/`scan_library` (which stay silent and just log +
drop on their no-op paths), `replace_library` is destructive — it wipes
every track on the iPod — so both guards reply with `sync_rejected`
(`reason: "already_syncing"` or `"no_ipod"`), matching `trigger_sync`'s
reply mechanism, so the UI always gets a definitive answer rather than a
request that silently goes nowhere. The resulting history entry records
`trigger: "manual"` — there is no dedicated `SyncTrigger` variant for a
replace, matching how `backfill_rockbox` is recorded.

---

## Daemon v1.6.0 — Playlist CRUD, per-device config, device preview (2026-07-18)

The daemon now emits `hello` with `protocol_version = "1.6.0"`. Purely
additive over v1.5.0: seven new commands, four new events, and a
deprecation note (no removed/changed fields) for `get_selection`/
`save_selection`/`custom_selection`. See `crates/classick/src/playlist.rs`,
`playlist_rules.rs`, `device_config.rs`, and `sync_set.rs` for the
host-side types these wire shapes mirror.

### New commands (UI → daemon)

| Type | Fields | Behavior |
|---|---|---|
| `list_playlists` | (none) | Replies `playlists_update`: every playlist in the store. |
| `get_playlist` | `slug` | Replies `playlist_detail`: that playlist's full content (track list or rule set), for the editor. `error` is set instead when the slug doesn't exist, the store can't be opened, or the on-disk file fails to parse. |
| `save_playlist` | `playlist` (a `PlaylistPayload`, below) | Create (absent `playlist.slug`) or replace (present `playlist.slug`) a playlist; persists atomically. No direct reply; broadcasts a fresh `playlists_update` to every client. |
| `delete_playlist` | `slug` | Delete a playlist by slug; no-op (still broadcasts) if the slug doesn't exist. No direct reply; broadcasts a fresh `playlists_update`. Deleting a playlist that's still subscribed on one or more devices does NOT touch those devices' `subscriptions.playlists` — the subscription is left dangling. It surfaces via `device_preview.unresolved_subscriptions` (below) and via a sync-time log line when the sync-set builder can't resolve it; it's never fatal to a sync. |
| `get_device_config` | `serial` | Replies `device_config_update` for that device: its resolved selection + subscriptions + settings. Never fails — an unknown `serial` resolves to each part's default. |
| `save_device_config` | `serial`, `selection?`, `subscriptions?`, `settings?` | Persists the provided parts (each field `None`/absent = "don't change", the same sentinel convention as `save_config`). No direct reply; broadcasts a fresh `device_config_update` to every client. If `serial` is the currently *configured* device, also broadcasts a refreshed `status_update` (a selection change may move "Y" in "X of Y synced"). |
| `preview_device` | `serial` | Pure computation over the cached library index + that device's selection/subscriptions/playlist-store state — no filesystem walk, nothing persists. Replies `device_preview`. |

`PlaylistPayload` (the `save_playlist` command's `playlist` field) is tagged
by `kind`:

```json
{"kind":"manual","slug":null,"name":"Gym","tracks":["Artist/Album/01.flac","B/02.flac"]}
{"kind":"smart","slug":"recent-idm","name":"Recent IDM","rules":{"version":1,"matching":"all","rules":[{"field":"genre","op":"is","value":"IDM"}],"limit":null,"order":"alpha","seed":0}}
```

`tracks` are source-relative paths (manual only); `rules` is a `SmartRules`
object exactly as `playlist_rules::SmartRules` serializes it (smart only).
An absent/`null` `slug` means "create a new playlist" — the daemon
allocates one via `PlaylistStore::unique_slug(name)`; a present `slug`
means "create-or-replace at exactly this slug" (the edit path). Track paths
aren't validated on save — `playlist::resolve_manual`'s existence/safety
check is the last line of defense at resolve time, by design (see its doc
comment), so an unsafe or nonexistent entry can round-trip through
`save_playlist` without failing the save; it's simply never resolved into a
sync set or a `playlists_update` size.

`selection`/`subscriptions`/`settings` on `save_device_config` (and
`device_config_update`, below) each mirror their on-disk type's meaningful
fields only — no `version` (a file-format implementation detail, not part
of the wire contract):

```json
{"mode":"include","rules":[{"kind":"artist","name":"Boards of Canada"}]}
{"playlists":["gym","chill"]}
{"auto_sync":true,"rockbox_compat":false}
```

### New events (daemon → UI)

| Type | Fields |
|---|---|
| `playlists_update` | `playlists[]` — `{slug, name, kind, tracks, bytes, error?}`. `kind` is `"manual"` \| `"smart"`. `tracks`/`bytes` are computed against the **cached library index** (never a walk): manual playlists resolve their source-relative tracks against the index (an entry the index doesn't know about is dropped, same "oracle" idea as `sync_set::compute` but index- instead of walk-backed); smart playlists evaluate their rules directly against the index. Sorted by `slug`. A playlist FILE the store failed to parse still surfaces here — named from its filename, `tracks`/`bytes` `0`, `error` set to the parse failure — instead of silently vanishing from the list. |
| `playlist_detail` | `slug`, `name?`, `kind?` (`"manual"` \| `"smart"`), `tracks?` (`string[]`, manual only), `rules?` (a `SmartRules` object, smart only), `error?`. Reply to `get_playlist`. On success `name`/`kind` are set together with the matching content field (`tracks` for manual, `rules` for smart) — unlike `playlists_update`'s summary, `tracks` here is the actual ordered path list, not a count. On failure (no playlist at `slug`, an unopenable store, or an on-disk file that fails to parse) `error` is set and `name`/`kind`/`tracks`/`rules` are all omitted. |
| `device_config_update` | `serial`, `selection` (`{mode, rules}`), `subscriptions` (`{playlists}`), `settings` (`{auto_sync, rockbox_compat}`). |
| `device_preview` | `selected_tracks`, `selected_bytes` (the scope-selection footprint, source-size estimate), `playlist_extra_tracks`, `playlist_extra_bytes` (subscribed-playlist members NOT already in the selection scope — the union's out-of-scope delta), `projected_free_bytes` (`u64 \| null`), `unresolved_subscriptions?` (`string[]`). `null` whenever the previewed `serial` isn't the device currently connected (no live `StorageInfo` to project from) — mirrors `library_update.scanned_at_unix_secs`'s "meaningful null" convention, not omission. When present, it's `current_free_bytes − (selected_bytes + playlist_extra_bytes)` — a conservative "as if syncing from empty" estimate, since this computation has no manifest/on-device knowledge of what's already synced. `unresolved_subscriptions` is the sorted set of this device's subscribed slugs that couldn't be resolved against the cached index (unknown slug, or a playlist-store load/open failure) — e.g. a subscription left dangling by `delete_playlist` (above). Those slugs contribute nothing to `playlist_extra_*`. The field is omitted from the wire entirely (not sent as `[]`) when every subscription resolved. See `daemon::library::compute_device_preview` for the exact math and its unit tests. |

`playlist_detail` example payloads:

```json
{"type":"playlist_detail","slug":"gym","name":"Gym","kind":"manual","tracks":["Artist/Album/01.flac","B/02.flac"]}
{"type":"playlist_detail","slug":"recent-idm","name":"Recent IDM","kind":"smart","rules":{"version":1,"matching":"all","rules":[{"field":"genre","op":"is","value":"IDM"}],"limit":null,"order":"alpha","seed":0}}
{"type":"playlist_detail","slug":"ghost","error":"no such playlist"}
```

`device_preview` example payloads — clean subscriptions vs. one dangling
(e.g. its playlist was deleted while still subscribed):

```json
{"type":"device_preview","selected_tracks":412,"selected_bytes":5123456789,"playlist_extra_tracks":3,"playlist_extra_bytes":90000000,"projected_free_bytes":1200000000}
{"type":"device_preview","selected_tracks":412,"selected_bytes":5123456789,"playlist_extra_tracks":0,"playlist_extra_bytes":0,"projected_free_bytes":null,"unresolved_subscriptions":["deleted-favorites"]}
```

### `get_selection`/`save_selection`/`custom_selection` deprecated in v1.6

As of v1.6.0, `get_selection`/`save_selection` (§"Daemon v1.4.0" above) read
and write the **configured** device's own per-device selection —
`selection::effective_device_selection_path(serial)`, seeded once from the
shared `selection.json` the first time it's resolved — rather than the
`custom_selection`-gated shared/per-device split
`selection::effective_selection_path` used to implement (§"`IpodIdentity`
gains `custom_selection`" above). Practically: `custom_selection`'s value no
longer changes which file these two commands touch. `get_selection` with no
device configured replies `mode: "all"`; `save_selection` in that state is
a no-op (logged, not persisted — there's no per-device path to resolve
without a serial). Both commands are kept only for UI clients that haven't
migrated to `get_device_config`/`save_device_config` with an explicit
`serial`, which is the supported way to read/write a *specific* device's
selection (not just "the" configured one) going forward. `custom_selection`
itself is unchanged on the wire (still rides `ipod` on `config_update`/
`save_config`) but is now vestigial: it no longer gates which selection
file `get_selection`/`save_selection` use.

Both singleton selection commands are removed in daemon protocol v2.0.0.
Clients must use the serial-keyed `get_device_config`, `save_device_config`,
and `preview_selection` commands described below.

## Daemon v1.7.0 — `resolve_tracks`: expand selection rules to track paths (2026-07-18)

The daemon now emits `hello` with `protocol_version = "1.7.0"`. Purely
additive over v1.6.0: one new command, one new event. Rationale: v1.6.0's
`library_update` is aggregate-only (artist/album/genre + counts, never
per-track paths — see `LibraryUpdate` in `crates/classick/src/
ipc_daemon.rs`), so a client that builds a rule-based selection (e.g. an
"Add Songs" picker offering artist/album/genre checkboxes) has no way to
turn that selection into the concrete track paths a manual playlist needs.
`resolve_tracks` closes that gap by doing the expansion host-side, against
the cached library index the daemon already maintains.

### New command (UI → daemon)

| Type | Fields | Behavior |
|---|---|---|
| `resolve_tracks` | `rules` (array of `SelectionRule`, below) | Pure computation over the cached library index; nothing persists. Replies `resolved_tracks`, sent synchronously inline — same reply-ordering contract as `preview_device` (§"Daemon v1.6.0" above): the daemon never spawns/awaits before sending this reply, so replies stay in request order for the client's FIFO correlation. |

`rules` reuses the **exact same wire shape** as `save_device_config`'s
`selection.rules` (`selection::SelectionRule`, tagged by `kind`) — no new
encoding:

```json
{"type":"resolve_tracks","rules":[
  {"kind":"artist","name":"Boards of Canada"},
  {"kind":"album","artist":"Aphex Twin","album":"Drukqs"},
  {"kind":"genre","name":"Ambient"}]}
```

An empty `rules` array is valid and resolves to zero tracks (there's
nothing to match).

### New event (daemon → UI)

| Type | Fields |
|---|---|
| `resolved_tracks` | `tracks` (`string[]`) — reply to `resolve_tracks`. |

```json
{"type":"resolved_tracks","tracks":["Artist/Album/01.flac","Artist/Album/02.flac"]}
```

`tracks` are source-relative paths (the same convention manual playlists'
`tracks` use — see `save_playlist`/`playlist_detail` in §"Daemon v1.6.0"
above), expanded against the cached library index with the SAME
case-insensitive matching the selection matcher uses
(`selection::Selection::wants`/`eq_fold`'s `to_lowercase` comparison — see
`crates/classick/src/selection.rs`). Internally this builds a throwaway
`Selection { mode: include, rules }` and reuses that exact matcher (see
`daemon::library::resolve_tracks`), so `resolve_tracks` always expands a
rule set to the same tracks a saved selection with those rules would keep.
A rule that matches nothing contributes nothing — never an error. Because
matching is a single OR-across-all-rules pass over the index rather than a
per-rule expand-then-union, a track matched by more than one rule (e.g. an
artist rule and a genre rule both matching the same file) appears exactly
once in `tracks`. Results are sorted lexicographically by path for
deterministic wire ordering. If the library index is absent (no source
configured yet) or not yet scanned, the reply is `{"tracks":[]}` — an empty
array is a valid reply, never an error.

## Daemon v2.0.0 — explicit device identity and request correlation (2026-07-18)

The daemon emits `hello` with `protocol_version = "2.0.0"`. This is a clean,
breaking pre-release wire revision. Version 1 payloads are not accepted as a
compatibility path: a missing required target or correlation field is a decode
error, not an instruction to guess the configured or connected device.

The daemon and UI continue to use newline-delimited JSON with the same
snake_case `type` discriminator. The breaking changes are:

- every request/reply command carries a required `request_id`;
- every device-specific command carries a required raw `serial` in addition to
  `request_id`;
- correlated replies carry `acknowledged_request_id`;
- device state is published as a serial-keyed inventory snapshot;
- forwarded subprocess events carry a required `session_id` and carry `serial`
  when the session belongs to a device;
- `get_selection` and `save_selection` are removed.

`subscribe_device_events`, `unsubscribe_device_events`, and `shutdown` remain
fieldless global commands. On-disk config, history, selection, settings, and
subscription migrations are independent of this wire break and remain intact.

### Commands (UI → daemon)

| Type | Required fields | Optional payload fields | Scope |
|---|---|---|---|
| `get_status` | `request_id` | — | Global |
| `get_config` | `request_id` | — | Global |
| `save_config` | `request_id` | `source`, `daemon`, `ipod` | Global |
| `get_history` | `request_id` | `limit` (defaults to 10) | Global; each returned history entry identifies its device |
| `get_library` | `request_id` | — | Global |
| `scan_library` | `request_id` | — | Global scan session |
| `retry_source_mount` | `allow_ui`, `request_id` | — | Global source recovery |
| `list_playlists` | `request_id` | — | Global |
| `get_playlist` | `slug`, `request_id` | — | Global |
| `save_playlist` | `playlist`, `request_id` | — | Global |
| `delete_playlist` | `slug`, `request_id` | — | Global |
| `resolve_tracks` | `rules`, `request_id` | — | Global |
| `forget_ipod` | `serial`, `request_id` | — | Device |
| `trigger_sync` | `source`, `serial`, `request_id` | — | Device |
| `cancel_sync` | `serial`, `request_id` | — | Active device sync |
| `pause` | `serial`, `request_id` | — | Active device sync |
| `decide_prompt` | `id`, `choice`, `serial`, `request_id` | — | Active device sync |
| `backfill_rockbox` | `serial`, `request_id` | — | Device |
| `replace_library` | `serial`, `request_id` | — | Device |
| `preview_selection` | `mode`, `rules`, `serial`, `request_id` | — | Device |
| `get_device_config` | `serial`, `request_id` | — | Device |
| `save_device_config` | `serial`, `request_id` | `selection`, `subscriptions`, `settings` (absent means “do not change”) | Device |
| `preview_device` | `serial`, `request_id` | — | Device |
| `subscribe_device_events` | — | — | Global handshake |
| `unsubscribe_device_events` | — | — | Global handshake |
| `shutdown` | — | — | Global |

Examples:

```json
{"type":"trigger_sync","source":"manual","serial":"RAW-A","request_id":"req-sync"}
{"type":"get_history","limit":50,"request_id":"req-history"}
{"type":"preview_selection","mode":"include","rules":[],"serial":"RAW-A","request_id":"req-preview"}
{"type":"retry_source_mount","allow_ui":true,"request_id":"req-source"}
```

The following v1 payload is invalid in v2 because it has neither a target nor
correlation id:

```json
{"type":"trigger_sync","source":"manual"}
```

### Events (daemon → UI)

Fields described as required must be present even when their value is zero or
an empty collection. Optional fields remain optional only when absence is a
real domain state.

| Type | Required fields | Domain-optional fields |
|---|---|---|
| `config_update` | `source`, `daemon`, `ipod`, `config_revision` | `acknowledged_request_id` (absent for an unsolicited broadcast) |
| `history_update` | `entries`, `acknowledged_request_id` | —; there is no top-level `serial` |
| `sync_rejected` | `reason`, `serial`, `acknowledged_request_id` | — |
| `sync_event` | `line`, `session_id`; `serial` for every device session | `serial` is omitted only for a global scan session |
| `device_inventory_snapshot` | `revision`, `devices` | Snapshot fields listed below |
| `selection_preview` | `selected_tracks`, `selected_bytes`, `adds`, `removes`, `serial`, `acknowledged_request_id` | — |
| `playlist_detail` | existing detail fields, `acknowledged_request_id` | content/error fields retain their documented result semantics |
| `device_config_update` | `serial`, `selection`, `subscriptions`, `settings`, `acknowledged_request_id` | — |
| `device_preview` | `serial`, existing preview fields, `acknowledged_request_id` | `projected_free_bytes` may be `null`; `unresolved_subscriptions` is omitted when empty |
| `resolved_tracks` | `tracks`, `acknowledged_request_id` | — |
| `source_availability` | `state`; `source_root` when `state` is `available` | `acknowledged_request_id` (terminal reply to an explicit retry only) |

`status_update`, `library_update`, `selection_update`, and `playlists_update`
may be either replies or broadcasts, so their `acknowledged_request_id` is
optional. `HistoryEntry.serial` is required on the v2 wire;
`HistoryEntry.session_id` remains optional because migrated historical records
may predate session attribution.

`config_revision` is monotonic for the lifetime of one daemon process. It
advances only after a content-changing config write succeeds; reads, failed
writes, and no-op saves retain the current revision. A new `hello` starts a new
connection epoch, so clients discard revision ordering from the prior daemon
process rather than comparing across restarts.

### Source availability and recovery

`source_availability.state` is exactly one of `available`, `remounting`,
`auth_required`, or `unavailable`. `source_root` is state-dependent: it is a
required string for `available` and is omitted (not `null`) for every other
state. The payload never contains an SMB URL, credentials, native mount error,
or backend diagnostic.

Automatic recovery attempts always suppress platform UI and publish
uncorrelated lifecycle events. Authentication UI is permitted only after the
client sends `retry_source_mount` with the required `allow_ui` field set to
`true`; omission is a decode error and `false` retains suppress-UI behavior.
When an `allow_ui: true` retry coalesces behind an automatic suppress-UI
attempt, an `auth_required` result starts exactly one UI-authorized attempt;
the daemon retains every coalesced request id until that attempt reaches a
terminal state. A non-authentication terminal result does not escalate.
`remounting` is an uncorrelated lifecycle broadcast. The terminal
`available`, `auth_required`, or `unavailable` event for an explicit retry
carries that command's `request_id` as `acknowledged_request_id`. If several
explicit retries coalesce behind one physical mount attempt, the daemon emits
one terminal correlated event per request id.

After `hello`, every newly connected client receives the current status,
device inventory, and one uncorrelated `source_availability` snapshot when a
source is configured. This replay includes terminal startup failures, so a UI
that opens after an automatic recovery attempt still renders the correct
recovery action. These three initial events are scoped to that connection;
existing clients do not receive another client's replay.

```json
{"type":"source_availability","state":"remounting"}
{"type":"source_availability","state":"auth_required","acknowledged_request_id":"req-source"}
{"type":"source_availability","state":"available","source_root":"/Volumes/data-1/media/music","acknowledged_request_id":"req-source"}
```

### Device inventory snapshot

```json
{"type":"device_inventory_snapshot","revision":9,"devices":[{"identity":{"serial":"RAW-A","model_label":"iPod Classic","name":"A"},"configured":true,"connected":true,"mount":"/Volumes/A","phase":"syncing","session_id":42,"storage":{"total_bytes":160000000000,"free_bytes":100000000000},"synced_count":12,"library_count":20,"latest_successful_sync":null,"latest_attempt":null,"last_terminal_error":null,"selection_revision":3,"settings_revision":4,"subscriptions_revision":5}]}
```

Each device contains:

- required `identity.serial` and `identity.model_label`; optional
  `identity.name`;
- required `configured`, `connected`, and `phase` (`disconnected`,
  `unconfigured`, `idle`, `syncing`, `paused`, or `error`);
- optional `mount`, `session_id`, `storage`, `library_count`,
  `latest_successful_sync`, `latest_attempt`, and `last_terminal_error`;
- required `synced_count`, `selection_revision`, `settings_revision`, and
  `subscriptions_revision`.

Raw serial spelling is preserved on the wire. Canonicalization is an internal
lookup concern and must not rewrite the identity shown to clients.

### Correlated event examples

```json
{"type":"sync_event","line":"{\"type\":\"track_done\"}","serial":"RAW-A","session_id":42}
{"type":"sync_rejected","reason":"already_syncing","serial":"RAW-A","acknowledged_request_id":"req-sync"}
{"type":"device_preview","serial":"RAW-A","selected_tracks":412,"selected_bytes":5123456789,"playlist_extra_tracks":3,"playlist_extra_bytes":90000000,"projected_free_bytes":1200000000,"acknowledged_request_id":"req-preview"}
```
