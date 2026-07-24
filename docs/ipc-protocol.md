# Classick protocol 3

Protocol `3.0.0` is the single newline-delimited JSON contract used by desktop
clients and by daemon-owned sync workers. Windows named pipes, Unix sockets,
and worker stdin/stdout use the same message envelope and validation rules.

## Handshake and framing

- UTF-8 JSON, one object per line, terminated by `\n`; producers flush each
  line.
- `hello` is the first message. It carries `protocol_version`, `role`
  (`desktop`, `daemon`, or `worker`), `software_version`, and a sorted
  capability list.
- Peers reject a wrong major, role, missing required capability, a second
  hello, or a command/event not allowed on that admitted stream.
- Same-major unknown event types are ignored. Unknown commands, malformed
  known messages, invalid routing, and unknown fields in owned schemas are
  rejected.
- Current daemon capabilities are `device_inventory`, `portable_profile`, and
  `typed_sync_progress`.

The shared Rust envelope is `wire::WireMessage`. Language-neutral positive and
negative examples live under `crates/classick/tests/data/wire-v3/` and are
consumed unchanged by Rust, Swift, and C# tests.

Music-facing strings are full Unicode and are preserved as UTF-8 on the wire.
This includes artist, album, title, album artist, composer, genre, playlist
display names, and source-relative track paths. Source-relative paths must
still be safe relative paths (forward slashes, no drive prefix, and no empty,
`.` or `..` components), but are not ASCII-limited. ASCII-only validation is
reserved for Classick-owned identifiers and structural filenames such as
device IDs, UUIDs, playlist slugs, and portable-profile paths.

## Routing and correlation

- Every query and mutation carries a lowercase non-nil UUID `request_id`.
- Device commands carry a canonical 16-uppercase-hex `device_id`.
- Active sync events and controls additionally carry a nonzero `session_id`.
- Prompts carry a nonzero `prompt_id` scoped to that session.
- Portable configuration mutations carry their own lowercase non-nil UUID
  `mutation_id`.

An acknowledgement is the correlated canonical event for that exact request.
Socket write completion, an echo, an uncorrelated inventory/config broadcast,
or a locally predicted state is not an acknowledgement.

`history.request_id` is present on a direct `get_history` response and absent
on the authoritative history broadcast published after a terminal sync result.

Configuration acceptance and device delivery are separate:

1. the daemon persists the complete desired component to the host outbox;
2. `device_config` reports `pending_device`;
3. when connected, the daemon runs a config-only device transaction;
4. exact readback changes delivery to `device_committed` and clears only the
   matching mutation.

`config_mutation_failed.stage` distinguishes host acceptance from device
delivery. A delivery failure retains accepted host intent.

## Message families

The exhaustive schemas are the serialized enums in:

- `crates/classick/src/wire/command.rs`
- `crates/classick/src/wire/event.rs`
- the remaining focused models under `crates/classick/src/wire/`

The public families are:

- global source/settings and source availability;
- device inventory, readiness, adoption, portable configuration, preview, and
  forgetting;
- sync, replace-library, Rockbox backfill, pause/cancel, prompts, typed
  progress, and terminal results;
- library scan/query, selection resolution, and drag/drop mutations;
- playlist list/detail/save/delete/append;
- history and daemon shutdown.

Inventory uses `device_id` only when ordinary USB identity is available.
Identity-unavailable observations carry an ephemeral `observation_id` and have
no mutating commands. Mount paths are operational diagnostics, never identity.

## Portable device settings

The complete protocol-3 `SettingsValue` is:

```json
{
  "schema_version": 1,
  "auto_sync": true,
  "rockbox_compat": false,
  "transcode_profile": "alac"
}
```

`transcode_profile` is required. Its V1 values are:

| Wire value | Output for sources that require transcoding |
| --- | --- |
| `alac` | Apple Lossless in `.m4a` |
| `aac_256` | AAC-LC at 256 kbps in `.m4a` |
| `aac_192` | AAC-LC at 192 kbps in `.m4a` |
| `aac_128` | AAC-LC at 128 kbps in `.m4a` |

The default used when adopting a device is `alac`. The same four values are
valid for every supported device: clients and the daemon must not infer,
filter, disable, or rewrite them from model, generation, capacity, or
capability data.

Changing the value makes affected tracks eligible for replacement on the next
sync, including an ALAC source previously copied losslessly when the new target
is AAC. Compatible lossy source files that Classick normally copies without
transcoding remain passthrough to avoid a lossy-to-lossy conversion. The
host-local `encoder` choice is separate: it selects the ALAC backend (`ffmpeg`
or `refalac`) and does not restrict the portable profile.

Library album, genre, and playlist summaries carry both source `bytes` and
aggregate `duration_ms`. Device-facing clients project AAC size as
`duration_ms × bitrate / 8`, plus a two-percent allowance for the MP4
container and tags. Missing duration falls back to source bytes. The daemon
uses the same estimate for `device_preview` and projected free space; ALAC
continues to use source bytes as its quick estimate.

## Compatibility

Protocol compatibility is major-version based and there is no production
fallback from protocol 3 to the former daemon-2/subprocess-1 protocols. Both
desktop clients and the daemon ship the same major. A mismatch is surfaced as
an actionable incompatible-core error before a mutation command is sent.

The prior append-only contracts are retained only in
[protocol history](archive/ipc-protocol-history.md) and are not current
authority.
