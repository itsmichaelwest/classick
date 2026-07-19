# Sync subprocess protocol 1.4.0

The desktop-owned daemon spawns `classick --ipc-mode` with stdin and stdout
piped. The subprocess emits events on stdout and receives commands on stdin.
Logs go to the Classick log directory; any non-JSON stdout corrupts this wire.

## Handshake

The first event is:

```json
{"type":"hello","protocol_version":"1.4.0","core_version":"0.0.1"}
```

The owner validates major version `1` before reducing later events or sending
commands.

## Events: core to owner

Optional fields are marked `?` and are omitted rather than serialized as null.

| `type` | Fields | Meaning |
| --- | --- | --- |
| `hello` | `protocol_version`, `core_version` | required first message |
| `header` | `source`, `ipod`, `manifest` | resolved run paths |
| `summary` | `add`, `modify`, `metadata_only`, `remove`, `unchanged`, `total_planned` | action-plan counts; `total_planned = add + modify + metadata_only + remove` |
| `review` | `summary`, `no_delete` | waits for `review_decision` |
| `prompt` | `id`, `message`, `options` | waits for matching `prompt_decision` |
| `form` | `id`, `label`, `initial`, `hint` | waits for matching `form_decision` |
| `track_start` | `current`, `total`, `label`, `eta_secs?` | one admitted track starts; ETA is whole-run-average seconds |
| `track_done` | `result` | admitted track ended; result is `applied` or `skipped` |
| `finalizing` | `reason`, `staged_albums`, `staged_tracks` | admission stopped; coordinated publication is still running |
| `cancelled` | none | graceful cancellation publication completed |
| `paused` | none | graceful pause publication completed |
| `log` | `message` | informational text |
| `error` | `message`, `recovery_hints?` | non-fatal or fatal diagnostic; hints omitted when empty |
| `finish` | `success`, `skipped_for_space?`, `artwork?`, `db_restored?` | terminal summary; stdout closes shortly afterward |

`review.summary` contains `add`, `modify`, `metadata_only`, `remove`, and
`unchanged`.

`finalizing.reason` is `cancelled` or `paused`.

`finish.skipped_for_space`, when present, contains `albums`, `tracks`, and
`bytes`. `finish.artwork`, when present, contains `embedded`, `eligible`, and
`failed_sources`. `db_restored` is emitted only when true.

Example terminal success:

```json
{"type":"finish","success":true,"artwork":{"embedded":18,"eligible":20,"failed_sources":2}}
```

Example failed run:

```json
{"type":"error","message":"could not publish the device database"}
{"type":"finish","success":false}
```

## Commands: owner to core

| `type` | Fields | Meaning |
| --- | --- | --- |
| `start` | none | reserved; currently ignored |
| `review_decision` | `decision` | answer a `review` |
| `prompt_decision` | `id`, `choice` | zero-based option for the matching prompt |
| `form_decision` | `id`, `value` | string answer; null means abort |
| `cancel` | none | stop admission at an album boundary, publish, and exit cancelled |
| `pause` | none | stop admission at an album boundary, publish, and exit paused |

`review_decision.decision` is a nested tagged object:

```json
{"type":"review_decision","decision":{"type":"apply","no_delete":false}}
{"type":"review_decision","decision":{"type":"dry_run"}}
{"type":"review_decision","decision":{"type":"quit"}}
```

Prompt and form IDs must echo the originating event. Unknown or stale IDs do
not authorize a different prompt.

## Lifecycle

### Normal success

1. Spawn with piped stdin/stdout.
2. Receive and validate `hello`.
3. Reduce progress and answer any blocking review/prompt/form.
4. Receive `finish { success: true }`.
5. Drain trailing stdout to EOF.
6. Process exits `0`.

### Failure

A fatal failure emits an `error`, then `finish { success: false }`, closes
stdout, and exits non-zero. EOF without a terminal event is a crash.

### Cancel

The owner writes exactly one `cancel` and keeps both pipes open. The expected
terminal order is:

1. `finalizing { reason: "cancelled", ... }`
2. zero or more continuing progress/log events
3. `cancelled`
4. `finish { success: true, ... }`
5. EOF and exit `0`

### Pause

Pause uses the same drain, with `reason: "paused"`, then `paused`, trailing
`finish { success: true, ... }`, EOF, and exit `0`.

The owner uses a progress-reset inactivity watchdog during finalization. It may
terminate a genuinely stalled owned child after the watchdog expires, but must
not impose a fixed total finalization duration.

## Forwarding through the daemon

The daemon wraps each raw subprocess event without rewriting it:

```json
{"type":"sync_event","line":"{\"type\":\"track_done\",\"result\":\"applied\"}","serial":"ABC123","session_id":42}
```

The outer serial and session ID are authoritative routing context. Clients must
validate them before applying the inner progress event.
