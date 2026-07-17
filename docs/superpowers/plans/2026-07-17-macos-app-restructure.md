# macOS App Restructure (Plan B of 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the macOS app around the approved design — sidebar with device disclosure (Music/Settings children), browse-only Library, the canonical device Music page (mode dropdown, facet control, conditional checkboxes, live capacity), the device Settings page, and playlist editors — consuming daemon protocol 1.6.0.

**Architecture:** `AppModel` (single `@Observable` reducer) grows playlist/device-config state fed by the 1.6.0 wire; navigation stays one `NavigationSplitView` whose sidebar selection is a new `SidebarDestination` enum (library / device(serial, page) / playlist(slug) / history); the rich browser becomes ONE reusable component (`LibraryBrowser`) parameterized by facet + optional checkbox bindings, used by the Library page (read-only), the device Music page (conditional checkboxes), and the Add Songs picker.

**Tech Stack:** Swift 6 strict concurrency, SwiftUI (macOS 15 floor; Liquid Glass niceties only via `if #available(macOS 26, *)`), SwiftPM + committed XcodeGen project.

**Spec:** `docs/superpowers/specs/2026-07-17-library-playlists-devices-design.md` (Figma frames `3:3773` / `4:6349` in file `5hOlDaKWg7UPFNnRY2fznV` are the visual truth). **Depends on Plan A** (`2026-07-17-playlists-core.md`) — wire 1.6.0 must exist before Task 1 here.

## Global Constraints

- Base: `main` with Plan A merged (daemon protocol 1.6.0 live).
- Work from `ui/macos/`. `swift test` + `swift build` (zero new warnings) before every commit; if any Swift file is ADDED or REMOVED run `xcodegen generate` and commit the `Classick.xcodeproj/project.pbxproj` diff in the same commit (bundle.sh builds from the committed pbxproj); final task runs `bundle.sh`.
- Wire field names are verbatim snake_case mirrors of `docs/ipc-protocol.md` v1.6.0; decoders are absent-tolerant (old daemons must not crash the app).
- Canonical-surface rule: sync intent is displayed/edited ONLY on device pages. The Library page ships with NO checkbox affordances. No drag-drop / context-menu gestures in this plan (deferred v2).
- Mode↔content contract: checkboxes render only in Selected/Except modes; Entire library renders the same browser read-only with the caption "Everything in your library syncs to this iPod."; selective captions: "Checked items sync to this iPod." / "Checked items are left off this iPod."
- Mode switch Entire→Selected seeds checkboxes from current device contents (wire: the reducer derives from the device's manifest-backed synced set exposed via `get_device_config`+`preview_device`; exact seeding = the selection rules that reproduce the device's current contents by artist/album — see Task 5).
- Replace Library keeps its typed-confirmation flow, relocated to Settings; Remove iPod = existing ForgetIpod; "Update Now" = existing BackfillRockbox command (label "Force update artwork and metadata").
- Empty states (each one line + one action): no source configured → "Choose your music folder…" (opens setup); scanning → progress line; library empty → "No audio files found in <path>"; device empty → "Nothing synced yet — press Sync Now."
- Disconnected device: children visible, pages editable with caption "Not connected — changes apply on next sync", Sync Now disabled, sidebar row dimmed, collapsed by default.
- Parent device row click selects its Music child; the chevron alone toggles disclosure.
- Swift 6 Sendable patterns as in existing code; no new third-party deps. Conventional Commits; stage by name; never amend.

---

### Task 1: Wire mirrors for 1.6.0

**Files:**
- Modify: `ui/macos/Sources/Classick/Ipc/WireModels.swift`, `ui/macos/Sources/Classick/Ipc/DaemonClient.swift` (send helpers if the pattern requires), Test: `ui/macos/Tests/ClassickTests/WireCodecTests.swift`

