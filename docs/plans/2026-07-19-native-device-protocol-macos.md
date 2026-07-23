# Native device protocol — macOS UI plan

**Status:** implemented

**Depends on:** [Rust core plan](2026-07-19-native-device-protocol-rust.md),
protocol 3 golden vectors

## 1. Outcome

The macOS app already has a `DeviceSerial`-keyed inventory and per-device
pages. This work tightens that architecture around canonical `DeviceId`, adds
readiness/hardware/delivery state, and replaces the nested v1/v2 wire models
with protocol 3. The daemon remains the authority for filesystem and hardware
interpretation.

## 2. Phase M1 — replace the wire layer

### Files

- consolidate `Ipc/DaemonCommand.swift`, `DaemonEvent.swift`, and
  `SyncEvent.swift` into a v3 message model;
- update `Ipc/DaemonClient.swift` handshake and stream handling;
- update `WireCodecTests.swift` and `DaemonClientTests.swift` from the shared
  golden vectors.

### Work

- Validate protocol major, daemon role, and capabilities before yielding an
  operational connection.
- Decode typed progress as ordinary v3 events; remove `sync_event.line` and
  the inner `SyncEvent` decoder.
- Introduce a validated `DeviceID` value that encodes as the canonical string.
- Require request/device/session/prompt routing from the frozen contract.
- Decode unknown additive events without losing the connection or mutating a
  device; reject malformed required fields and wrong-role traffic.
- Expose an explicit incompatible-core connection state for the app delegate
  and views.

### Tests

- consume every shared golden and negative vector;
- reject v1/v2, wrong role, invalid ID, and misrouted progress;
- preserve socket line order and reconnect handshake ordering;
- prove nested JSON decoding no longer exists.

## 3. Phase M2 — migrate the device model

Rename `DeviceSerial` to `DeviceID` through:

- `DeviceViewState` and `DeviceReducer`;
- `AppModel` dictionaries, focus, pending intents, history, and notifications;
- sidebar destinations, drop targets, pages, setup, and device actions.

Do not perform a blind string rename. Construct/validate IDs at the wire edge,
then use the value type as dictionary key. Mount path stays a replaceable
connection attribute.

Keep identity-unavailable candidates in a separate
`[ObservationID: UnidentifiedDeviceViewState]` read-only collection. The
connection-scoped observation ID is not persisted, cannot be placed in a
device destination, and cannot form a command. If identity later becomes
available, normal inventory reduction adds the `DeviceID` entry and removes
the observation.

Extend `DeviceViewState` with:

- readiness;
- hardware facts and provenance;
- profile/adoption status;
- config delivery state per component/mutation;
- the existing connection, config, preview, session, storage, history, and
  progress state.

Reducer rules:

- inventory snapshots are canonical for connection/readiness/hardware;
- correlated config events are canonical for accepted/delivered config;
- a pending local draft survives unrelated snapshots and older device state;
- progress reduces only when both device and session match;
- reconnect with a new mount keeps the same device state;
- unknown phases/events do not collapse the whole device to idle.

### Tests

- update `DeviceInventoryReducerTests`, `AppModelReducerTests`, acknowledged
  draft tests, and notification tests;
- interleave two devices and two sessions;
- cover stale reconnect snapshots and pending-host precedence;
- prove no mount path or display label is used as a dictionary key.

## 4. Phase M3 — readiness and honest hardware presentation

Update `DeviceRowPresentation`, `DeviceIdentityLogic`, `DeviceIcon`, sidebar,
menu content, setup window, music page, and settings page.

Rules:

- title prefers the Apple-owned iPod name;
- exact artwork requires a deterministic daemon-decoded model/colour fact;
- otherwise select a generic family/model illustration;
- do not derive colour in Swift, use storage as presentation identity, or add
  an appearance setting;
- accessibility text states the known family/model without relying on artwork.

Readiness behavior:

- `ready`: normal Classick adoption/config/sync subject to other gates;
- `needs_apple_initialization`: visible with Finder setup guidance, no Classick
  setup or sync button;
- `invalid_database`: visible recovery guidance, no ordinary sync;
- `identity_unavailable`: visible diagnostic state, no mutation.

The guidance must say Finder/Apple software initializes the iPod; it must not
imply Classick can initialize it. Do not surface SCSI/elevation as a normal
remedy.

