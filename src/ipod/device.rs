//! Read FirewireGUID from the iPod's SysInfo and push it into libgpod's
//! device struct so itdb_write computes a valid signed iTunesDB.

use anyhow::{anyhow, Result};
use std::ffi::{CStr, CString};
use std::path::Path;

use crate::ffi;

/// Extract the value of the `FirewireGuid:` line from a SysInfo body.
/// Returns just the hex value (typically `0x...`).
pub fn extract_firewire_guid(sysinfo: &str) -> Result<String> {
    match parse_sysinfo_field(sysinfo, "FirewireGuid") {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) => Err(anyhow!("FirewireGuid line has no value")),
        None => Err(anyhow!("FirewireGuid key not found in SysInfo")),
    }
}

/// Resolve `<mount>\iPod_Control\Device\SysInfo`, read it, extract FirewireGuid.
pub fn read_firewire_guid(ipod_mount: &Path) -> Result<String> {
    let path = crate::ipod::layout::sysinfo_path(ipod_mount);
    let body = std::fs::read_to_string(&path)
        .map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
    extract_firewire_guid(&body)
}

/// Push the FirewireGuid into libgpod's `Itdb_Device` struct via the per-field
/// setter `itdb_device_set_sysinfo`. We use this instead of
/// `itdb_device_read_sysinfo_xml` because libplist is stripped from our
/// libgpod build (Phase 0 Task 3 patch).
///
/// # Safety
/// `device` must be a valid `*mut Itdb_Device` obtained from libgpod
/// (e.g. via `(*db.as_ptr()).device` after a successful `itdb_parse_file`).
pub unsafe fn set_firewire_guid(
    device: *mut ffi::Itdb_Device,
    guid: &str,
) -> Result<()> {
    if device.is_null() {
        return Err(anyhow!("Itdb_Device pointer is NULL"));
    }
    const KEY: &CStr = c"FirewireGuid";
    let value = CString::new(guid)
        .map_err(|_| anyhow!("FirewireGuid contains interior NUL byte"))?;
    ffi::itdb_device_set_sysinfo(device, KEY.as_ptr(), value.as_ptr());
    Ok(())
}

/// Push the ModelNumStr into libgpod's `Itdb_Device` struct. Without
/// this, `itdb_device_get_ipod_info` returns `UNKNOWN` and
/// `itdb_device_get_checksum_type` returns `ITDB_CHECKSUM_NONE`, so
/// libgpod writes an unsigned/placeholder hash that iTunes refuses to
/// validate ("cannot read the contents of the iPod"). The value must
/// resolve through libgpod's `ipod_info_table` — real Apple model
/// numbers like `MC293`, `MB565`, `MB029` (the lookup strips a
/// leading alpha so `MC293`, `C293`, and `xC293` all resolve to the
/// same entry).
///
/// # Safety
/// `device` must be a valid `*mut Itdb_Device` obtained from libgpod.
pub unsafe fn set_model_num(
    device: *mut ffi::Itdb_Device,
    model_num: &str,
) -> Result<()> {
    if device.is_null() {
        return Err(anyhow!("Itdb_Device pointer is NULL"));
    }
    const KEY: &CStr = c"ModelNumStr";
    let value = CString::new(model_num)
        .map_err(|_| anyhow!("ModelNumStr contains interior NUL byte"))?;
    ffi::itdb_device_set_sysinfo(device, KEY.as_ptr(), value.as_ptr());
    Ok(())
}

/// Full identity libgpod's hash58 code path needs to sign an
/// iTunesDB iTunes will accept on read.
///
/// Both fields together resolve through `itdb_device_get_ipod_info`
/// and `itdb_device_get_firewire_id`, which feed
/// `itdb_hash58_write_hash`'s key derivation. A missing or wrong
/// `model_num_str` collapses the checksum_type to `NONE` and the
/// resulting DB is iTunes-unreadable even though the iPod firmware
/// will still play the music.
#[derive(Debug, Clone)]
pub struct LibgpodIdentity {
    pub firewire_guid: String,
    pub model_num_str: String,
}

/// Resolve the identity libgpod needs to sign the iTunesDB, in
/// preference order:
///
/// 1. **On-disk SysInfo** if it carries both keys (older firmware,
///    a gtkpod user before us, an iPod previously paired with
///    pre-2010 iTunes). Honour what's there — never overwrite.
/// 2. **SCSI INQUIRY VPD** (the authoritative path — same mechanism
///    iTunes uses to identify the device, returns the real
///    ModelNumStr direct from device firmware).
/// 3. **USB PID + capacity heuristic** (fallback when SCSI is
///    unavailable; permission denied, unsupported firmware, etc.).
///
/// Returns an error only if all three paths fail to produce a
/// FirewireGuid — at which point libgpod can't sign anything and
/// the sync would be DOA regardless.
#[cfg(windows)]
pub fn resolve_libgpod_identity(ipod_mount: &Path) -> Result<LibgpodIdentity> {
    // Path 1: on-disk SysInfo.
    let sysinfo_path = crate::ipod::layout::sysinfo_path(ipod_mount);
    let sysinfo_text = std::fs::read_to_string(&sysinfo_path).unwrap_or_default();
    let disk_guid = parse_sysinfo_field(&sysinfo_text, "FirewireGuid")
        .filter(|s| !s.is_empty());
    let disk_model = parse_sysinfo_field(&sysinfo_text, "ModelNumStr")
        .filter(|s| !s.is_empty());

    // If on-disk SysInfo gives us BOTH, use it as-is and skip the
    // shell-out. Modern iTunes leaves SysInfo empty so this rarely
    // hits, but when it does (older iTunes, prior tool) we honour it.
    if let (Some(guid), Some(model_num_str)) = (disk_guid.clone(), disk_model.clone()) {
        return Ok(LibgpodIdentity { firewire_guid: guid, model_num_str });
    }

    // Path 2+3: shell to PowerShell to get USB info + SCSI INQUIRY
    // for the authoritative ModelNumStr.
    let letter = drive_letter(ipod_mount)
        .ok_or_else(|| anyhow!("path {} has no drive letter", ipod_mount.display()))?;
    let recovered = recover_ipod_info_from_usb(letter)
        .ok_or_else(|| anyhow!("USB recovery failed for drive {letter}"))?;

    let firewire_guid = disk_guid.unwrap_or_else(|| recovered.firewire_guid.clone());

    // ModelNumStr preference: SCSI (authoritative) > heuristic > disk-leftover.
    let model_num_str = recovered
        .sysinfo_extended_parsed
        .as_ref()
        .and_then(|p| p.model_num_str.clone())
        .or_else(|| recovered.identity.map(|id| id.model_num.to_string()))
        .or(disk_model)
        .ok_or_else(|| {
            anyhow!(
                "could not determine ModelNumStr for iPod at {} \
                 (PID {:?}, capacity {:?} bytes, SCSI parsed: {})",
                ipod_mount.display(),
                recovered.pid,
                recovered.capacity_bytes,
                recovered.sysinfo_extended_parsed.is_some(),
            )
        })?;

    Ok(LibgpodIdentity { firewire_guid, model_num_str })
}