**Interfaces (produces, exact Swift names):** `PlaylistSummary{slug,name,kind,tracks,bytes,error?}`, `PlaylistPayload` (enum manual/smart with associated payloads mirroring the doc), `SmartRulesWire` (verbatim rules JSON shape), `PlaylistDetail{slug,name?,kind?,tracks?,rules?,error?}` (reply to get_playlist; on error only slug+error are set), `SubscriptionsWire{version,playlists}`, `DeviceSettingsWire{version,autoSync,rockboxCompat}` (CodingKeys auto_sync/rockbox_compat), `DevicePreview{selectedTracks,selectedBytes,playlistExtraTracks,playlistExtraBytes,projectedFreeBytes?,unresolvedSubscriptions?}` (unresolved_subscriptions is omitted from the wire when empty — decode as optional, treat nil as []); `DaemonCommand` cases `.listPlaylists,.getPlaylist(slug),.savePlaylist(PlaylistPayload),.deletePlaylist(slug),.getDeviceConfig(serial),.saveDeviceConfig(serial,selection?,subscriptions?,settings?),.previewDevice(serial)`; `DaemonEvent` cases `.playlistsUpdate,.playlistDetail,.deviceConfigUpdate,.devicePreview`. Note: the wire omits `version` inside nested `subscriptions`/`settings` payloads (meaningful-fields-only convention) — decoders must default it (e.g. `version = 1` when absent).

- [ ] **Step 1: Failing codec tests** — decode each event from doc-literal JSON strings; encode each command and assert exact `"type"` strings + field names; absent-field tolerance (deviceConfigUpdate without settings). **Step 2–4: RED → implement → `swift test` PASS, `swift build` clean.**
- [ ] **Step 5: Commit** — `git commit -m "feat(ui): wire mirrors for daemon protocol 1.6.0"`

### Task 2: AppModel state + navigation model

**Files:**
- Modify: `ui/macos/Sources/Classick/Model/AppModel.swift`, Create: `ui/macos/Sources/Classick/Model/SidebarDestination.swift` (+ `xcodegen generate`), Test: `ui/macos/Tests/ClassickTests/AppModelReducerTests.swift`

**Interfaces (produces):** `enum SidebarDestination: Hashable { case library, device(serial: String, page: DevicePage), playlist(slug: String), history }`, `enum DevicePage { case music, settings }`; AppModel state: `playlists: [PlaylistSummary]`, `deviceConfigs: [String: DeviceConfigState]` (`selection`, `subscriptions`, `settings`, `preview: DevicePreview?`), reducer arms for the three new events, `selectedDestination: SidebarDestination?` with the parent-click rule as a pure helper `static func destinationForDeviceRowClick(serial: String) -> SidebarDestination` (→ `.device(serial, .music)`).

- [ ] **Step 1: Failing reducer tests** — playlists_update replaces list; device_config_update upserts by serial; device_preview attaches to the right serial; parent-click helper returns music page. **Step 2–4: RED → implement → PASS.**
- [ ] **Step 5: Commit** (include pbxproj) — `git commit -m "feat(ui): app model state + sidebar destination model for 1.6.0"`

### Task 3: Sidebar restructure

**Files:**
- Modify: `ui/macos/Sources/Classick/Views/MainWindow.swift`, Create: `ui/macos/Sources/Classick/Views/Sidebar.swift` (+ xcodegen), Test: reducer-level tests only (layout not unit-testable)

**Behavior (from frame 3:3773):** Sections Library / Devices / Playlists (+ button → creates an empty manual playlist via `.savePlaylist` with name "New Playlist", selects it) / History. Device rows: `DisclosureGroup` with custom label row (ipod SF symbol, name, eject button calling existing eject/forget affordance — eject only when connected); children Music (`music.note`) + Settings (`gear`) as `SidebarDestination` links at indent level 2; row label tap → `destinationForDeviceRowClick`; disconnected device dimmed (`.foregroundStyle(.secondary)`), `DisclosureGroup` default-collapsed, children still navigable. Playlist rows: `music.note.list` + name; error badge (`exclamationmark.triangle`) when `summary.error != nil`.

- [ ] **Steps:** implement; add a reducer test for +-button flow (savePlaylist command emitted then selection moves on playlists_update containing the new slug — test the pure decision fn you extract for "select newest created slug"); `swift test`+build; commit — `git commit -m "feat(ui): sidebar with device disclosure, playlists section, history"`

### Task 4: `LibraryBrowser` component + browse-only Library page

