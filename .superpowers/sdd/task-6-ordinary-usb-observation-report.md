# Task 6: ordinary USB observation source with no automatic SCSI

## Outcome

Implemented a production `device::discovery::observe_mount` entry point for a
caller-supplied mount and `ObservationId`. It classifies the filesystem before
performing one best-effort ordinary USB probe, then assembles the result through
`ReportedDeviceObservation`.

Recognizable mounts remain visible as `identity_unavailable` when association
or USB identity fails. The supplied observation ID is retained and the result
is never mutation-eligible. The observer reads only an already-existing regular
flat `SysInfo` and accepts non-empty `ModelNumStr`/`FirmwareVersion` fields as
optional enrichment; it never modifies the file.

The native result contains only:

- raw USB iSerial/FireWire GUID;
- USB product ID;
- exact capacity bytes.

Windows retains its zero-access volume queries plus SetupAPI/Configuration
Manager association. macOS retains its IOKit mount/media-to-USB association.
Linux retains its mountinfo/block-device/sysfs parent walk. Unsupported targets
return no USB facts while filesystem classification remains available.

Removed the automatic cached SCSI inquiry and all SCSI/raw extended-response
fields from `ipod/device.rs`. Legacy v2 scanning and libgpod identity resolution
now consume only pre-existing flat SysInfo fields and ordinary USB facts. The
PID/capacity model selection remains confined to that signing/presentation
compatibility adapter and is not supplied to `HardwareFacts` as a reported
model, generation, or colour.

## TDD evidence

### RED 1: production observation boundary

After adding the discovery tests but before production changes:

```text
cargo test -p classick --lib device::discovery_tests -- --test-threads=1
error[E0432]: unresolved imports
`super::discovery::observe_mount_with_probe`,
`super::discovery::OrdinaryUsbFacts`
```

The failure was the expected missing-feature failure.

### GREEN 1

After adding the observer and narrow facts boundary:

```text
cargo test -p classick --lib device::discovery_tests -- --test-threads=1
8 passed; 0 failed
```

Coverage includes restored and invalid-database layouts with byte-for-byte
zero-write snapshots, missing/malformed/failed USB identity, observation-ID
retention, ambiguous Classic 160 GB catalogue behavior, optional SysInfo facts,
probe ordering/cardinality, the exhaustive ordinary-facts shape, and a scoped
source regression forbidding SCSI references in production discovery/identity.

### RED 2: Windows USB-parent authority

Focused review found that the existing Windows parent parser accepted a PID
without proving the parent was Apple:

```text
cargo test -p classick --lib \
  ipod::device::tests::pid_extraction_rejects_non_apple_usb_parent \
  -- --exact --test-threads=1
FAILED: VID_1234 with PID_1261 was accepted
```

### GREEN 2

The parser now requires an exact `VID_05AC` component before accepting PID:

```text
focused regression: 1 passed; 0 failed
```

### RED 3: Windows Apple-parent fact boundary

A second Windows-focused parser test was added before replacing disk-interface
serial extraction with facts derived from the associated USB parent:

```text
cargo test -p classick --lib \
  ipod::device::tests::apple_usb_parent_facts_reject_non_apple_identity_and_retain_partial_facts \
  -- --exact --test-threads=1
error[E0425]: cannot find function `ordinary_facts_from_apple_usb_parent`
```

### GREEN 3

The pure parent parser now rejects non-Apple parents and retains PID/capacity
when the Apple parent's iSerial is malformed:

```text
focused regression: 1 passed; 0 failed
ipod::device::tests: 24 passed; 0 failed
```

## Verification

- `cargo test -p classick --lib device::discovery_tests -- --test-threads=1`
  - 8 passed, 0 failed.
- `cargo test -p classick --lib ipod::device::tests -- --test-threads=1`
  - 24 passed, 0 failed.
- `cargo test -p classick --lib ipod::macos_iokit::tests -- --test-threads=1`
  - 4 passed, 0 failed.
- `cargo test -p classick --lib device::observation_tests -- --test-threads=1`
  - 12 passed, 0 failed.
- `cargo test -p classick --lib device::readiness_tests -- --test-threads=1`
  - 12 passed, 0 failed.
- `cargo test --workspace -- --test-threads=1`
  - exit 0; 778 library tests plus all binary, integration, example, and doc
    test targets passed.
- `cargo fmt --all --check`
  - exit 0.
- `git diff --check`
  - exit 0.
- `cargo check --workspace`
  - exit 0.

The first full workspace attempt caught a compile regression after making the
macOS IOKit serial field optional: `examples/mac-identity.rs` formatted the
public field as `Display`. The diagnostic example now formats the optional
value explicitly, preserving PID/capacity facts when Apple association succeeds
without a usable serial. The complete workspace gate above is the post-fix run.

`cargo check -p classick --target x86_64-pc-windows-msvc` could not reach
Classick code on this macOS host: `blake3`'s build script requires MSVC
`ml64.exe`, which is unavailable. A real Windows-native compile/test remains a
later gate.

## Safety review

- Production observation performs reads only; it contains no create, write,
  repair, initialization, persistence, shell-out, elevation, wire, or daemon
  operation.
- No raw USB identifier or rejected identifier is logged by the new path.
- `OrdinaryUsbFacts` has no disk number, SCSI response, printed serial,
  display/volume identity, battery, appearance, or capability field.
- `ipod/device.rs` contains no `scsi_inquiry` or `read_sysinfo_extended`
  reference. `scsi_inquiry.rs` remains an explicitly invoked diagnostic.
- No private device captures or facts were read or committed.

## Files

- `crates/classick/src/device/discovery.rs`
- `crates/classick/src/device/discovery_tests.rs`
- `crates/classick/src/device/mod.rs`
- `crates/classick/Cargo.toml` (comments only; no dependency change)
- `crates/classick/examples/mac-identity.rs`
- `crates/classick/src/ipod/device.rs`
- `crates/classick/src/ipod/macos_iokit.rs`
- `crates/classick/src/scsi_inquiry.rs`
- `docs/architecture.md`
- `LEARNINGS.md`
- `.superpowers/sdd/task-6-ordinary-usb-observation-report.md`

## Remaining gates

- Windows-native compile/tests for SetupAPI, Configuration Manager, and
  zero-access IOCTL bindings.
- Physical-device observation on macOS, Windows, and Linux. No physical-device
  or Windows-native success is claimed here.
