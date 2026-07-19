# Daemon protocol 2.0.0

Desktop applications connect to the daemon over `\\.\pipe\classick` on
Windows or the platform Unix-socket path returned by
`default_pipe_name()` on macOS/Linux. The daemon owns device state and may have
multiple clients.

## Handshake and subscriptions

The daemon sends `hello` first:

```json
{"type":"hello","protocol_version":"2.0.0","core_version":"0.0.1"}
```

Clients accept compatible major version `2`. `subscribe_device_events` and
`unsubscribe_device_events` have no request ID. Authoritative inventory
snapshots coexist with device-connected/disconnected transition broadcasts.

Every other query or mutation command carries a stable string `request_id`
unless the command table says otherwise.

## Commands: UI to daemon

`serial` is the raw device serial. `rules` uses the shared selection-rule shape
defined below.

| `type` | Fields | Canonical response or outcome |
| --- | --- | --- |
| `get_status` | `request_id` | correlated `status_update` |
| `get_config` | `request_id` | correlated `config_update` |
| `save_config` | `source?`, `daemon?`, `ipod?`, `request_id` | correlated `config_update`; partial failure broadcasts uncorrelated actual state plus `command_failed` |
| `forget_ipod` | `serial`, `request_id` | inventory/config/status broadcasts or `command_failed` |
| `trigger_sync` | `source`, `serial`, `request_id` | session events; `sync_rejected` on admission failure |
| `get_history` | `limit?`, `request_id` | `history_update`; default limit 10 |
| `subscribe_device_events` | none | enables transition delivery |
| `unsubscribe_device_events` | none | disables transition delivery |
| `cancel_sync` | `serial`, `request_id` | session finalization or `sync_rejected`; no separate success ack |
| `pause` | `serial`, `request_id` | session finalization or `sync_rejected`; no separate success ack |
| `decide_prompt` | `id`, `choice`, `serial`, `request_id` | forwarded to active session; rejection is correlated |
| `backfill_rockbox` | `serial`, `request_id` | session events or `sync_rejected` |
| `replace_library` | `serial`, `request_id` | session events or `sync_rejected` |
| `get_library` | `request_id` | correlated `library_update` |
| `scan_library` | `request_id` | scan/status/library events; no direct success ack |
| `retry_source_mount` | `allow_ui`, `request_id` | terminal correlated `source_availability` |
| `preview_selection` | `mode`, `rules`, `serial`, `request_id` | `selection_preview` |
| `list_playlists` | `request_id` | correlated `playlists_update` |
| `get_playlist` | `slug`, `request_id` | `playlist_detail` |
| `save_playlist` | `playlist`, `request_id` | correlated `playlists_update` or `command_failed` |
| `delete_playlist` | `slug`, `request_id` | correlated `playlists_update` plus changed device configs, or `command_failed` |
| `get_device_config` | `serial`, `request_id` | correlated `device_config_update` |
| `save_device_config` | `serial`, `selection?`, `subscriptions?`, `settings?`, `request_id` | correlated update only if every requested component persists; otherwise uncorrelated actual state plus `command_failed` |
| `preview_device` | `serial`, `request_id` | `device_preview` |
| `resolve_tracks` | `rules`, `request_id` | `resolved_tracks` |
| `add_selection_to_device` | `request_id`, `serial`, `rules` | `device_selection_added` or `library_mutation_rejected` |
| `append_selection_to_playlist` | `request_id`, `slug`, `rules` | `playlist_selection_appended` or `library_mutation_rejected` |
| `shutdown` | none | daemon enters the shared drain and closes the connection |

`trigger_sync.source` is `manual`, `scheduled`, or `plug_in`. Session admission
rejections are `already_syncing`, `no_ipod`, or `not_configured`.
`too_many_failures` remains an enum-reserved legacy value; current production
orchestration reports that condition as a terminal aborted session, not a
`sync_rejected` admission response.

## Events: daemon to UI

Optional means omitted. Nullable means the JSON key is present and may be null.

| `type` | Required fields | Optional/nullable fields |
| --- | --- | --- |
| `hello` | `protocol_version`, `core_version` | — |
| `status_update` | `state`, `configured`, `ipod_connected`, `synced_count` | optional `last_sync`, `next_scheduled_unix_secs`, `storage`, `library_count`, `acknowledged_request_id` |
| `config_update` | `source` (nullable), `daemon` (nullable), `ipod` (nullable), `config_revision` | optional `acknowledged_request_id` |
| `history_update` | `entries`, `acknowledged_request_id` | — |
| `device_connected` | `serial`, `model_label`, `drive` | optional `name` |
| `device_disconnected` | `serial` | — |
| `device_inventory_snapshot` | `revision`, `devices` | — |
| `sync_rejected` | `reason`, `serial`, `acknowledged_request_id` | — |
| `command_failed` | `acknowledged_request_id`, `error` | — |
| `sync_event` | `line`, `session_id` | optional `serial` |
| `library_update` | `source_root` (nullable), `scanned_at_unix_secs` (nullable), `artists`, `genres`, `total_tracks`, `total_bytes` | optional `acknowledged_request_id` |
| `selection_update` | `mode`, `rules` | optional `serial`, `acknowledged_request_id`; retained for compatibility, not currently produced by normal v2 runtime paths |
| `selection_preview` | `selected_tracks`, `selected_bytes`, `adds`, `removes`, `serial`, `acknowledged_request_id` | — |
| `playlists_update` | `playlists`, `playlist_revision` | optional `acknowledged_request_id` |
| `playlist_detail` | `slug`, `playlist_revision`, `acknowledged_request_id` | optional `name`, `kind`, `tracks`, `rules`, `error` |
| `device_config_update` | `serial`, `selection`, `subscriptions`, `settings`, `selection_revision`, `settings_revision`, `subscriptions_revision` | optional `acknowledged_request_id` |
| `device_preview` | `serial`, `selected_tracks`, `selected_bytes`, `playlist_extra_tracks`, `playlist_extra_bytes`, `projected_free_bytes` (nullable), `acknowledged_request_id` | optional `unresolved_subscriptions` (omitted when empty) |
| `resolved_tracks` | `tracks`, `acknowledged_request_id` | — |
| `source_availability` | `state` | optional `source_root`, `acknowledged_request_id` |
| `device_selection_added` | `acknowledged_request_id`, `serial`, `matched_tracks`, `missing_tracks`, `selection_changed`, `selection_revision`, `selection`, `delivery` | — |
| `playlist_selection_appended` | `acknowledged_request_id`, `slug`, `appended_tracks`, `playlist_revision`, `playlist` | — |
| `library_mutation_rejected` | `acknowledged_request_id`, `target`, `code`, `message` | — |