/// Non-Windows stub. On other platforms we can only consult the
/// on-disk SysInfo (no USB shell-out, no SCSI pass-through).
/// Sufficient for tests and gtkpod-style installs.
#[cfg(not(windows))]
pub fn resolve_libgpod_identity(ipod_mount: &Path) -> Result<LibgpodIdentity> {
    let sysinfo_path = crate::ipod::layout::sysinfo_path(ipod_mount);
    let sysinfo_text = std::fs::read_to_string(&sysinfo_path)
        .map_err(|e| anyhow!("reading {}: {e}", sysinfo_path.display()))?;
    let firewire_guid = parse_sysinfo_field(&sysinfo_text, "FirewireGuid")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("no FirewireGuid in {}", sysinfo_path.display()))?;
    let model_num_str = parse_sysinfo_field(&sysinfo_text, "ModelNumStr")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("no ModelNumStr in {}", sysinfo_path.display()))?;
    Ok(LibgpodIdentity { firewire_guid, model_num_str })
}

/// Detected iPod identity returned by drive-scan helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedIpod {
    pub serial: String,
    pub model_label: String,
    pub drive: String,
    /// User-set "iPod name" from the iTunesDB master-playlist name
    /// (e.g. "Michael's iPod"). `None` if the iTunesDB couldn't be
    /// parsed at scan time — UI falls back to `model_label` in that
    /// case. Populated lazily on plug-in by the daemon to keep
    /// scan_drive_for_ipod itself cheap.
    pub name: Option<String>,
}

/// Scan all Windows drive letters for an iPod (presence of
/// iPod_Control\Device\SysInfo). Returns the first match.
pub fn scan_for_ipod() -> Option<DetectedIpod> {
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:\\", letter as char);
        let drive_path = std::path::Path::new(&drive);
        if !drive_path.exists() {
            continue;
        }
        if let Some(detected) = scan_drive_for_ipod(drive_path) {
            return Some(detected);
        }
    }
    None
}

/// Test-friendly variant: check a specific drive (or any path) for the
/// iPod_Control\Device\SysInfo file and read identity from it.
///
/// SysInfo recovery: iTunes reformats / restores can leave SysInfo as
/// a 0-byte file (the iPod's FirewireGuid is hardware-burnt and Apple
/// expects iTunes to repopulate the file on first pair, but ipod-sync
/// can't wait for that). When SysInfo lacks a FirewireGuid we extract
/// it from the Windows USB device path (the same value lives in the
/// USB descriptor as `iSerialNumber`) and write a synthetic SysInfo
/// so apply_loop's read_firewire_guid finds it and libgpod can sign
/// the iTunesDB on write.
pub fn scan_drive_for_ipod(drive: &std::path::Path) -> Option<DetectedIpod> {
    // F-09: require BOTH SysInfo and iTunesDB. A device with only SysInfo
    // is mid-restore (no DB written yet); the daemon would announce
    // "connected" but a sync attempt would fail at OwnedDb::open. The
    // unified predicate keeps daemon detection and CLI mount-detection
    // in agreement about what counts as an iPod.
    if !crate::ipod::layout::is_ipod_mount(drive) {
        return None;
    }
    let sysinfo = crate::ipod::layout::sysinfo_path(drive);
    let text = std::fs::read_to_string(&sysinfo).unwrap_or_default();

    // If on-disk SysInfo already carries identity (older firmware,
    // a gtkpod user before us, or a pre-fix install of ipod-sync
    // itself), use that — leave the file untouched.
    let mut serial = parse_sysinfo_field(&text, "FirewireGuid");
    let mut model_num = parse_sysinfo_field(&text, "ModelNumStr").unwrap_or_default();
    let mut model_label_override: Option<String> = None;

    let need_serial_recovery = serial.as_deref().map(str::is_empty).unwrap_or(true);
    let need_model_recovery = model_num.is_empty();

    if need_serial_recovery || need_model_recovery {
        // SysInfo doesn't carry the field(s) we need. This is the
        // common case on modern iPods — iTunes does NOT populate
        // SysInfo (it leaves the file 0 bytes and reads everything
        // from the device firmware via SCSI INQUIRY on demand). We
        // mirror iTunes' behavior: query SCSI for the authoritative
        // identity, fall back to the USB PID + capacity heuristic if
        // SCSI is unavailable, and **never write to SysInfo on
        // disk** — that file is iTunes' territory and writing to it
        // is what was breaking the iPod's ability to be re-managed
        // with iTunes.
        #[cfg(windows)]
        if let Some(letter) = drive_letter(drive) {
            if let Some(recovered) = recover_ipod_info_from_usb(letter) {
                tracing::info!(
                    "ipod: USB recovery for {}:\\ → guid={}, pid={:?}, capacity={:?} bytes, \
                     disk_number={:?}, heuristic_identity={:?}, scsi_xml={} bytes, scsi_parsed={}",
                    letter,
                    recovered.firewire_guid,
                    recovered.pid,
                    recovered.capacity_bytes,
                    recovered.disk_number,
                    recovered.identity,
                    recovered.sysinfo_extended_xml.as_deref().map(str::len).unwrap_or(0),
                    recovered.sysinfo_extended_parsed.is_some(),
                );
                if need_serial_recovery {
                    serial = Some(recovered.firewire_guid.clone());
                }
                if need_model_recovery {
                    // Preference order for ModelNumStr:
                    //   1. SCSI INQUIRY (authoritative — direct from
                    //      device firmware, matches what iTunes uses)
                    //   2. USB PID + capacity heuristic (works for
                    //      Classic + Nano via libgpod's table)
                    //   3. Legacy xPID_XXXX marker (still parseable
                    //      by describe_model but breaks libgpod's hash
                    //      path; only kicks in when both above fail)
                    if let Some(parsed) = &recovered.sysinfo_extended_parsed {
                        if let Some(mn) = &parsed.model_num_str {
                            model_num = mn.clone();
                            model_label_override = Some(describe_model(mn));
                        }
                    }
                    if model_num.is_empty() {
                        if let Some(identity) = recovered.identity {
                            model_num = identity.model_num.to_string();
                            model_label_override = Some(identity.label.to_string());
                        } else if let Some(pid) = recovered.pid {
                            model_num = format!("xPID_{:04X}", pid);
                        }
                    }
                }
                // NOTE: we do NOT call write_synthesized_sysinfo.
                // Modern iTunes leaves SysInfo as 0 bytes; we mirror
                // that. The identity is held in memory (DetectedIpod)
                // and fed to libgpod via set_sysinfo at apply time.
            }
        }
    }

    let serial = serial?;
    if serial.is_empty() { return None; }
    let model_label = model_label_override.unwrap_or_else(|| describe_model(&model_num));
    Some(DetectedIpod {
        serial,
        model_label,
        drive: drive.to_string_lossy().into_owned(),
        // Filled in by the daemon (or left as None) — iTunesDB parsing
        // is expensive and not needed for serial/model identification.
        name: None,
    })
}

