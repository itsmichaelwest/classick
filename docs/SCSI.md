# SCSI INQUIRY VPD Pages — Research, Findings, and Open Decisions

Working notes from a deep investigation into how iTunes identifies a
connected iPod and whether ipod-sync can match it. Captured here so
the next person (including future-us) doesn't have to re-derive any
of this.

**Status as of 2026-05-24**: investigation complete; implementation
of SCSI INQUIRY transport is in `src/scsi_inquiry.rs` and works under
admin; runtime architecture decision still pending the
heuristic-acceptance test (see "Decision matrix" at the bottom).

---

## The original problem

iTunes refuses iPods that ipod-sync has touched, with the error
"iTunes cannot read the contents of the iPod". The iPod itself plays
the synced music — its firmware accepts our `iTunesDB` — but iTunes
won't mount it for syncing/restoring/etc. We needed to know why
iTunes rejects and how to fix it.

Our first theory (the iTunesDB hash signature was wrong) led us to
discover that:

1. iPod Classic 7G uses the **hash58** algorithm, which is fully
   reverse-engineered in libgpod.
2. The only inputs hash58 needs are the FirewireGuid and the iPod's
   generation (for libgpod's checksum-type dispatch).
3. We had been populating libgpod's `device->sysinfo` hash table
   with a synthetic `ModelNumStr = "xPID_1261"` marker that
   libgpod's `ipod_info_table` doesn't recognise. That collapsed
   `itdb_device_get_checksum_type()` to `ITDB_CHECKSUM_NONE` and the
   resulting DB was unsigned — explaining the iTunes rejection.

That sub-bug is now fixed: we write a real Apple `ModelNumStr` (e.g.
`MC293` for a 160GB iPod Classic) derived from USB PID + drive
capacity, via libgpod's existing `itdb_device_set_sysinfo` in-memory
helper. We do NOT write to the on-disk `SysInfo` file (modern iTunes
leaves it 0 bytes, and our writing it was a deviation that may have
been contributing to the rejection).

The remaining question: **does iTunes need more than what libgpod's
hash58 path produces from FirewireGuid + ModelNumStr?** That's what
the SCSI investigation was about — finding out what iTunes itself
reads from the device, and whether we need to match it exactly.

---

## How iTunes identifies an iPod (the answer)

**iTunes sends SCSI INQUIRY EVPD commands over standard USB mass
storage** to read a multi-page XML plist directly from the device
firmware. The data is generated on-the-fly from the iPod's NOR
flash `SysCfg`. It is **not** stored as a file on the iPod's data
partition; iTunes re-queries it on every connection.

This XML is what older versions of libgpod called `SysInfoExtended`.
Pre-2010 iTunes wrote it to
`iPod_Control/Device/SysInfoExtended` for the benefit of third-party
tools. Modern iTunes does not — it just reads SCSI on demand.

### The protocol

Two-step SCSI INQUIRY EVPD sequence:

**Step 1**: Send INQUIRY with VPD page `0xC0` (the index page).
Response payload is a list of supported content page codes. For the
iPod Classic 7G this is pages `0xC2` through `0xE8` (39 pages).

**Step 2**: For each page code in the list, send INQUIRY with
`CDB[2] = page`. Each response contains a chunk of UTF-8 XML in
`response[4..]` (after the 4-byte VPD header). Concatenate the
chunks in order — the result is a complete Apple plist XML
document.

Each INQUIRY uses a 6-byte CDB:

```
CDB[0] = 0x12   // INQUIRY opcode
CDB[1] = 0x01   // EVPD bit set
CDB[2] = <page code>
CDB[3] = 0x00
CDB[4] = 0xFC   // allocation length = 252 bytes
CDB[5] = 0x00
```

On Windows: send via `DeviceIoControl(handle,
IOCTL_SCSI_PASS_THROUGH_DIRECT, ...)`. On Linux: via the SG_IO
ioctl. libgpod's `tools/ipod-scsi.c` implements this for Linux.

### What the XML contains (real dump from our 160GB Classic 7G)

13,174 bytes. The interesting top-level keys we extract:

| Key | Example value | Use |
|---|---|---|
| `BuildID` | `9.0.4` | Internal firmware build |
| `VisibleBuildID` | `2.0.4` | User-visible firmware version |
| `FamilyID` | `11` | libgpod-internal device family ID (Classic) |
| `UpdaterFamilyID` | `35` | Firmware-updater family |
| `FireWireGUID` | `000A27002138B0A8` | Crypto key for hash58 (also in USB iSerialNumber) |
| `SerialNumber` | `EXAMPLE1234` | Apple's 11-char serial (printed on case) |
| `DBVersion` | `3` | Determines hash type: 3 = HASH58, 4 = HASH72, etc. |
| `MinITunesVersion` | `9.0` | Minimum iTunes version this firmware speaks |
| `RAM` | `64` | MB of RAM |
| `MaxTracks` | `65534` | Library size limit |
| `MaxFileSizeInGB` | `4` | Single-file size limit |
| `Capacity` | (absent — use `VisibleCapacity` in newer firmware) | Marketed capacity |

Plus extensive sub-dicts for `AudioCodecs`, `VideoCodecs`,
`AlbumArt`, `ImageSpecifications`, `BuiltInGames` (our device:
"iPod Quiz", "Klondike", "Vortex" — Vortex was Classic-7G-only),
`VoiceMemoFormats`, `VolumeInformation`, etc.

**Notable for our device**: `ModelNumStr` is absent from this
firmware's XML. We confirmed Classic 7G with VisibleBuildID 2.0.4
just doesn't expose it. The community-published SysInfoExtended dumps
from older Classic firmware DO include `ModelNumStr`, so this is
firmware-version-dependent. Our heuristic (PID + capacity → MC293)
fills the gap.

### Why iTunes' rejection is NOT about us missing this data

For Classic devices (all three generations: 1G, 2G, 3G):
- DBVersion is always 3 → HASH58
- HASH58 algorithm needs ONLY the FirewireGuid (8 bytes, fully
  derived from USB iSerialNumber)
- libgpod's `itdb_device_get_checksum_type()` returns HASH58 when
  the device generation resolves to `ITDB_IPOD_GENERATION_CLASSIC_*`
- Generation resolution requires either (a) SysInfoExtended
  populated (we'd need disk write or in-memory patch), or
  (b) `ModelNumStr` populated to a value in libgpod's
  `ipod_info_table`

So for Classic, populating just `ModelNumStr` + `FirewireGuid`
via the existing `itdb_device_set_sysinfo` in-memory call is
**functionally equivalent** to populating the full SysInfoExtended
struct, as far as hash signing goes. The DB bytes should be
identical.

Where SCSI WOULD matter:
- **iPod Nano 5G** (DBVersion 4) — needs hash72, which derives a
  per-device key from data inside SysInfoExtended that we can't
  reconstruct from USB alone.
- **iPod Nano 6G/7G** — needs hashAB (a third algorithm).
- **Forensic UI display** — showing the exact firmware version,
  RAM, capabilities to the user.
- **Capability negotiation** — knowing supported codecs, artwork
  format IDs, etc. without hardcoding them.

For the current iPod Classic 7G goal, SCSI INQUIRY gives us
zero additional information that affects iTunes acceptance.

---

## Permission model (the hard problem)

`IOCTL_SCSI_PASS_THROUGH_DIRECT` control code = `0x4D014`. The low
bits encode `FILE_READ_ACCESS | FILE_WRITE_ACCESS`. Windows' I/O
manager validates this against the open handle's granted access
before dispatching the IOCTL.

So the handle to `\\.\<DriveLetter>:` (or `\\.\PhysicalDriveN`) must
have BOTH read AND write access for the IOCTL to fire. On modern
Windows, opening a raw volume with `GENERIC_READ | GENERIC_WRITE`
requires administrator elevation.

### What we tested

| Open mode | Caller privilege | Result |
|---|---|---|
| `\\.\PhysicalDriveN` + `GENERIC_READ` | Normal user | `CreateFile` fails: `ERROR_ACCESS_DENIED` |
| `\\.\G:` + `GENERIC_READ` | Normal user | `CreateFile` fails: `ERROR_ACCESS_DENIED` |
| `\\.\G:` + zero access | Normal user | `CreateFile` succeeds, IOCTL fails: `ERROR_ACCESS_DENIED` |
| `\\.\G:` + `GENERIC_READ \| GENERIC_WRITE` | Normal user | `CreateFile` fails: `ERROR_ACCESS_DENIED` |
| `\\.\G:` + `GENERIC_READ \| GENERIC_WRITE` | Admin | **Works** — returns 13KB of XML |

The non-admin paths all fail at the IOCTL level. There is no
documented Windows API path that exposes vendor-specific SCSI VPD
pages (0xC0+) without administrator-level handle access.

### What does NOT work

- `IOCTL_STORAGE_QUERY_PROPERTY` — has zero `STORAGE_PROPERTY_ID`
  value for arbitrary VPD pages. `StorageDeviceIdProperty` returns
  only Windows-parsed identifiers from VPD page 0x83, not raw
  Apple pages.
- `IOCTL_STORAGE_PROTOCOL_COMMAND` — added in Win10 1903, scoped to
  NVMe/ATA, no SCSI VPD pass-through.
- `IOCTL_SCSI_PASS_THROUGH` (non-DIRECT) — same access requirements
  as DIRECT variant.
- WMI / `MSFT_PhysicalDisk` / `Win32_DiskDrive` — exposes only
  Windows-parsed identifiers, blind to vendor VPD pages.
- `Windows.Devices.Usb` (WinRT) — Mass Storage Class (0x08) is
  explicitly blocked from this API.

### What DOES work (with caveats)

| Approach | One-time cost | Per-launch cost | Engineering effort |
|---|---|---|---|
| Daemon runs as admin | UAC every launch | UAC every launch | Low |
| Per-device UAC probe + cache | One UAC per new iPod | None | Medium |
| LocalSystem helper service (MSIX) | One UAC at install | None | Medium |
| SDDL grant via traditional installer (WiX/MSI) | One UAC at install | None | Medium-High |

---

## MSIX-specific constraints

We discovered that MSIX packaging (the default for WinUI 3 / Windows
App SDK) has hard limits on install-time operations:

- **No arbitrary install-time code execution** — no MSI custom
  actions, no scripts. The `customInstallActions` extension is
  scoped to MSIXVC (Xbox/Game packaging) only.
- **No kernel-mode driver installation** — drivers must come via
  Windows Update, WHQL, or a traditional installer outside the MSIX.
- **No `DeviceCapability` for mass storage** — USB Class 0x08 is
  explicitly blocked from `Windows.Devices.Usb`.

So inside MSIX, the ONLY way to get unprivileged SCSI access at
runtime is:

### The `desktop6:Service` extension

Added in Win10 2004. Lets MSIX install a real Windows service that
runs as a specified account (LocalSystem, LocalService, or
NetworkService). Requires three restricted capabilities:

```xml
<Capabilities>
  <rescap:Capability Name="runFullTrust" />
  <rescap:Capability Name="packagedServices" />
  <rescap:Capability Name="localSystemServices" />
</Capabilities>

<Applications>
  <Application ...>
    <Extensions>
      <desktop6:Extension Category="windows.service"
                          Executable="ipod-sync-helper-svc.exe"
                          EntryPoint="Windows.FullTrustApplication">
        <desktop6:Service Name="IpodSyncHelper"
                          StartupType="auto"
                          StartAccount="localSystem" />
      </desktop6:Extension>
    </Extensions>
  </Application>
</Applications>
```

MSIX install triggers ONE UAC prompt (because installing a service
requires admin). Service runs at boot under LocalSystem (highest
privilege), exposes a named pipe `\\.\pipe\ipod-sync-helper` with
SDDL `D:(A;;GRGW;;;BU)` (grant read+write to Built-in Users).
Unprivileged daemon connects, sends a request, gets the SysInfoExtended
XML back. Zero UAC prompts after install.

### Microsoft Store distribution

`packagedServices` + `localSystemServices` are restricted capabilities
requiring Store reviewer approval. Microsoft has approved them for:

- VPN clients (Cisco AnyConnect)
- Backup tools
- Hardware management utilities
- System monitoring apps

Not guaranteed approval — depends on submission justification. An
iPod sync tool that needs to communicate with device firmware fits
this category but a rejection is possible. Sideload distribution
(the typical WinUI 3 internal flow) has no restriction.

---

## Alternative: traditional installer (WiX MSI, Inno, etc.)

WinUI 3 / Windows App SDK supports two distribution models:

**Packaged (MSIX, default)** — all the MSIX restrictions above.

**Unpackaged (introduced in WAS 1.0)** — app runs without MSIX
identity. Can be distributed via ANY installer technology:

- WiX (MSI) — open source, free, mature, Microsoft uses it internally
- Inno Setup — popular for indie apps
- NSIS — lightweight scripting
- Advanced Installer — commercial GUI tool
- Squirrel.Windows — delta-update-friendly

With a traditional admin installer, the install-time step can call
`SetupDiSetDeviceRegistryProperty(SPDRP_SECURITY, sddl, ...)` to
grant Built-in Users full access to the iPod's device object via
its registry-stored security descriptor. After install:

- Daemon runs as a normal user
- `CreateFile(\\.\G:, GENERIC_READ | GENERIC_WRITE, ...)` succeeds
  without elevation (the BU SDDL grant lets normal users open the
  handle)
- `IOCTL_SCSI_PASS_THROUGH_DIRECT` dispatches
- No service needed, no per-launch UAC, no per-device UAC

Trade-offs vs MSIX:

| | MSIX + helper service | Traditional installer + SDDL grant |
|---|---|---|
| First UAC | At MSIX install | At MSI install |
| Per-launch UAC | None | None |
| Per-device UAC | None | None |
| MSIX auto-update | ✓ | ✗ (need Squirrel/Winget/own updater) |
| Store distribution | Possible (with review) | ✗ |
| Engineering effort | Service binary + pipe IPC + manifest | Installer authoring + SetupDi call |
| Runtime architecture | Service indirection | Direct |
| Apple-style? | Yes (Docker, AnyConnect) | Yes (Logitech, Razer, every USB-tool vendor) |

Both are legitimate Windows patterns. Both require ONE admin step
at install time and zero after.

---

## What we built so far

In service of investigating this, we've shipped (uncommitted):

- `src/scsi_inquiry.rs` — `IOCTL_SCSI_PASS_THROUGH_DIRECT` wrapper.
  Iterates VPD page 0xC0 → 0xC2..0xE8, returns the full XML.
  Works under admin; gracefully fails under non-admin so the
  caller can fall back to the heuristic.
- `src/sysinfo_extended.rs` — `plist` crate parser. Extracts
  `ModelNumStr`, `SerialNumber`, `FirewireGuid`, `FamilyID`,
  `BuildID`, capacity into a strongly-typed `ParsedSysInfo`.
  Six unit tests including the Classic 3G fixture.
- `examples/scsi-probe.rs` — standalone CLI binary to dump
  SysInfoExtended XML from any connected iPod for diagnostic
  purposes. Confirms the SCSI code works end-to-end (verified
  against the user's actual iPod returning 13,174 bytes of valid
  XML under admin).
- `src/ipod/device.rs::scan_drive_for_ipod` — now calls SCSI as
  the preferred identity source, falling back to the
  PID-+-capacity heuristic when SCSI is unavailable.
  **Crucially: we no longer write `SysInfo` to disk.** Modern
  iTunes leaves that file at 0 bytes; we mirror that exactly.
- `src/ipod/device.rs::resolve_libgpod_identity` — apply-time
  identity resolution with the preference order:
  1. on-disk SysInfo (older iTunes / gtkpod state)
  2. SCSI INQUIRY (authoritative)
  3. USB PID + capacity heuristic
- `src/ipod/device.rs::set_model_num` — populates libgpod's
  in-memory `device->sysinfo` hash table with `ModelNumStr`,
  parallel to the existing `set_firewire_guid`.
- `src/apply_loop.rs` — calls `resolve_libgpod_identity` and
  pushes both FirewireGuid and ModelNumStr into libgpod before
  `itdb_write`.

We have NOT (and have decided NOT to):

- Write `SysInfoExtended` to disk on the iPod — deviation from
  modern iTunes' on-disk behavior.
- Write `SysInfo` to disk — same reason.
- Patch libgpod to expose an in-memory `device->sysinfo_extended`
  setter — would unlock hash72/hashAB but adds vendor patch
  maintenance burden; out of scope for current Classic-only goal.

---

## Decision matrix (open)

The next decision depends on whether iTunes accepts the DB we
produce with the heuristic-only path (no SCSI access, just
FirewireGuid + heuristic ModelNumStr fed in-memory to libgpod).

### If iTunes accepts → ship the heuristic path

- Daemon stays unprivileged forever
- No installer changes needed
- SCSI code stays in the tree, dormant; useful for:
  - Future Nano 5G+ support (would need libgpod patch + helper
    service for runtime access)
  - Diagnostic `scsi-probe.exe` example
  - Showing precise firmware info in UI for power-user diagnostics

### If iTunes rejects → SCSI access becomes a real requirement

Then we need ONE of:

**Option A: LocalSystem helper service inside MSIX** (~half day)
- Keeps MSIX distribution path
- One UAC at MSIX install
- Risk: Store may not approve `localSystemServices` (sideload OK
  unconditionally)
- Service binary is tiny (~100 lines of Rust), stateless,
  crash-safe

**Option B: Traditional installer (WiX MSI) with SDDL grant** (~day)
- Switches WinUI 3 to unpackaged distribution
- One UAC at MSI install
- Loses MSIX auto-update (need Squirrel or similar)
- Loses Store path
- Simpler runtime architecture (no service indirection)
- Same pattern every USB hardware vendor uses on Windows

**Option C: Per-device UAC probe** (~half day, worse UX)
- Each new iPod = one UAC prompt at first plug-in
- Cache the result in our config (`%APPDATA%\ipod-sync\...`)
- iTunes restore wipes the iPod → re-prompt next plug-in
- Works regardless of MSIX vs traditional installer
- Worse UX than A or B but no installer changes

### Recommended decision flow

```
[Test heuristic path]
       │
   does iTunes accept?
   │           │
  yes         no
   │           │
   ▼           ▼
 [SHIP]    [Pick A or B based on MSIX commitment]
           │
           ├── Want to keep MSIX → Option A
           └── Want simpler runtime → Option B
                                       │
                                       └── (lose Store, lose MSIX auto-update)
```

---

## Open questions / future work

- **Is `localSystemServices` actually rejectable by the Microsoft
  Store for our use case?** Need a Store reviewer's input. Best
  approach: file a precertification submission with the helper
  service to see.
- **Does Apple's `\Driver\AppleIPod` expose a non-admin IOCTL
  surface for model identification?** Possible but undocumented;
  would require API Monitor / WinDbg of iTunes-to-driver traffic
  to RE. Risky bet — Apple can change it any time. Not worth
  pursuing unless other paths fail.
- **Should we support hash72 / hashAB (Nano 5G+)?** Currently out
  of scope per SPEC §7 but would expand the supported-device
  matrix significantly. Needs libgpod patching + SCSI access
  (which an MSIX helper service would already provide).
- **Should we ever write the on-disk `SysInfoExtended`?** Pre-2010
  iTunes did, and the iPod firmware tolerates it. But modern
  iTunes doesn't, so writing it would deviate from current
  iTunes behavior. Probably not, unless we discover a hash path
  that genuinely requires the file present.

---

## Primary sources

- [IOCTL_SCSI_PASS_THROUGH_DIRECT (MSDN)](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddscsi/ni-ntddscsi-ioctl_scsi_pass_through_direct)
- [ACLs and the Device Stack (MSDN)](https://learn.microsoft.com/en-us/previous-versions/windows/drivers/storage/acls-and-the-device-stack)
- [SDDL for Device Objects (MSDN)](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/sddl-for-device-objects)
- [`libgpod/tools/ipod-scsi.c`](https://github.com/fadingred/libgpod/blob/master/tools/ipod-scsi.c) — reference Linux implementation
- [`libgpod/src/itdb_device.c`](https://github.com/gtkpod/libgpod/blob/master/src/itdb_device.c) — model table + checksum dispatch
- [`desktop6:Service` element (MSDN)](https://learn.microsoft.com/en-us/uwp/schemas/appxpackage/uapmanifestschema/element-desktop6-service)
- [Restricted capabilities (MSDN)](https://learn.microsoft.com/en-us/windows/uwp/packaging/app-capability-declarations#restricted-capabilities)
- [`dstaley/ipod-sysinfo`](https://github.com/dstaley/ipod-sysinfo) — community-collected SysInfoExtended dumps
- [`freemyipod.org/wiki/SysCfg`](https://freemyipod.org/wiki/SysCfg) — iPod firmware NOR flash layout

## Reference artifacts in this repo

- `reference-itunes-baseline.bin` — iTunes-restored, empty 14KB DB
- `reference-itunes-sysinfo.txt` — iTunes' post-restore SysInfo (0 bytes — empty)
- `reference-itunes-after-sync.bin` — iTunes-synced 16KB DB with one track
- `reference-itunes-sysinfo-after-sync.txt` — iTunes' post-sync SysInfo (still 0 bytes — confirms iTunes doesn't write SysInfo)
