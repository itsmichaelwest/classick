# Classick IPC protocols

Classick has two independent newline-delimited JSON protocols. This document is
the normative entry point; the linked references define every current message.

| Channel | Current version | Transport | Schema |
| --- | --- | --- | --- |
| sync subprocess | `1.4.0` | child stdin/stdout | [Subprocess protocol](ipc/subprocess.md) |
| desktop UI ↔ daemon | `2.0.0` | Windows named pipe or Unix socket | [Daemon protocol](ipc/daemon.md) |

The version numbers are not a shared release train. A daemon `sync_event`
contains one raw subprocess-protocol line and therefore carries both protocol
contexts.

## Common framing

- UTF-8 JSON, exactly one object per line, terminated by `\n`.
- The top-level `type` discriminator and all field names use `snake_case`.
- The producer flushes every line.
- `hello` is the first message from the producer. No other event may overtake
  it.
- Protocol versions use semantic versioning. A major mismatch is incompatible;
  minor and patch changes are backward-compatible additions or clarifications.
- Consumers ignore unknown event types and unknown object fields after a valid
  same-major handshake. Required fields on known messages remain required.
- Malformed JSON is logged/rejected without being reflected back into the
  stream. A consumer must not write diagnostics to the subprocess stdout
  channel.

## Correlation and ordering

Subprocess prompts correlate by numeric `id`. Daemon mutations and queries
correlate by string `request_id`; canonical replies expose it as
`acknowledged_request_id`.

An acknowledgement proves that the reply's canonical state was durably
persisted for that exact request. An uncorrelated broadcast, socket write, echo,
or locally predicted state is not an acknowledgement. A partial failure may
broadcast the actual canonical state with no acknowledgement and then send a
correlated failure.

Each connection preserves input-line order. Clients that persist unsent or
unacknowledged intents replay the same encoded bytes and request ID after
reconnection. Additive mutations are idempotent by request ID and payload
fingerprint.

## Compatibility sources

The Rust serialized enums are the implementation authority:

- `crates/classick/src/ipc.rs`
- `crates/classick/src/ipc_daemon.rs`

The macOS and Windows models must accept every valid current Rust message and
must encode commands with the same field names and defaults. Wire codec tests
contain representative doc-shaped payloads.

The previous append-only protocol narrative is retained only as
[history](archive/ipc-protocol-history.md). It is not a current contract.