/// Extract a leading drive letter from a path like `G:\` → `'G'`.
#[cfg(windows)]
fn drive_letter(drive: &std::path::Path) -> Option<char> {
    let s = drive.to_str()?;
    let first = s.chars().next()?;
    if first.is_ascii_alphabetic() { Some(first.to_ascii_uppercase()) } else { None }
}

/// Recovered identity for an iPod whose SysInfo file is empty.
///
/// We collect two independent pieces of information:
///
/// 1. **USB-derived identity** (`firewire_guid`, `pid`, `capacity_bytes`):
///    the heuristic fallback. PID + capacity together resolve to a
///    libgpod-recognised `IpodIdentity` for the well-known Classic and
///    Nano families.
///
/// 2. **SCSI-derived rich identity** (`sysinfo_extended_xml`): the
///    full `SysInfoExtended` Apple plist read directly from the
///    device firmware via SCSI INQUIRY VPD pages — the same mechanism
///    iTunes uses. When available this is **authoritative** — the
///    device tells us its exact ModelNumStr, SerialNumber, FamilyID,
///    artwork formats, etc. with no guessing. The caller prefers
///    this over the USB heuristic when both are present.
///
/// `disk_number` is the `N` in `\\.\PhysicalDriveN`, captured from
/// the same PowerShell query so a follow-up SCSI INQUIRY doesn't have
/// to re-resolve the volume.
#[cfg(windows)]
struct UsbIpodInfo {
    firewire_guid: String,
    pid: Option<u16>,
    capacity_bytes: Option<u64>,
    disk_number: Option<u32>,
    identity: Option<IpodIdentity>,
    sysinfo_extended_xml: Option<String>,
    sysinfo_extended_parsed: Option<crate::sysinfo_extended::ParsedSysInfo>,
}

/// `(ModelNumStr, friendly-label)` pair for a detected iPod. The
/// `model_num` MUST be a value libgpod's `ipod_info_table` in
/// `src/itdb_device.c` recognises — otherwise the hash58 codepath
/// silently degrades to `ITDB_CHECKSUM_NONE` and iTunes refuses the
/// resulting iTunesDB. `label` is the user-facing string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IpodIdentity {
    model_num: &'static str,
    label: &'static str,
}

/// Process-lifetime cache of SCSI INQUIRY attempts, keyed on
/// FirewireGuid. Stores either the successfully-read XML (so
/// subsequent polls reuse it without re-issuing the IOCTL) or the
/// error string (so subsequent polls don't keep hammering a known-
/// failing operation and flooding the log).
///
/// `OnceLock<Mutex<…>>` rather than `lazy_static!` to stay free of
/// macro deps; mutex lock is sub-microsecond and the cache is read
/// at most every few seconds.
#[cfg(windows)]
static SCSI_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Result<String, String>>>>
    = std::sync::OnceLock::new();

/// Resolve `(xml, parsed)` for the SCSI INQUIRY result against the
/// volume at `drive_letter`, consulting the per-FirewireGuid cache
/// first so repeated device-watcher polls don't re-issue an IOCTL
/// that's already known to fail (or succeed) for this device.
#[cfg(windows)]
fn scsi_inquiry_cached(
    drive_letter: char,
    firewire_guid: &str,
) -> (Option<String>, Option<crate::sysinfo_extended::ParsedSysInfo>) {
    let cache = SCSI_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));

    // Hot path: cache hit — no IOCTL, no PowerShell, no log.
    {
        let guard = cache.lock().expect("SCSI cache mutex poisoned");
        if let Some(entry) = guard.get(firewire_guid) {
            return match entry {
                Ok(xml) => {
                    let parsed = crate::sysinfo_extended::ParsedSysInfo::from_xml(xml).ok();
                    (Some(xml.clone()), parsed)
                }
                Err(_) => (None, None),
            };
        }
    }

    // Cold path: first attempt for this device. Log once — success
    // OR failure — so the daemon log records what happened, then
    // store the result for future polls.
    let result = crate::scsi_inquiry::read_sysinfo_extended(drive_letter);
    let cache_value: Result<String, String> = match &result {
        Ok(xml) => {
            tracing::info!(
                "ipod: SCSI INQUIRY succeeded for {firewire_guid} ({} bytes); cached for this session",
                xml.len(),
            );
            Ok(xml.clone())
        }
        Err(e) => {
            tracing::info!(
                "ipod: SCSI INQUIRY unavailable for {firewire_guid} ({e}); falling back to USB \
                 heuristic and caching the failure so we don't retry every poll"
            );
            Err(format!("{e}"))
        }
    };
    cache.lock().expect("SCSI cache mutex poisoned")
        .insert(firewire_guid.to_string(), cache_value);

    match result {
        Ok(xml) => {
            let parsed = crate::sysinfo_extended::ParsedSysInfo::from_xml(&xml).ok();
            (Some(xml), parsed)
        }
        Err(_) => (None, None),
    }
}