**Files:**
- Create: `ui/macos/Sources/Classick/Views/LibraryBrowser.swift` (+ xcodegen), Modify: `ui/macos/Sources/Classick/Views/LibraryView.swift` (becomes a thin wrapper: facet picker + `LibraryBrowser(mode: .browse)`; DELETE its checkbox plumbing), Test: `ui/macos/Tests/ClassickTests/LibraryBrowserLogicTests.swift` (+ xcodegen)

**Interfaces (produces):** `struct LibraryBrowser: View` with `enum Mode { case browse; case select(checked: Binding<Set<SelectionKey>>, style: SelectStyle) }`, `enum Facet { case artists, albums, genres, playlists }` (playlists facet only meaningful on device pages — browse mode hides it), `SelectionKey` = existing selection-rule shapes (artist/album/genre keys). Rows per frame 3:3773: artist column-headers grouping album rows (checkbox when selecting + name + "N tracks" + formatted bytes). Pure helpers exposed for tests: grouping/ordering fn from the wire `LibraryArtist/LibraryAlbum` aggregates, and `checkState(for:)` tri-state (artist header checkbox = all/some/none of its albums).

- [ ] **Step 1: Failing logic tests** — grouping order; tri-state artist checkbox from album set; toggling artist checks all its albums (pure fn on the Set). **Step 2–4: implement; Library page renders browse mode with zero checkbox affordances (assert via code review, not test). `swift test`+build.**
- [ ] **Step 5: Commit** — `git commit -m "feat(ui): reusable LibraryBrowser; Library page becomes browse-only"`

### Task 5: Device Music page

**Files:**
- Create: `ui/macos/Sources/Classick/Views/DeviceMusicPage.swift` (+ xcodegen), Modify: `ui/macos/Sources/Classick/Views/DeviceRow.swift` (only if shared formatting helpers move), `ui/macos/Sources/Classick/ClassickApp.swift` (route destination), Test: `ui/macos/Tests/ClassickTests/DeviceMusicLogicTests.swift` (+ xcodegen)

**Behavior (frame 3:3773):** toolbar = title + "Last synced …" subtitle, `Picker` popup `Sync: Entire library / Selected items / All except selected` bound to the device's `SelectionMode`, prominent Sync Now (disabled while syncing/scanning/disconnected); segmented control Artists|Albums|Genres|Playlists; content = `LibraryBrowser` — `.browse` when mode == entire, `.select` bound to the device's selection rules otherwise; caption line per the Global Constraints copy; Playlists segment = subscriptions checklist (playlist name + "N tracks · X GB" + "always synced, shown on the iPod" caption); floating capacity bar (existing formatting helpers) showing `preview` when present: "used + projected" segments, caption gains skipped-for-space text (existing trust-package state) and "N tracks missing art" line.
**Mode-switch seeding (pure, tested):** `static func seededSelection(fromDeviceContents albums: [LibraryAlbum-like], previousMode: SelectionMode, newMode: SelectionMode, current: SelectionRules) -> SelectionRules` — entire→selected returns include-rules reproducing the device's current contents (album-level rules); selected↔except keep rules; →entire keeps rules dormant. Reducer sends `save_device_config` + `preview_device` on every selection/subscription/mode edit (debounced like the existing auto-save selection pattern).

- [ ] **Step 1: Failing tests** — seeding truth table; caption-for-mode fn; Sync-Now-disabled predicate (syncing/scanning/disconnected). **Step 2–4: implement; `swift test`+build.**
- [ ] **Step 5: Commit** — `git commit -m "feat(ui): device Music page — mode dropdown, conditional checkboxes, subscriptions, live capacity"`

### Task 6: Device Settings page

