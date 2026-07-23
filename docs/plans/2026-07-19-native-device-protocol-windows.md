# Native device protocol — Windows UI plan

**Status:** implemented; native Windows build remains a platform verification
gate

**Depends on:** [Rust core plan](2026-07-19-native-device-protocol-rust.md),
protocol 3 golden vectors

## 1. Outcome

The Windows app moves from its current configured-device/tray-centric state to
a `device_id`-keyed client model that can represent multiple remembered and
connected iPods. It remains a thin client: the Rust daemon owns discovery,
readiness, hardware decoding, reconciliation, and device writes.

The tray popover may focus one device at a time, but the settings window must
expose every remembered/connected device and each device's independent music
selection, subscriptions, settings, delivery state, and sync progress.

## 2. Phase W1 — replace the wire layer

### Files

- replace `Classick.UI.Core/Ipc/DaemonCommand.cs`, `DaemonEvent.cs`,
  `IpcCommand.cs`, and `IpcEvent.cs` with a single v3 message hierarchy;
- simplify `DaemonClient.cs`, `DaemonEventRouter.cs`, and
  `RoutedSyncEvent.cs` around the shared envelope;
- update `Classick.UI.Tests/IpcWireFormatTests.cs`, `DaemonClientTests.cs`, and
  `DaemonEventRouterTests.cs`.

### Work

- Validate `hello` major, daemon role, and required capabilities before the app
  sends any command.
- Decode typed progress directly; delete `SyncEventEnvelope.Line` and the
  second JSON deserialization pass.
- Model `DeviceId` as a validated value at decode/construction boundaries,
  while serializing the canonical string.
- Require request/device/session routing according to the frozen vectors.
- Preserve unknown additive event handling without mapping an unknown device
  state to a different known state.
- Surface an incompatible-core state with one actionable error; do not launch
  setup or send config when the handshake fails.

### Tests

- consume every shared golden and negative vector;
- reject old major, wrong role, invalid `device_id`, and misrouted progress;
- prove one reader preserves line order;
- prove no client code deserializes nested JSON.

## 3. Phase W2 — introduce a multi-device application store

`App.xaml.cs` currently owns singleton `LatestConfig`, `LatestStatus`,
`ConfiguredSerial`, `_latestInventory`, and one `PopoverViewModel`. Replace
that mixed state with a testable `DeviceStore` in `Classick.UI.Core` or a
link-testable ViewModels source:

```text
DeviceStore
  GlobalConfig                 source location and truly global preferences
  Devices[DeviceId]
    Identity                   name plus daemon-decoded hardware facts
    Readiness
    Connection/session state
    Config                     selection/subscriptions/settings/revisions
    Delivery                   accepted host mutation vs device committed
    Storage/history/progress
  Unidentified[ObservationId]  connection-scoped read-only diagnostic rows
```

`ObservationId` is never promoted to durable identity or accepted by a device
command. Remove its row when its connection generation disappears; if a later
snapshot supplies a valid `DeviceId`, treat that as a new identified entry.

`App.xaml.cs` should own lifecycle and dispatcher marshalling only. Move
inventory/config/progress reduction into the store so event ordering and
multi-device routing are unit-testable.

Choose popover focus deterministically:

1. active sync already in focus;
2. sole active sync;
3. last explicitly selected device if still known;
4. sole connected configured device;
5. no implicit choice when ambiguous.

Never fall back to the first inventory entry for a mutating command. Every
action captures its target `DeviceId` and, for active sync controls, session ID.

### Tests

- two devices reduce independently under interleaved inventory/config/progress;
- reconnect/mount change preserves the same entry;
- stale session progress cannot update a new session;
- forget/disconnect cannot retarget a pending action;
- focus is deterministic and ambiguous focus disables mutation.

## 4. Phase W3 — readiness and device presentation

Extend the device row/popup presentation model with:

- readiness (`ready`, needs Apple initialization, invalid DB, identity
  unavailable);
- family/generation/exact model/colour facts with provenance;
- profile/adoption status;
- connected/disconnected and sync phase.

Presentation rules:

- prefer the Apple-owned iPod name for the title;
- use exact model/colour artwork only when the daemon supplies a deterministic
  decoded fact;
- otherwise use generic family/model artwork;
- never offer an appearance preference or infer silver from missing colour;
- never present capacity as the model name.