/// Ask Windows for the USB device path of the volume mounted at
/// `<letter>:\` and extract both the 16-hex-digit FirewireGuid and
/// the USB Product ID.
///
/// Apple iPods burn the FirewireGuid into the USB device descriptor
/// as the iSerialNumber, which Windows surfaces in two places:
///   - the storage-class path under `\\?\usbstor#…` (where the
///     FirewireGuid sits between `&` separators), and
///   - the USB-class path under `USB\VID_05AC&PID_XXXX\<guid>`
///     (where the PID identifies the specific model).
///
/// We query both via one PowerShell call so the daemon stays free of
/// Win32 storage-API bindings. Recovery only runs on iPod plug-in
/// when SysInfo is empty, so the ~500ms shell-out is acceptable.
#[cfg(windows)]
fn recover_ipod_info_from_usb(drive_letter: char) -> Option<UsbIpodInfo> {
    // Single script returning four pipe-separated fields:
    //   1. disk path  (\\?\usbstor#disk&ven_apple&… — contains the
    //                  16-hex FirewireGuid)
    //   2. usb path   (USB\VID_05AC&PID_XXXX\… — gives us the PID)
    //   3. disk size  (raw bytes — used to disambiguate Classic 1G/2G/3G)
    //   4. disk number (the N in \\.\PhysicalDriveN — used by
    //                   scsi_inquiry to open the device for SCSI
    //                   INQUIRY VPD pass-through, which returns the
    //                   real ModelNumStr direct from device firmware)
    let script = format!(
        "$disk = Get-Volume -DriveLetter {0} | Get-Partition | Get-Disk; \
         $diskPath = $disk.Path; \
         $diskSize = $disk.Size; \
         $diskNumber = $disk.Number; \
         $usbPath = ''; \
         if ($diskPath -match '[0-9a-fA-F]{{16}}') {{ \
             $guid = $matches[0]; \
             $usbPath = (Get-CimInstance Win32_PnPEntity -Filter \"Service='AppleIPod'\" | \
                         Where-Object {{ $_.PNPDeviceID -like \"*$guid*\" }} | \
                         Select-Object -First 1 -ExpandProperty PNPDeviceID); \
         }} \
         Write-Output \"$diskPath|$usbPath|$diskSize|$diskNumber\"",
        drive_letter
    );
    use crate::windows_proc::NoConsoleWindow;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .no_console()
        .output()
        .ok()?;
    if !output.status.success() { return None; }
    let combined = String::from_utf8_lossy(&output.stdout);
    let mut parts = combined.split('|');
    let disk_path = parts.next()?;
    let usb_path = parts.next().unwrap_or("");
    let size_str = parts.next().unwrap_or("").trim();
    let disk_number_str = parts.next().unwrap_or("").trim();

    let firewire_guid = extract_firewire_guid_from_usb_path(disk_path)?;
    let pid = extract_pid_from_apple_usb_path(usb_path);
    let capacity_bytes = size_str.parse::<u64>().ok();
    let disk_number = disk_number_str.parse::<u32>().ok();
    let identity = pid.and_then(|p| identify_ipod(p, capacity_bytes));

    // Try the authoritative SCSI INQUIRY path, with a per-process
    // per-FirewireGuid cache so repeated polls don't re-attempt a
    // privileged IOCTL that's already known to be permission-denied.
    // Without this, the device-watcher's ~2s poll cadence would
    // reissue the IOCTL forever and flood the log; with this, the
    // attempt runs at most once per device per daemon lifetime.
    let (sysinfo_extended_xml, sysinfo_extended_parsed) =
        scsi_inquiry_cached(drive_letter, &firewire_guid);

    Some(UsbIpodInfo {
        firewire_guid,
        pid,
        capacity_bytes,
        disk_number,
        identity,
        sysinfo_extended_xml,
        sysinfo_extended_parsed,
    })
}

/// Parse the 4-hex-digit USB Product ID out of an Apple USB device
/// instance path like `USB\VID_05AC&PID_1261\000A27002138B0A8`.
/// Case-insensitive on the `PID_` token because Windows surfaces the
/// instance ID in either case depending on which API the caller used.
fn extract_pid_from_apple_usb_path(path: &str) -> Option<u16> {
    let upper = path.to_ascii_uppercase();
    let needle = "PID_";
    let start = upper.find(needle)? + needle.len();
    let end = start + 4;
    if end > upper.len() { return None; }
    let hex = &upper[start..end];
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) { return None; }
    u16::from_str_radix(hex, 16).ok()
}