**Files:**
- Create: `ui/macos/Sources/Classick/Views/DeviceSettingsPage.swift` (+ xcodegen), Modify: `ui/macos/Sources/Classick/Views/DeviceView.swift` (Replace flow relocates; keep the view as the source of the confirmation sheet code or move it wholesale — implementer's judgment, note it), `ui/macos/Sources/Classick/Views/SettingsView.swift` (app-level settings LOSE the global rockbox toggle — line stays only if a global fallback remains; per spec it moves per-device: remove from app settings, note migration in a caption), Test: reducer/logic tests

**Behavior (frame 4:6349):** `Form` sections — Info (Name, Capacity, Synced "X of Y", Last synced — all read-only from existing state); Toggles (Sync automatically when connected → `settings.autoSync`; Rockbox compatibility mode → `settings.rockboxCompat`; Force update artwork and metadata → "Update Now" button → existing `.backfillRockbox`); Destructive (label "Erase iPod and re-sync current selection" + "Replace Library…" destructive-bordered button → EXISTING typed-confirmation sheet + `.replaceLibrary`); Remove ("Remove Michael's iPod from Classick" + "Remove iPod" → existing forget flow with its confirm). Toggle edits send `save_device_config(settings:)`.

- [ ] **Steps:** logic tests (toggle edit emits save with only `settings` present — pure command-builder fn); implement; `swift test`+build; commit — `git commit -m "feat(ui): device Settings page per design frame; per-device toggles"`

### Task 7: Playlist editor pages

**Files:**
- Create: `ui/macos/Sources/Classick/Views/PlaylistPage.swift`, `ui/macos/Sources/Classick/Views/SmartRulesEditor.swift`, `ui/macos/Sources/Classick/Views/AddSongsPicker.swift` (+ xcodegen), Test: `ui/macos/Tests/ClassickTests/PlaylistEditorLogicTests.swift` (+ xcodegen)

**Behavior:** Manual: track list (title/artist via library index lookup, missing-file rows dimmed with warning icon), reorder via `.onMove`, remove via `.onDelete`/toolbar, **Add Songs…** sheet = `LibraryBrowser(.select)` scoped to a temp Set + Add button appends (dedup, keep order); rename inline; toolbar menu Delete Playlist (confirm: "Also unsubscribes N device(s)" when subscribed — count from deviceConfigs). Smart: name field, Match all/any picker, rule rows (field/op/value), limit (none/bytes/tracks with value), order picker; live preview footer "N tracks · X GB" from a `previewSmart` evaluation — v1 computes via `save` + `playlists_update` round-trip (summary carries tracks/bytes); acceptable latency, note in code. All edits send `.savePlaylist` (debounced).

- [ ] **Steps:** failing logic tests (append-dedup-preserve-order; delete-confirm count fn; rule-row validity → save-enabled predicate); implement; `swift test`+build; commit — `git commit -m "feat(ui): playlist editors — manual with Add Songs picker, smart rule builder"`

### Task 8: Empty states, disconnected pages, final polish + gate

**Files:**
- Modify: the pages above + `ui/macos/Sources/Classick/Views/MenuContent.swift` (rename "Update artwork & metadata" if wording drifted), Test: extend reducer tests

**Behavior:** the four empty states verbatim from Global Constraints (`ContentUnavailableView` pattern already in the codebase); disconnected-device caption + Sync Now disabled + dimmed row verified; `if #available(macOS 26, *)` glass touches ONLY where a plain control differs visibly (expected: none — document the finding either way).

- [ ] **Steps:** implement; full `swift test` + `swift build` + `ui/macos/bundle.sh` (app builds); commit — `git commit -m "feat(ui): empty states, disconnected-device behavior, restructure polish"`
- [ ] **Manual gate checklist (user, with iPod):** sidebar disclosure + parent-click; mode switch seeds from device contents (zero-diff at switch); subscribed playlist outside scope syncs + appears in iPod Music menu; playlists-only device; Settings toggles round-trip; Replace/Remove flows; empty states; disconnected editing applies on next sync; capacity projection tracks checkbox changes.

---

## Self-review notes (for executors)

- Spec §5 maps: sidebar→T3, Library→T4, Music page→T5, Settings→T6, playlist pages→T7, empty/disconnected→T8; §4 wire→T1; menu-bar unchanged (T8 label check only).
- ONE browser component serves three surfaces — do not fork it per page; that reuse is the design's coherence guarantee.
- The seeding fn (T5) is THE trust-critical logic in this plan: it must reproduce current device contents exactly (album-granularity) so a mode switch is zero-diff. Its truth-table test is not optional.
- Deleting LibraryView's checkbox plumbing (T4) is intentional spec behavior (canonical surface), not scope creep.
