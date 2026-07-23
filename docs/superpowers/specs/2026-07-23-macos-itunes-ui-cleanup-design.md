# macOS iTunes-Inspired UI Cleanup Design

## Scope

This is a Swift-only cleanup of the existing macOS window. It keeps the native
`NavigationSplitView`, the floating glass device bar, current navigation, and
current configuration semantics. It does not change the daemon, IPC contract,
device protocol, or Windows UI.

## Visual direction

Use mid-2000s iTunes as an information-hierarchy reference rather than copying
Aqua chrome. The source list should be compact and legible, library rows should
behave like a dependable outline, and device state should be conveyed once.
Native macOS materials, typography, selection, accessibility, and controls
remain authoritative for macOS 15 through macOS 27.

## Device presentation

- A remembered disconnected device remains in the Devices section, dimmed.
- Its sidebar row shows only the device artwork and name. It does not say
  "Not connected".
- Readiness problems such as required Finder setup remain visible because they
  require action and are not redundant connection status.
- The floating glass bar remains.
- For a disconnected remembered device, the bar shows artwork, device name,
  and "Connect to sync". It omits the unavailable capacity meter and all
  "Not connected" copy.
- Connected, syncing, paused, readiness, and error presentations retain their
  existing capacity, progress, actions, and diagnostic content.

## Configuration feedback

Music and Settings remain editable while the iPod is disconnected. Routine
local-save, host-save, and pending-device-delivery states are silent. Only
actionable host-acceptance and device-delivery failures render a status message.
This removes the standalone "Waiting for iPod" sections without hiding errors.

## Device artwork

The macOS UI loads artwork only from
`AMPDevices.framework/Versions/A/Resources`.

- A hardware observation that already resolves to an exact AMPDevices resource
  records that resource name in a macOS presentation cache keyed by canonical
  device serial.
- When a later disconnected snapshot lacks hardware facts, the cached exact
  resource is reused.
- If no verified exact resource has ever been observed, use AMPDevices'
  `iPodGeneric.icns`; do not substitute an SF Symbol or infer colour/model from
  a display label or capacity.
- The cache is presentation-only and is not device identity or protocol
  authority.

## Library disclosure behavior

Artist and genre expansion is explicit state owned by `LibraryBrowser`, keyed
by facet plus stable case-insensitive name. Clicking the disclosure or the row
toggles that state; dragging remains available in browse mode. Multiple groups
may remain expanded, and a library snapshot refresh does not collapse a group
whose key still exists.

## Validation

Use focused Swift tests for presentation copy/layout state, status visibility,
artwork resolution/cache behavior, and disclosure-state transitions. Then run
one macOS Xcode build and inspect the disconnected sidebar, floating bar, exact
or generic AMPDevices icon, and artist expansion in the running app.