/// Resolve `(ModelNumStr, friendly label)` for a detected iPod from
/// its USB Product ID + raw disk capacity in bytes.
///
/// # Policy
///
/// **`model_num` is always a real Apple value that libgpod's
/// `ipod_info_table` recognises** (cross-checked against
/// `src/itdb_device.c` in github.com/gtkpod/libgpod). If we'd have to
/// invent one — e.g. iPod Nano 7G, which Apple shipped in 2012 and
/// libgpod never added support for — this function returns `None` so
/// the caller falls back to the legacy `xPID_XXXX` marker rather than
/// writing a value that looks real but trips libgpod's UNKNOWN path.
///
/// **`label` only claims what USB actually tells us.** We can know
/// the device family (Classic/Nano/Mini/Shuffle/Video/Photo) from the
/// USB PID. We can sometimes know the capacity from the raw disk
/// size. We *cannot* know color or exact SKU. The label reflects only
/// what's truly determined; ambiguous cases get the more general
/// label, not a confident-sounding guess.
///
/// **Capacity-based disambiguation runs only where it changes the
/// answer.** Within an iPod Classic, capacity tells 80GB-1G from
/// 120GB-2G; 160GB stays ambiguous between 1G-thick (2007) and
/// 3G-thin (2009) — we pick the 3G ModelNumStr (MC293) as a hash-
/// neutral default (both 1G and 3G map to `ITDB_CHECKSUM_HASH58` in
/// libgpod) and label it just "iPod Classic (160GB)" without claiming
/// a generation. For Nano/Shuffle/etc., the USB PID already gives the
/// generation and capacity doesn't change the hash path within a
/// generation, so we don't bother capacity-disambiguating those (the
/// label stays generation-only — accurate, not presumptuous about
/// capacity or color).
///
/// # ModelNumStr "default SKU" convention
///
/// For each (PID, capacity) bucket we pick ONE specific ModelNumStr
/// from libgpod's table. We default to the silver / white / smallest-
/// capacity SKU because:
/// - libgpod groups by generation for hash computation — all SKUs in
///   a generation produce the same hash path (HASH58 vs HASH72 vs
///   HASHAB vs none), so the specific choice within a generation is
///   functionally irrelevant for signing.
/// - The label we surface to the user doesn't claim color anyway.
/// - iTunes (if it cross-checks ModelNumStr against any USB descriptor
///   field) may care about capacity-bucket accuracy. We address that
///   via capacity-based disambiguation where applicable.
///
/// Capacity tolerance bands: Apple's marketed sizes map to raw disk
/// reports of ~size×10⁹ bytes. We use generous bands so a slightly-
/// under-spec drive (formatting overhead, firmware partition, retired
/// block remapping) still classifies correctly.
fn identify_ipod(pid: u16, capacity_bytes: Option<u64>) -> Option<IpodIdentity> {
    let gb = capacity_bytes.map(|b| b / 1_000_000_000); // marketed decimal GB

    match pid {
        // === iPod Classic family (PID shared across all three gens) ===
        // libgpod CLASSIC_1/2/3 all map to ITDB_CHECKSUM_HASH58, so a
        // wrong-within-Classic SKU still produces a valid hash. The
        // capacity tells us 1G vs 2G unambiguously; 160GB stays
        // ambiguous between thick 1G and thin 3G and the label
        // reflects that.
        0x1261 => Some(match gb {
            Some(g) if g < 100 => IpodIdentity {
                model_num: "MB029",  // CLASSIC_1 80GB silver
                label: "iPod Classic (1st gen, 80GB)",
            },
            Some(g) if g < 140 => IpodIdentity {
                model_num: "MB562",  // CLASSIC_2 120GB silver
                label: "iPod Classic (2nd gen, 120GB)",
            },
            Some(_) => IpodIdentity {
                model_num: "MC293",  // CLASSIC_3 160GB silver — hash-neutral
                                     // pick (CLASSIC_1 thick 160GB also valid)
                label: "iPod Classic (160GB)",  // no gen claim — ambiguous
            },
            None => return None,  // No capacity → can't safely pick a
                                  // SKU; fall back to legacy xPID marker
        }),

        // === iPod Nano family ===
        // PID identifies the generation unambiguously, so we don't
        // need capacity to pick the right hash path. ModelNumStr is
        // the silver SKU (smallest-capacity variant) from libgpod's
        // table; the label stays generation-only, not claiming a
        // capacity or color we can't determine from USB.
        // libgpod hash support: NANO_3/4 use hash58 (supported here);
        // NANO_5 uses hash72 and NANO_6 uses hashAB (libgpod can't
        // sign these correctly — iTunes will reject the resulting DB
        // regardless of what we put in SysInfo, but we still set a
        // real ModelNumStr so the UI is honest).
        0x1240 => Some(IpodIdentity { model_num: "A350", label: "iPod Nano (1st gen)" }),
        0x1260 => Some(IpodIdentity { model_num: "A477", label: "iPod Nano (2nd gen)" }),
        0x1262 => Some(IpodIdentity { model_num: "A978", label: "iPod Nano (3rd gen)" }),
        0x1263 => Some(IpodIdentity { model_num: "B480", label: "iPod Nano (4th gen)" }),
        0x1265 => Some(IpodIdentity { model_num: "C027", label: "iPod Nano (5th gen)" }),
        0x1266 => Some(IpodIdentity { model_num: "C525", label: "iPod Nano (6th gen)" }),
        // Nano 7G (PID 0x1267, 2012-2017): Apple ModelNumStrs are
        // D376/D744/etc., NONE of which appear in libgpod's table.
        // Returning a fake ModelNumStr would be worse than returning
        // None — at least None lets the caller fall back to xPID_1267
        // which describe_model can still render as "iPod Nano (7th
        // gen)" via the legacy path. The functional outcome is the
        // same (libgpod can't sign for Nano 7G either way), but the
        // honest fallback doesn't pollute SysInfo with a value Apple
        // never assigned.

        // === iPod Mini / Shuffle / Video / Photo / Original ===
        // All use hash type "none" in libgpod (these older iPods
        // don't sign the iTunesDB at all). The ModelNumStr affects
        // only libgpod's model metadata + the UI label — never the
        // hash path — so the silver/smallest-capacity SKU pick is
        // safe.
        0x1205 => Some(IpodIdentity {
            model_num: "9160",  // MINI_1 4GB silver (PID also serves MINI_2)
            label: "iPod Mini",  // no gen claim — PID shared 1G/2G
        }),
        0x1209 => Some(IpodIdentity {
            model_num: "A002",  // VIDEO_1 30GB white
            label: "iPod Video (5th gen)",
        }),
        0x1206 => Some(IpodIdentity {
            model_num: "A444",  // VIDEO_2 30GB white
            label: "iPod Video (5.5 gen)",
        }),
        0x1204 => Some(IpodIdentity {
            model_num: "9829",  // PHOTO 30GB
            label: "iPod Photo",
        }),
        // PID 0x1202 covers iPod 1G + 2G (no in-USB distinguisher).
        // PID 0x1201 = iPod 3G. PID 0x1203 = iPod 4G. We pick a 1G
        // ModelNumStr as the catch-all (8513 = 1G 5GB) — none of
        // these generations sign the DB so the choice is cosmetic.
        // Caller's xPID fallback covers the rare case where we want
        // more precision later.
        0x1202 => Some(IpodIdentity {
            model_num: "8513",  // FIRST 5GB (PID shared 1G/2G)
            label: "iPod (1st/2nd gen)",
        }),
        0x1201 => Some(IpodIdentity {
            model_num: "8976",  // THIRD 10GB
            label: "iPod (3rd gen)",
        }),
        0x1203 => Some(IpodIdentity {
            model_num: "9282",  // FOURTH 20GB
            label: "iPod (4th gen)",
        }),
        0x1300 => Some(IpodIdentity {
            model_num: "9724",  // SHUFFLE_1 512MB
            label: "iPod Shuffle (1st gen)",
        }),
        0x1301 => Some(IpodIdentity {
            model_num: "A546",  // SHUFFLE_2 1GB silver
            label: "iPod Shuffle (2nd gen)",
        }),
        0x1302 => Some(IpodIdentity {
            model_num: "C306",  // SHUFFLE_3 2GB silver
            label: "iPod Shuffle (3rd gen)",
        }),
        0x1303 => Some(IpodIdentity {
            model_num: "C584",  // SHUFFLE_4 2GB silver
            label: "iPod Shuffle (4th gen)",
        }),

        _ => None,
    }
}

