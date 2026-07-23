# macOS iTunes-Inspired UI Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove redundant disconnected/waiting UI, preserve the floating glass
device bar, make artist/genre expansion reliable, and keep AMPDevices artwork
available while disconnected.

**Architecture:** Keep the change inside the macOS Swift target. Pure
presentation rules remain testable outside SwiftUI; `DeviceIcon` owns a
UserDefaults-backed presentation cache, while `LibraryBrowser` owns explicit
disclosure state.

**Tech Stack:** Swift 6.3, SwiftUI, AppKit `NSImage`, XCTest, macOS 15+.

## Global Constraints

- Do not change Rust, IPC, the device protocol, or Windows UI.
- Do not infer device colour or exact model from capacity, name, or display label.
- Use AMPDevices artwork on macOS, including `iPodGeneric.icns` as the honest fallback.
- Preserve actionable readiness and failure messages.
- Keep the existing floating glass device bar.
- Do not commit unless the user explicitly requests a commit.

---

### Task 1: Clean up disconnected and configuration status presentation

**Files:**
- Modify: `ui/macos/Sources/Classick/Views/Sidebar.swift`
- Modify: `ui/macos/Sources/Classick/Model/DeviceRowPresentation.swift`
- Modify: `ui/macos/Sources/Classick/Views/DeviceRow.swift`
- Modify: `ui/macos/Sources/Classick/Model/DeviceConfigEditingState.swift`
- Modify: `ui/macos/Sources/Classick/Views/DeviceConfigStatusView.swift`
- Modify: `ui/macos/Sources/Classick/Views/DeviceSettingsPage.swift`
- Test: `ui/macos/Tests/ClassickTests/DeviceRowPresentationTests.swift`
- Test: `ui/macos/Tests/ClassickTests/DeviceSettingsLogicTests.swift`

**Interfaces:**
- Produces: `SidebarDeviceRow.detail: String?`
- Produces: `DeviceRowPresentation.Meter.hidden`
- Produces: `DeviceConfigComponentStatus.shouldPresent: Bool`

- [ ] Write tests asserting that disconnected sidebar detail is absent,
  disconnected device-row copy is "Connect to sync" with a hidden meter, and
  only failure statuses should present.
- [ ] Run the focused tests and verify they fail against current behavior.
- [ ] Implement the optional sidebar detail, hidden meter rendering, revised
  disconnected copy, and failure-only configuration status visibility.
- [ ] Re-run the focused tests and verify they pass.

### Task 2: Make disclosure state explicit and durable

**Files:**
- Modify: `ui/macos/Sources/Classick/Views/LibraryBrowser.swift`
- Test: `ui/macos/Tests/ClassickTests/LibraryBrowserLogicTests.swift`

**Interfaces:**
- Produces: `LibraryBrowser.DisclosureKey`
- Produces: `LibraryBrowser.toggledDisclosure(_:in:)`

- [ ] Write a test that toggles artist and genre keys independently and retains
  multiple expanded keys across library-value replacement.
- [ ] Run the focused test and verify the missing disclosure API fails to compile.
- [ ] Bind each `DisclosureGroup` to `@State` keyed by `DisclosureKey`, and add
  an explicit row tap that toggles the same state without removing browse-mode
  dragging.
- [ ] Re-run the focused test and verify it passes.

### Task 3: Resolve every rendered icon through AMPDevices

**Files:**
- Modify: `ui/macos/Sources/Classick/Views/DeviceIcon.swift`
- Modify: `ui/macos/Sources/Classick/Views/Sidebar.swift`
- Modify: `ui/macos/Sources/Classick/Views/DeviceRow.swift`
- Test: `ui/macos/Tests/ClassickTests/DeviceIconLogicTests.swift`

**Interfaces:**
- Produces: `DeviceArtworkCache`
- Changes: `DeviceIcon` accepts an optional `DeviceID`
- Produces: exact observed resource, cached exact resource, or
  `iPodGeneric.icns`; no SF Symbol fallback.

- [ ] Write focused tests using an isolated `UserDefaults` suite to prove an
  exact resource is cached by serial, reused without hardware facts, and never
  replaces a newer verified exact observation.
- [ ] Run the focused tests and verify they fail because the cache/resolver does
  not exist.
- [ ] Add the presentation cache and AMPDevices generic fallback, pass serials
  from sidebar and floating-bar call sites, and record only exact verified
  resources.
- [ ] Re-run the focused tests and verify they pass.

### Task 4: Focused integration verification

**Files:**
- Modify only task files if verification exposes a blocking issue.

- [ ] Run the affected Swift test classes.
- [ ] Run the macOS app's Xcode build once.
- [ ] Inspect the running disconnected UI: no sidebar status line, compact
  floating glass bar with "Connect to sync", AMPDevices artwork, no waiting
  panels, and working artist expansion.
- [ ] Review only the task diff for correctness, accessibility, and accidental
  scope growth.