For `needs_apple_initialization`, explain that Apple software must initialize
the iPod and disable setup/sync mutation. For invalid DB, show recovery
guidance without claiming Classick can initialize or repair it. For missing
identity, expose non-privileged diagnostic wording and do not suggest running
as administrator merely to sync.

Update `WizardDevicePage` and `WizardViewModel`: uninitialized candidates are
visible but not selectable for Classick adoption; ready candidates are keyed by
`DeviceId`. The first-run flow configures Classick for an Apple-initialized
device, not the device itself.

## 5. Phase W4 — per-device pages and disconnected editing

The current Settings view mostly edits global daemon configuration and an iPod
chooser. Add a device list and selected-device surface. Keep source library,
notifications, launch behavior, and app update controls global; move these to
the selected device:

- selection and subscribed playlists;
- auto-sync;
- Rockbox compatibility;
- device sync/replacement/remove actions;
- delivery and last-sync state.

Do not copy Finder/iTunes flags into these settings.

Remove the current global auto-sync workaround while doing this. In
`SettingsViewModel` and `WizardViewModel`, stop hard-coding
`DaemonSettings.Enabled = true` and stop using `subsequent_sync_mode` as the
on/off signal. Initial setup writes the selected device's `auto_sync`; apply
versus review remains a separate policy if it survives the v3 inventory.

Edits use an acknowledged draft per component:

1. optimistically retain the user's draft;
2. send a v3 mutation with request and mutation IDs;
3. clear the request only when the correlated canonical config update arrives;
4. show `Saved on this PC — waiting for iPod` for `pending_device`;
5. show ordinary saved state after `device_committed`;
6. retain the draft and an actionable error when host acceptance fails.

A disconnected device remains fully editable. Reconnecting must not briefly
render/import stale device values over a pending host draft. Auto-sync UI must
reflect the accepted host value immediately.

Do not implement the different-library prompt in this work. Existing manual
replacement remains explicit; portable library identity is a separate design.

### Tests

- disconnected setting edit becomes pending and survives window recreation;
- connected edit transitions accepted -> committed;
- out-of-order unrelated config events do not clear a draft;
- pending auto-sync off renders off before reconnect and prevents sync state;
- first-run “manual” produces per-device `auto_sync = false` rather than a
  global daemon value that still admits auto-sync;
- switching selected devices cannot leak drafts or subscriptions;
- no device appearance field is editable or persisted by the client.

## 6. Phase W5 — typed progress and tray behavior

Refactor `PopoverViewModel` and `App.xaml.cs` progress handling to consume v3
events directly. Preserve prompt routing, cancellation drain, finalization,
history, notifications, ETA, and terminal summaries.

The tray icon represents application-level activity; the popover clearly names
the focused device. When multiple devices sync, show an aggregate activity
state and let the user choose which device to inspect rather than merging track
counts. Notifications include the targeted device name and use
`device_id`/session for deduplication without displaying the raw ID.

Readiness states override the ordinary “Sync now” affordance. Eject remains a
mount operation on the currently targeted connected device, never on a cached
mount from another inventory generation.

## 7. Project/test plumbing

- Keep pure state, wire, and presentation logic out of the WinAppSDK assembly
  so the plain `net10.0` test project avoids module initialization.
- Update `Classick.UI.Tests.csproj` linked sources deliberately for any new
  link-tested ViewModels; do not make tests load the packaged app assembly.
- Add device artwork assets only for generic families and exact variants the
  daemon can actually report. Provide accessible text independent of colour or
  icon.
- Marshal all observable mutations onto `DispatcherQueue`; decode/reduction
  may remain pure off-thread.

## 8. Windows completion gate

```text
dotnet test Classick.UI.Tests/Classick.UI.Tests.csproj
dotnet build Classick.UI.slnx -c Debug -p:Platform=x64
dotnet build Classick.UI.slnx -c Release -p:Platform=x64
dotnet build Classick.UI.slnx -c Release -p:Platform=ARM64
```

Manual checks:

- old daemon gives a clear incompatibility error before setup;
- initialized and uninitialized devices render distinctly;
- two remembered iPods keep independent pages and pending edits;
- disabling auto-sync disconnected prevents plug-in sync;
- exact/generic icon selection matches daemon facts;
- typed progress, prompts, pause/cancel, notifications, and eject target the
  correct device;
- Windows Apple software/iTunes can manage the iPod after Classick sync.
