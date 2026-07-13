# ipod-sync IPC protocol v1.1.0

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

Protocol version follows **semver**. The current version is **`1.1.0`**.

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
| `track_start` | `current`, `total`, `label`                                                                  | Begin a per-track operation               |
| `track_done`  | (none)                                                                                       | Increment completed count                 |
| `log`         | `message`                                                                                    | Informational log line                    |
| `error`       | `message`, `recovery_hints?`                                                                 | Non-fatal or fatal error                  |
| `finish`      | `success`                                                                                    | Run complete; core will close stdout      |
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
| `protocol_version` | `string` | Semver of the wire protocol the core speaks. Currently `1.1.0`. |
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

> **Known M1 limitation.** The current `ProgressEvent::Summary` variant
> does not carry `metadata_only`; the IPC backend serializes `0` for it.
> The `review` event below carries the accurate count via its nested
> `summary` object, which is what the UI should use for action-plan
> rendering. Widening `ProgressEvent::Summary` is a post-M1 follow-up.

```json
{"type":"summary","add":12,"modify":3,"metadata_only":0,"remove":0,"unchanged":1260,"total_planned":15}
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
{"type":"review","summary":{"add":12,"modify":3,"metadata_only":0,"remove":0,"unchanged":1260},"no_delete":false}
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

| Field     | Type     | Notes                                              |
|-----------|----------|----------------------------------------------------|
| `current` | `number` | 1-based index of this track within the run.        |
| `total`   | `number` | Total tracks planned (matches `summary.total_planned`). |
| `label`   | `string` | Human-readable label, e.g. `"Aphex Twin - Selected Ambient Works II - #ATC1"`. |

```json
{"type":"track_start","current":1,"total":15,"label":"Aphex Twin - Selected Ambient Works II - #ATC1"}
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

| Field     | Type      | Notes                                                |
|-----------|-----------|------------------------------------------------------|
| `success` | `boolean` | `true` for a clean run, `false` for fatal failure.   |

```json
{"type":"finish","success":true}
```

```json
{"type":"finish","success":false}
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
| 1.1.0    | 0.1.x        | 0.1.x      | Current    | Additive: `pause` command (§5.6) + terminal `paused` event (§4.12). Handshake still requires major version `1` on both sides. |

Bumps will append rows here. Don't edit historical rows.

---

## v1.1.0 — Daemon extensions (UI ↔ daemon channel)

> **Namespace note:** the daemon-pipe protocol versions independently of the
> subprocess stdio protocol documented in §§1–11 above — they share the
> `major.minor.patch` scheme and both currently sit on major `1`, but their
> minor versions move on separate schedules and a matching number (e.g. both
> once being `1.1.0`) is coincidence, not a shared release train. As of this
> writing the daemon protocol is at **`1.2.0`** — see "Daemon v1.2.0" below;
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