Update previews and logic tests for exact and generic art, every readiness
state, long names, VoiceOver labels, and disconnected remembered devices.

## 5. Phase M4 — disconnected edits and delivery state

The existing `AcknowledgedDraft`/request-correlation pattern is the base.
Extend it so every device-config component distinguishes:

- local draft not yet accepted;
- accepted on this Mac and `pending_device`;
- `device_committed`;
- host acceptance failure;
- device delivery failure with the accepted host value retained.

On `DeviceMusicPage` and `DeviceSettingsPage`:

1. keep selection/subscriptions/auto-sync/Rockbox editable while disconnected;
2. send request and mutation IDs;
3. clear the submission only on its correlated config acknowledgement;
4. display a compact “Waiting for iPod” state for pending device delivery;
5. remove it when the exact mutation is reported committed;
6. prevent imported stale device state from overwriting a pending draft.

Update `SetupWindow`/`AppDelegate.finishSetup` so the initial auto-sync choice
creates the selected device's setting through the same mutation path. It must
not remain a global daemon toggle or reset another remembered iPod.

Auto-sync off must render immediately after host acceptance. The daemon—not
Swift—guarantees it is consulted before connection-triggered admission.

Keep the source library global. Do not implement portable library comparison or
the different-library replacement prompt. Existing explicit replacement UI
must remain clearly user-initiated.

### Tests

- extend `AcknowledgedDraftTests`, `DeviceSettingsLogicTests`, and
  `DeviceMusicLogicTests` for accepted-versus-delivered states;
- disconnected edit survives navigation and reconnect;
- unrelated acknowledgements cannot clear the draft;
- two devices cannot share pending state;
- delivery failure does not visually revert the accepted host setting;
- no appearance field is sent or cached as device config.

## 6. Phase M5 — typed progress and multi-device interaction

Update `AppModel+DeviceReducer`, prompt presentation, `Notifier`, menu content,
and device pages to consume v3 progress directly.

- Route every progress, prompt, decision, pause, cancel, and terminal event by
  `DeviceID` plus session ID.
- Keep one prompt associated with its owning device/session; changing focused
  device must not retarget the answer.
- Preserve finalization and EOF drain behavior.
- When more than one device is active, show aggregate activity in the menu-bar
  label and explicit per-device progress in the sidebar/page. Do not merge
  counts or guess a focus.
- Notification identifiers may include the canonical ID for uniqueness, but
  notification copy must use the iPod name/model and not display the raw ID.
- Eject uses the current inventory mount for the selected connected device.

### Tests

- stale/wrong-session progress ignored;
- prompt answer retains original device/session after navigation;
- concurrent sessions have independent progress and notifications;
- terminal state persists until canonical inventory/history catches up;
- disconnect during finalization does not offer unsafe actions.

## 7. Project and release plumbing

- Adding/removing Swift files requires `xcodegen generate`; require a clean
  regeneration diff after committed project changes.
- Keep wire/reducer/presentation logic `Sendable` and unit-testable under
  SwiftPM; keep AppKit operations in the app layer and on the main actor.
- Do not add App Sandbox entitlements; the daemon/device/socket architecture is
  unchanged.
- Update preview fixtures to use redacted canonical IDs and cover initialized,
  uninitialized, exact-hardware, generic-hardware, disconnected-pending, and
  multi-sync states.
- Update README/protocol references from daemon v2 to unified v3 only at the
  coordinated switch.

## 8. macOS completion gate

```text
cd ui/macos && swift test
cd ui/macos && xcodegen generate
git diff --exit-code -- ui/macos/Classick.xcodeproj
cargo build --release
ui/macos/bundle.sh
```

Also build the app through the release Xcode configuration used by packaging.

Manual checks:

- an old daemon yields an incompatibility surface before commands are enabled;
- factory-restored and Finder-initialized devices render distinctly;
- two remembered iPods keep independent configuration and pending delivery;
- disabling auto-sync disconnected prevents plug-in sync;
- exact/generic artwork follows daemon facts without appearance preferences;
- typed progress, prompt, pause/cancel, notifications, and eject stay on the
  correct device;
- Finder/Apple Music can manage the post-Classick iPod and firmware playback
  and artwork remain correct.