`status_update.state` is `idle`, `syncing`, or `scanning`. Clients map unknown
additive states to idle rather than dropping the entire event.

`source_availability.state` is `available`, `remounting`, `auth_required`, or
`unavailable`. `source_root` is present only when available; backend diagnostics
and credentials never cross the wire.

## Shared payloads

### Selection

`mode` is `all`, `include`, or `exclude`. Rules are tagged objects:

```json
{"kind":"artist","name":"Birdy"}
{"kind":"album","artist":"Birdy","album":"Fire Within"}
{"kind":"genre","name":"Pop"}
```

Device selection payloads contain `mode` and `rules`. Subscriptions contain
`playlists` (stable slugs). Device settings contain `auto_sync` and
`rockbox_compat`.

### Global daemon settings

`DaemonSettings` fields are:

| Field | Type/default |
| --- | --- |
| `enabled` | boolean, `true` |
| `autostart_with_windows` | boolean, `false` |
| `first_sync_mode` | `review` or `auto_apply`; default `review` |
| `subsequent_sync_mode` | `review` or `auto_apply`; default `auto_apply` |
| `schedule_minutes` | unsigned integer, `30` |
| `notify_on` | `all`, `errors_only`, or `none`; default `all` |
| `rockbox_compat` | boolean, `false` |
| `drop_sync_behavior` | `immediate` or `next_sync`; default `immediate` |

`IpodIdentity` contains `serial`, `model_label`, optional `name`, and
`custom_selection` (default false).

### Playlists

`save_playlist.playlist` is tagged by `kind`:

- manual: optional `slug`, `name`, and ordered source-relative `tracks`
- smart: optional `slug`, `name`, and `rules` (`version`, `matching`, rule
  list, optional limit, order, seed)

An absent slug creates a new stable slug; a present slug replaces that exact
playlist. `playlist_detail` exposes only `tracks` for manual and only `rules`
for smart. On not-found/parse failure, content/name/kind are omitted and
`error` is present.

### Device inventory

Each `devices` entry contains:

- `identity`: `serial`, `model_label`, optional `name`
- `configured`, `connected`, optional `mount`
- `phase`: `disconnected`, `unconfigured`, `idle`, `syncing`, `paused`, or
  `error`
- optional `session_id` and `storage`
- `synced_count`, optional `library_count`
- optional `latest_successful_sync`, `latest_attempt`, `last_terminal_error`
- `selection_revision`, `settings_revision`, `subscriptions_revision`

Storage contains `total_bytes` and `free_bytes`.

### History

History entries contain serial, optional session ID, timestamp, duration,
trigger, outcome, optional error, optional summary, and optional true-only
`db_restored`. Triggers are `plug_in`, `scheduled`, `manual`, or `coalesced`;
outcomes are `ok`, `error`, `aborted`, or `cancelled`.

### Library-drop outcomes

`device_selection_added.delivery` is `added_and_syncing`,
`added_for_next_sync`, or `already_present`. A playlist append returns the
canonical complete manual playlist and appended count.

`library_mutation_rejected.target` is either
`{"kind":"device_selection","serial":"..."}` or
`{"kind":"manual_playlist","slug":"..."}`. A request ID replay with the
same fingerprint returns the original canonical acknowledgement without
reapplying; a different fingerprint is rejected.

## Revisions and durable acknowledgement

Config, playlist, device-selection, device-settings, subscriptions, and
inventory revisions are monotonic within their authority. Clients ignore stale
lower revisions. Same-revision replay may still carry the acknowledgement that
settles an outstanding request.

For a multi-file mutation, correlate the canonical update only after all
requested authorities persist. If some authority fails after another changed,
publish the actual state uncorrelated and send `command_failed` with the request
ID. This lets the UI update truth without clearing the durable intent as a
success.

## Logging and secrets

Daemon and subprocess modes write logs below the platform local-data directory
under `classick/logs`. IPC stdout contains JSON only. Logs, errors, config, and
wire payloads must not expose SMB credentials, authentication backend details,
or secret-bearing source URLs.