/// Find a 16-hex-digit run inside a Windows USB device path and
/// format it as the canonical `0x...` FirewireGuid string.
///
/// Anchors on word-boundary-like checks (hex chars not adjacent on
/// either side) so we don't accidentally lop a 16-char substring out
/// of a longer hex sequence elsewhere in the path.
fn extract_firewire_guid_from_usb_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 16 { return None; }
    for start in 0..=bytes.len() - 16 {
        let window = &bytes[start..start + 16];
        if !window.iter().all(|b| b.is_ascii_hexdigit()) { continue; }
        if start > 0 && bytes[start - 1].is_ascii_hexdigit() { continue; }
        if start + 16 < bytes.len() && bytes[start + 16].is_ascii_hexdigit() { continue; }
        let hex = std::str::from_utf8(window).ok()?;
        return Some(format!("0x{}", hex.to_uppercase()));
    }
    None
}

/// Strict `Key: value` parser for the iPod's flat-text SysInfo file.
///
/// Matches the exact key (case-sensitive — matches how iTunes writes it).
/// Lines where the key is a mere prefix of `key` (e.g. `FirewireGuidSomething`
/// when searching for `FirewireGuid`) are skipped — see test
/// `ignores_lines_starting_with_firewire_guid_prefix_but_not_exact_key`.
fn parse_sysinfo_field(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        if k.trim() == key {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// Best-effort human-friendly label from a ModelNumStr value found in
/// SysInfo. Recognises real Apple model numbers we write via
/// `identify_ipod` plus the legacy `xPID_XXXX` round-trip marker from
/// older builds (so a SysInfo carried over from a pre-fix install
/// still produces a sane UI label without re-shelling). M5 will
/// replace this with libgpod's full model lookup.
fn describe_model(model_num: &str) -> String {
    let upper = model_num.trim_start_matches('x').to_uppercase();
    if let Some(hex) = upper.strip_prefix("PID_") {
        if let Ok(pid) = u16::from_str_radix(hex, 16) {
            // Legacy xPID_XXXX marker. We don't have a capacity hint
            // in this read-side path, so we use a family-only lookup
            // that returns the most general (non-capacity-claiming)
            // label for the PID — never invents a generation when one
            // can't be determined.
            return family_label_for_pid(pid)
                .map(str::to_string)
                .unwrap_or_else(|| format!("iPod (PID {:#06X})", pid));
        }
    }
    match upper.as_str() {
        "MB029" | "MB147" | "MB145" | "MB150" => "iPod Classic (1st gen)".to_string(),
        "MB562" | "MB565" => "iPod Classic (2nd gen)".to_string(),
        "MC293" | "MC297" => "iPod Classic (3rd gen)".to_string(),
        _ if !upper.is_empty() => format!("iPod ({upper})"),
        _ => "iPod (model unknown)".to_string(),
    }
}

/// Family-only friendly label for a USB PID, used by `describe_model`
/// when reading a legacy `xPID_XXXX` marker out of SysInfo. Unlike
/// `identify_ipod`, this never claims a generation we can't determine
/// from the PID alone — for shared-PID families (iPod Classic 1G/2G/3G,
/// iPod Mini 1G/2G) it returns the family name without a generation
/// number.
fn family_label_for_pid(pid: u16) -> Option<&'static str> {
    match pid {
        0x1261 => Some("iPod Classic"),
        0x1240 => Some("iPod Nano (1st gen)"),
        0x1260 => Some("iPod Nano (2nd gen)"),
        0x1262 => Some("iPod Nano (3rd gen)"),
        0x1263 => Some("iPod Nano (4th gen)"),
        0x1265 => Some("iPod Nano (5th gen)"),
        0x1266 => Some("iPod Nano (6th gen)"),
        0x1267 => Some("iPod Nano (7th gen)"),
        0x1205 => Some("iPod Mini"),
        0x1209 => Some("iPod Video (5th gen)"),
        0x1206 => Some("iPod Video (5.5 gen)"),
        0x1204 => Some("iPod Photo"),
        0x1202 => Some("iPod (1st/2nd gen)"),
        0x1201 => Some("iPod (3rd gen)"),
        0x1203 => Some("iPod (4th gen)"),
        0x1300 => Some("iPod Shuffle (1st gen)"),
        0x1301 => Some("iPod Shuffle (2nd gen)"),
        0x1302 => Some("iPod Shuffle (3rd gen)"),
        0x1303 => Some("iPod Shuffle (4th gen)"),
        _ => None,
    }
}

/// Enumerate Windows drive letters A-Z, find drives that look like an iPod
/// (have `iPod_Control\iTunes\iTunesDB`), and return the unique mount.
pub fn detect_ipod_mount() -> Result<String> {
    let candidates = candidate_drives()
        .into_iter()
        .filter(looks_like_ipod)
        .collect();
    pick_mount(candidates)
}

/// Return all currently-existing drive letters A:\\ through Z:\\.
fn candidate_drives() -> Vec<String> {
    ('A'..='Z')
        .map(|c| format!("{c}:\\"))
        .filter(|d| std::path::Path::new(d).exists())
        .collect()
}

/// True if `drive` looks like a mounted iPod. Uses the canonical predicate
/// (both SysInfo and iTunesDB present); see findings F-09.
fn looks_like_ipod(drive: &String) -> bool {
    crate::ipod::layout::is_ipod_mount(std::path::Path::new(drive))
}

/// Given a set of iPod-looking mounts, return the unique one or an error.
fn pick_mount(mounts: Vec<String>) -> Result<String> {
    match mounts.len() {
        0 => Err(anyhow!(
            "no iPod found mounted on any drive. Plug in the iPod (or pass --ipod <drive>)."
        )),
        1 => Ok(mounts.into_iter().next().unwrap()),
        _ => Err(anyhow!(
            "multiple iPod-like drives found: {}. Pass --ipod <drive> to disambiguate.",
            mounts.join(", ")
        )),
    }
}

#[cfg(test)]
mod detection_tests {
    use super::*;

    #[test]
    fn pick_mount_single_match() {
        let mounts = vec!["G:\\".to_string()];
        let mount = pick_mount(mounts).unwrap();
        assert_eq!(mount, "G:\\");
    }

    #[test]
    fn pick_mount_no_match_errors() {
        let err = pick_mount(vec![]).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("no ipod"));
    }

    #[test]
    fn pick_mount_multiple_matches_errors() {
        let mounts = vec!["E:\\".to_string(), "G:\\".to_string()];
        let err = pick_mount(mounts).unwrap_err();
        assert!(err.to_string().contains("E:") && err.to_string().contains("G:"),
            "error message must enumerate the candidates");
        assert!(err.to_string().contains("--ipod"),
            "error must hint at --ipod flag");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../../tests/fixtures/sample-sysinfo.txt");

    #[test]
    fn extracts_firewire_guid_from_real_sample() {
        let guid = extract_firewire_guid(SAMPLE).expect("extract");
        // Classic uses a 16-hex-digit ID with 0x prefix.
        assert!(guid.starts_with("0x"), "expected hex prefix, got: {guid}");
        assert_eq!(guid.len(), 18, "expected 0x + 16 hex chars, got len {}: {guid}", guid.len());
        assert!(guid[2..].chars().all(|c| c.is_ascii_hexdigit()),
            "expected hex digits, got: {guid}");
    }

    #[test]
    fn errors_on_missing_key() {
        let sysinfo = "ModelNumStr: MB029\nOther: value\n";
        assert!(extract_firewire_guid(sysinfo).is_err());
    }

    #[test]
    fn errors_on_missing_value() {
        let sysinfo = "FirewireGuid:\nModelNumStr: MB029\n";
        assert!(extract_firewire_guid(sysinfo).is_err());
    }

    #[test]
    fn ignores_lines_starting_with_firewire_guid_prefix_but_not_exact_key() {
        let sysinfo = "FirewireGuidSomething: 0xDEADBEEF\nFirewireGuid: 0x000A27002138B0A8\n";
        assert_eq!(
            extract_firewire_guid(sysinfo).unwrap(),
            "0x000A27002138B0A8"
        );
    }

    #[test]
    fn scan_for_ipod_returns_none_when_no_drives_match() {
        let tmp = std::env::temp_dir().join(format!("ipod-scan-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let result = scan_drive_for_ipod(&tmp);
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn scan_drive_for_ipod_detects_serial_when_both_files_present() {
        // F-09: scan_drive_for_ipod requires BOTH SysInfo (for identity)
        // AND iTunesDB (proves it's syncable). A device with only one is
        // mid-restore or corrupted — we don't try to sync to it.
        let tmp = std::env::temp_dir().join(format!("ipod-scan-found-test-{}", std::process::id()));
        let sysinfo_dir = tmp.join("iPod_Control").join("Device");
        let itunes_dir = tmp.join("iPod_Control").join("iTunes");
        std::fs::create_dir_all(&sysinfo_dir).unwrap();
        std::fs::create_dir_all(&itunes_dir).unwrap();
        std::fs::write(
            sysinfo_dir.join("SysInfo"),
            "FirewireGuid: 0xEXAMPLE1234\nModelNumStr: xMB029\n",
        ).unwrap();
        std::fs::write(itunes_dir.join("iTunesDB"), b"").unwrap();
        let detected = scan_drive_for_ipod(&tmp).expect("should detect");
        assert_eq!(detected.serial, "0xEXAMPLE1234");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn extracts_firewire_guid_from_typical_usb_device_path() {
        let path = r"\\?\usbstor#disk&ven_apple&prod_ipod&rev_1.62#a&bf8ed55&0&000a27002138b0a8&0#{53f56307-b6bf-11d0-94f2-00a0c91efb8b}";
        let guid = extract_firewire_guid_from_usb_path(path).expect("should extract");
        assert_eq!(guid, "0x000A27002138B0A8");
    }

    #[test]
    fn extracts_firewire_guid_uppercases_hex() {
        let path = "ven_apple#000a27002138b0a8&0";
        let guid = extract_firewire_guid_from_usb_path(path).expect("should extract");
        assert!(guid.starts_with("0x"));
        assert!(guid[2..].chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
    }

    #[test]
    fn does_not_extract_15_or_17_hex_digits_as_firewire_guid() {
        // Too short.
        assert!(extract_firewire_guid_from_usb_path("abc&00a27002138b0a8&0").is_none());
        // Too long (17 hex) — anchor check rejects substrings of longer runs.
        assert!(extract_firewire_guid_from_usb_path("abc&00a27002138b0a8ff&0").is_none());
    }

    #[test]
    fn ignores_short_hex_runs_in_path() {
        let path = r"\\?\foo#bar&0&abc&def";
        assert!(extract_firewire_guid_from_usb_path(path).is_none());
    }

    #[test]
    fn extracts_pid_from_apple_usb_path() {
        let path = r"USB\VID_05AC&PID_1261\000A27002138B0A8";
        assert_eq!(extract_pid_from_apple_usb_path(path), Some(0x1261));
    }

    #[test]
    fn extracts_pid_handles_lowercase_hex() {
        let path = r"usb\vid_05ac&pid_1265\deadbeefdeadbeef";
        assert_eq!(extract_pid_from_apple_usb_path(path), Some(0x1265));
    }

    #[test]
    fn pid_extraction_rejects_non_hex() {
        assert!(extract_pid_from_apple_usb_path(r"USB\VID_05AC&PID_XYZW\abc").is_none());
    }

    /// Apple uses a single USB PID (0x1261) for ALL iPod Classic
    /// generations (1G/2G/3G), so capacity disambiguates 80GB/120GB.
    /// 160GB is intentionally NOT pinned to a generation — both thick
    /// 1G (2007, MB145/MB150) and thin 3G (2009, MC293/MC297) shipped
    /// at 160GB and USB carries no way to tell them apart. The label
    /// must NOT lie about which one it is.
    #[test]
    fn classic_pid_disambiguates_by_capacity() {
        let id_80 = identify_ipod(0x1261, Some(80 * 1_000_000_000)).unwrap();
        assert_eq!(id_80.model_num, "MB029");
        assert_eq!(id_80.label, "iPod Classic (1st gen, 80GB)");

        let id_120 = identify_ipod(0x1261, Some(120 * 1_000_000_000)).unwrap();
        assert_eq!(id_120.model_num, "MB562");
        assert_eq!(id_120.label, "iPod Classic (2nd gen, 120GB)");

        let id_160 = identify_ipod(0x1261, Some(160 * 1_000_000_000)).unwrap();
        assert_eq!(id_160.model_num, "MC293",
            "160GB defaults to 3G ModelNumStr (hash-neutral with 1G thick)");
        assert_eq!(id_160.label, "iPod Classic (160GB)",
            "label must NOT claim a specific generation — we genuinely can't tell");
    }

    /// Classic PID without a capacity hint cannot safely pick a
    /// generation-specific SKU. Returning None lets the caller fall
    /// back to the legacy xPID marker rather than writing a confident-
    /// looking-but-wrong ModelNumStr into SysInfo.
    #[test]
    fn classic_pid_without_capacity_returns_none() {
        assert!(identify_ipod(0x1261, None).is_none());
    }

    /// PID disambiguation for the Nano family — fixes a previous bug
    /// where 0x1263 and 0x1265 were mislabeled as Classic SKUs. PID
    /// gives the generation; we don't have a way to know capacity or
    /// color per SKU, so the label is generation-only (honest).
    /// 0x1263 = Nano 4G (hash58, fully supported), 0x1265 = Nano 5G
    /// (hash72, not supported by libgpod's free codepath but still
    /// gets the right UI label).
    #[test]
    fn nano_pids_resolve_to_correct_family() {
        assert_eq!(identify_ipod(0x1240, None).unwrap().label, "iPod Nano (1st gen)");
        assert_eq!(identify_ipod(0x1260, None).unwrap().label, "iPod Nano (2nd gen)");
        assert_eq!(identify_ipod(0x1262, None).unwrap().label, "iPod Nano (3rd gen)");
        assert_eq!(identify_ipod(0x1263, None).unwrap().label, "iPod Nano (4th gen)");
        assert_eq!(identify_ipod(0x1265, None).unwrap().label, "iPod Nano (5th gen)");
        assert_eq!(identify_ipod(0x1266, None).unwrap().label, "iPod Nano (6th gen)");
    }

    /// Nano 7G (PID 0x1267, 2012) is intentionally absent from the
    /// table: Apple's ModelNumStrs for it (D376/D744/etc.) don't
    /// appear in libgpod's `ipod_info_table`. Writing a fake value
    /// would silently take libgpod down its UNKNOWN-checksum path,
    /// indistinguishable from the legacy xPID marker. None preserves
    /// honest fallback.
    #[test]
    fn nano_7g_not_in_libgpod_table_returns_none() {
        assert!(identify_ipod(0x1267, None).is_none());
    }

    /// Unknown PIDs return None so the caller falls back to the
    /// xPID_XXXX legacy marker. iPod Touch (0x129E) is intentionally
    /// not in the table — it's outside ipod-sync's scope per SPEC §7.
    #[test]
    fn unknown_pids_return_none() {
        assert!(identify_ipod(0xFFFF, None).is_none());
        assert!(identify_ipod(0x129E, None).is_none()); // iPod Touch — out of scope
    }

    #[test]
    fn describe_model_recognises_real_classic_model_nums() {
        // Real Apple ModelNumStr values our synthesiser writes after
        // the model_num_for_pid lookup. These are what end up on disk
        // (and what libgpod consumes for its hash58 path); the friendly
        // label here is what the UI shows back to the user.
        assert_eq!(describe_model("MB029"), "iPod Classic (1st gen)");
        assert_eq!(describe_model("MB565"), "iPod Classic (2nd gen)");
        assert_eq!(describe_model("MC293"), "iPod Classic (3rd gen)");
    }

    #[test]
    fn describe_model_round_trips_legacy_pid_marker() {
        // Older versions of ipod-sync wrote `xPID_XXXX` into synthetic
        // SysInfo when no real ModelNumStr was known; describe_model
        // continues to round-trip those for back-compat with iPods
        // that still have a synthetic SysInfo on disk from a prior
        // build (also the current fallback path for PIDs outside the
        // identify_ipod table). describe_model has no capacity hint
        // in this codepath so it returns the no-capacity Classic
        // default label, not a specific-generation label.
        assert!(describe_model("xPID_1261").starts_with("iPod Classic"));
        assert_eq!(describe_model("xPID_1265"), "iPod Nano (5th gen)");
        assert!(describe_model("xpid_1261").starts_with("iPod Classic"));
        assert!(describe_model("xPID_FFFF").starts_with("iPod ("));
    }

    #[test]
    fn scan_drive_for_ipod_rejects_sysinfo_without_itunes_db() {
        // F-09 regression: a half-restored device with SysInfo but no
        // iTunesDB must NOT be reported as a syncable iPod.
        let tmp = std::env::temp_dir().join(format!("ipod-scan-partial-test-{}", std::process::id()));
        let sysinfo_dir = tmp.join("iPod_Control").join("Device");
        std::fs::create_dir_all(&sysinfo_dir).unwrap();
        std::fs::write(
            sysinfo_dir.join("SysInfo"),
            "FirewireGuid: 0xEXAMPLE1234\nModelNumStr: xMB029\n",
        ).unwrap();
        assert!(scan_drive_for_ipod(&tmp).is_none(),
            "SysInfo without iTunesDB must not be classified as an iPod");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
