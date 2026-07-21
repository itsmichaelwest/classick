//! Read FirewireGUID from the iPod's SysInfo and push it into libgpod's
//! device struct so itdb_write computes a valid signed iTunesDB.

use anyhow::{anyhow, Result};
use std::ffi::{CStr, CString};
use std::path::Path;

use crate::device::OrdinaryUsbFacts;
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
    let body =
        std::fs::read_to_string(&path).map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
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
pub unsafe fn set_firewire_guid(device: *mut ffi::Itdb_Device, guid: &str) -> Result<()> {
    if device.is_null() {
        return Err(anyhow!("Itdb_Device pointer is NULL"));
    }
    const KEY: &CStr = c"FirewireGuid";
    let value =
        CString::new(guid).map_err(|_| anyhow!("FirewireGuid contains interior NUL byte"))?;
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
pub unsafe fn set_model_num(device: *mut ffi::Itdb_Device, model_num: &str) -> Result<()> {
    if device.is_null() {
        return Err(anyhow!("Itdb_Device pointer is NULL"));
    }
    const KEY: &CStr = c"ModelNumStr";
    let value =
        CString::new(model_num).map_err(|_| anyhow!("ModelNumStr contains interior NUL byte"))?;
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

/// Resolve the identity libgpod needs to sign the iTunesDB from valid
/// pre-existing SysInfo fields plus ordinary USB enumeration. The USB
/// PID/capacity model selection is a compatibility adapter for signing,
/// not a reported hardware fact.
///
/// Returns an error if neither source can produce a
/// FirewireGuid — at which point libgpod can't sign anything and
/// the sync would be DOA regardless.
pub fn resolve_libgpod_identity(ipod_mount: &Path) -> Result<LibgpodIdentity> {
    let sysinfo_path = crate::ipod::layout::sysinfo_path(ipod_mount);
    let sysinfo_text = std::fs::read_to_string(&sysinfo_path).unwrap_or_default();
    let disk_guid = parse_sysinfo_field(&sysinfo_text, "FirewireGuid").filter(|s| !s.is_empty());
    let disk_model = parse_sysinfo_field(&sysinfo_text, "ModelNumStr").filter(|s| !s.is_empty());

    if let (Some(guid), Some(model_num_str)) = (disk_guid.clone(), disk_model.clone()) {
        return Ok(LibgpodIdentity {
            firewire_guid: guid,
            model_num_str,
        });
    }

    let recovered = ordinary_usb_facts_for_mount(ipod_mount)
        .ok_or_else(|| anyhow!("USB recovery failed for {}", ipod_mount.display()))?;
    let firewire_guid = disk_guid
        .or(recovered.raw_usb_iserial)
        .ok_or_else(|| anyhow!("USB identity unavailable for {}", ipod_mount.display()))?;
    let model_num_str = disk_model
        .or_else(|| {
            recovered
                .usb_product_id
                .and_then(|pid| identify_ipod(pid, recovered.capacity_bytes))
                .map(|identity| identity.model_num.to_owned())
        })
        .ok_or_else(|| {
            anyhow!(
                "could not determine ModelNumStr for iPod at {} (PID {:?}, capacity {:?} bytes)",
                ipod_mount.display(),
                recovered.usb_product_id,
                recovered.capacity_bytes,
            )
        })?;
    Ok(LibgpodIdentity {
        firewire_guid,
        model_num_str,
    })
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
    /// Windows volume GUID for this mount, in `\\?\Volume{GUID}\` form.
    /// Stable across drive-letter reassignments and unplug/replug
    /// cycles, so the daemon's polling watcher can fast-path subsequent
    /// observations: resolve the cached GUID → current mount path with
    /// one Win32 call, vs. re-walking every present volume + re-reading
    /// SysInfo. `None` on non-Windows (no native enumeration yet) and
    /// when the volume GUID query failed (rare — only on permission
    /// errors or unmount races).
    pub volume_guid: Option<String>,
}

/// Scan for every mounted iPod (presence of both `iPod_Control\Device\SysInfo`
/// AND `iPod_Control\iTunes\iTunesDB` — see `is_ipod_mount`).
///
/// Uses [`candidate_mount_points`] for enumeration, which on Windows
/// natively asks the OS which drive letters are present + removable/fixed
/// (no per-letter `Path::exists()` probe, no walking through 26 missing
/// letters every poll, no hanging on slow network mounts).
pub fn scan_for_ipods() -> Vec<DetectedIpod> {
    let mut detected: Vec<_> = candidate_mount_points()
        .into_iter()
        .filter_map(|mount| scan_drive_for_ipod(&mount))
        .collect();
    detected.sort_by(|left, right| {
        left.serial
            .cmp(&right.serial)
            .then_with(|| left.drive.cmp(&right.drive))
    });
    detected
}

/// Compatibility wrapper for callers that can only work with one device.
/// The returned device is deterministic because [`scan_for_ipods`] sorts its
/// result by serial and mount path.
pub fn scan_for_ipod() -> Option<DetectedIpod> {
    scan_for_ipods().into_iter().next()
}

/// Test-friendly variant: check a specific drive (or any path) for the
/// iPod_Control\Device\SysInfo file and read identity from it.
///
/// Modern Apple software can leave SysInfo empty. In that case the legacy v2
/// adapter recovers the FirewireGuid from ordinary USB enumeration and may use
/// PID plus capacity to select a signing-only libgpod model. It never writes
/// the inferred model or recovered identity to SysInfo.
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
    // a gtkpod user before us, or a pre-fix install of classick
    // itself), use that — leave the file untouched.
    let mut serial = parse_sysinfo_field(&text, "FirewireGuid");
    let mut model_num = parse_sysinfo_field(&text, "ModelNumStr").unwrap_or_default();
    let mut model_label_override: Option<String> = None;

    let need_serial_recovery = serial.as_deref().map(str::is_empty).unwrap_or(true);
    let need_model_recovery = model_num.is_empty();

    if need_serial_recovery || need_model_recovery {
        if let Some(recovered) = ordinary_usb_facts_for_mount(drive) {
            let identity = recovered
                .usb_product_id
                .and_then(|pid| identify_ipod(pid, recovered.capacity_bytes));
            tracing::info!(
                "ipod: ordinary USB recovery for {} → identity_available={}, pid={:?}, \
                 capacity={:?} bytes, signing_identity={:?}",
                drive.display(),
                recovered.raw_usb_iserial.is_some(),
                recovered.usb_product_id,
                recovered.capacity_bytes,
                identity,
            );
            if need_serial_recovery {
                serial = recovered.raw_usb_iserial.clone();
            }
            if need_model_recovery {
                if let Some(identity) = identity {
                    model_num = identity.model_num.to_string();
                    model_label_override = Some(identity.label.to_string());
                } else if let Some(pid) = recovered.usb_product_id {
                    model_num = format!("xPID_{pid:04X}");
                }
            }
        }
    }

    let serial = serial?;
    if serial.is_empty() {
        return None;
    }
    let model_label = model_label_override.unwrap_or_else(|| describe_model(&model_num));
    // Stash the volume GUID so the watcher can fast-path subsequent
    // polls (one Win32 resolve vs. re-walking every present volume).
    // Best-effort: a None here just degrades the next poll back to a
    // full scan, no correctness impact.
    let volume_guid = volume_guid_for_mount(drive);
    Some(DetectedIpod {
        serial,
        model_label,
        drive: drive.to_string_lossy().into_owned(),
        // Filled in by the daemon (or left as None) — iTunesDB parsing
        // is expensive and not needed for serial/model identification.
        name: None,
        volume_guid,
    })
}

/// Resolve `<mount>` (e.g. `G:\`) to its stable `\\?\Volume{GUID}\`
/// form via `GetVolumeNameForVolumeMountPointW`. `None` on non-Windows
/// or when the OS rejects the query (path doesn't exist, no GUID
/// assigned, permission denied). The returned string includes the
/// trailing `\` per Win32 convention so it can be fed straight back
/// to `GetVolumePathNamesForVolumeNameW`.
pub fn volume_guid_for_mount(mount: &std::path::Path) -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;

        // GetVolumeNameForVolumeMountPointW requires the mount path to
        // end with a backslash, and returns a string of the form
        // "\\?\Volume{GUID}\" (50 chars + null is the documented worst
        // case; 64 wide chars is a generous buffer).
        let mut mount_str = mount.to_string_lossy().into_owned();
        if !mount_str.ends_with('\\') && !mount_str.ends_with('/') {
            mount_str.push('\\');
        }
        let mount_w: Vec<u16> = std::ffi::OsStr::new(&mount_str)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut buf = [0u16; 64];
        let ok = unsafe {
            GetVolumeNameForVolumeMountPointW(mount_w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32)
        };
        if ok == 0 {
            return None;
        }
        let nul = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..nul]))
    }
    #[cfg(not(windows))]
    {
        let _ = mount;
        None
    }
}

/// Reverse of `volume_guid_for_mount`: resolve a `\\?\Volume{GUID}\`
/// string to its current mount path (typically a drive letter like
/// `G:\`, but also folder mounts if present).
///
/// Returns `None` when the volume is no longer mounted (device
/// unplugged, ejected, or moved to a different machine). The daemon
/// uses this as a fast-path disconnect signal — far cheaper than
/// re-walking every present volume and inspecting iPod_Control.
pub fn mount_for_volume_guid(volume_guid: &str) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetVolumePathNamesForVolumeNameW;

        let guid_w: Vec<u16> = std::ffi::OsStr::new(volume_guid)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // GetVolumePathNamesForVolumeNameW returns a double-null-
        // terminated multi-string of mount paths. MAX_PATH (260) is
        // generous for the typical case (one drive letter); we'd only
        // need more for many folder mounts on the same volume, which
        // is exotic.
        let mut buf = vec![0u16; 260];
        let mut returned_len: u32 = 0;
        let ok = unsafe {
            GetVolumePathNamesForVolumeNameW(
                guid_w.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut returned_len,
            )
        };
        if ok == 0 {
            return None;
        }
        // First null-terminated entry. Skip empty (a volume with no
        // mount points returns just `\0\0`).
        let first_nul = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        if first_nul == 0 {
            return None;
        }
        Some(std::path::PathBuf::from(String::from_utf16_lossy(
            &buf[..first_nul],
        )))
    }
    #[cfg(not(windows))]
    {
        let _ = volume_guid;
        None
    }
}

/// Fast-path device check for the daemon's polling watcher: given a
/// known volume GUID from a prior full scan, resolve it to the current
/// mount path and verify the iPod files are still there. Skips the
/// drive-letter enumeration + per-mount file probes + SCSI INQUIRY +
/// USB descriptor lookup — all of which only need to run on the cold
/// path (first observation, or after a fast-path miss).
///
/// Returns `None` when the volume GUID no longer resolves (device gone
/// or moved) or when the resolved mount no longer contains the canonical
/// iPod files. Callers fall back to `scan_for_ipod` in that case.
///
/// On hit, returns a fresh `DetectedIpod` with the current drive path
/// (which may differ from the cached observation if Windows reassigned
/// the letter) and the cached identity carried forward.
pub fn try_resolve_known_volume(volume_guid: &str, prev: &DetectedIpod) -> Option<DetectedIpod> {
    let mount = mount_for_volume_guid(volume_guid)?;
    if !crate::ipod::layout::is_ipod_mount(&mount) {
        return None;
    }
    Some(DetectedIpod {
        serial: prev.serial.clone(),
        model_label: prev.model_label.clone(),
        drive: mount.to_string_lossy().into_owned(),
        name: prev.name.clone(),
        volume_guid: Some(volume_guid.to_string()),
    })
}

/// Extract a leading drive letter from a path like `G:\` → `'G'`.
#[cfg(windows)]
fn drive_letter(drive: &std::path::Path) -> Option<char> {
    let s = drive.to_str()?;
    let first = s.chars().next()?;
    if first.is_ascii_alphabetic() {
        Some(first.to_ascii_uppercase())
    } else {
        None
    }
}

/// `(ModelNumStr, friendly-label)` pair for a detected iPod. The
/// `model_num` MUST be a value libgpod's `ipod_info_table` in
/// `src/itdb_device.c` recognises — otherwise the hash58 codepath
/// silently degrades to `ITDB_CHECKSUM_NONE` and iTunes refuses the
/// resulting iTunesDB. `label` is the user-facing string.
///
/// `#[allow(dead_code)]` because the only consumer (`identify_ipod`,
/// reached from the legacy v2 adapters) may be platform-gated. The
/// struct + its consumer still want to live in platform-neutral test
/// territory — `identify_ipod`'s PID-disambiguation tests cover real
/// product logic that's worth running on Linux/macOS CI too.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IpodIdentity {
    model_num: &'static str,
    label: &'static str,
}

/// Collect the ordinary OS facts associated with one mounted volume.
/// Failure to associate the mount returns `None`; individual facts remain
/// optional so a recognizable filesystem can still be observed safely.
pub(crate) fn ordinary_usb_facts_for_mount(mount: &std::path::Path) -> Option<OrdinaryUsbFacts> {
    #[cfg(windows)]
    {
        let letter = drive_letter(mount)?;
        windows_ordinary_usb_facts(letter)
    }
    #[cfg(target_os = "linux")]
    {
        linux_ordinary_usb_facts(mount)
    }
    #[cfg(target_os = "macos")]
    {
        macos_ordinary_usb_facts(mount)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = mount;
        None
    }
}

/// Windows USB-descriptor recovery via native Win32 calls — no
/// PowerShell shellout.
///
/// What we need (and where it comes from):
///
/// | Field          | Source                                            |
/// |----------------|---------------------------------------------------|
/// | `disk_number`  | `IOCTL_STORAGE_GET_DEVICE_NUMBER` on `\\.\<letter>:` |
/// | `capacity_bytes` | `IOCTL_DISK_GET_LENGTH_INFO` on same handle    |
/// | `raw_usb_iserial` | 16-hex instance segment on the Apple USB parent |
/// | `usb_product_id` | `PID_XXXX` in the parent USB device's instance ID (`CM_Get_Parent` then `CM_Get_Device_IDW`) |
///
/// All handles are opened with zero access so no admin elevation is
/// needed for any of the IOCTLs; SetupAPI enumeration is unprivileged.
/// Total cost on a warm Windows: low single-digit milliseconds. The
/// PowerShell version it replaced cost 300-500ms (process spawn +
/// CLR init + CIM query + parse), once per iPod plug-in.
#[cfg(windows)]
fn windows_ordinary_usb_facts(drive_letter: char) -> Option<OrdinaryUsbFacts> {
    // Phase 1: open the volume, query disk number + capacity. The
    // handle can be dropped immediately after these two IOCTLs — the
    // remaining work (SetupAPI walk) talks to the device tree, not
    // the volume.
    let (disk_number, capacity_bytes) = {
        let handle = open_volume_for_query(drive_letter)?;
        let raw = std::os::windows::io::AsRawHandle::as_raw_handle(&handle)
            as windows_sys::Win32::Foundation::HANDLE;
        let dn = query_storage_device_number(raw)?;
        // Capacity is best-effort; if it fails we still return a
        // useful observation (capacity is only used to disambiguate
        // Classic 1G/2G/3G).
        let cap = query_disk_length(raw);
        (dn, cap)
    };

    // Phase 2: SetupAPI walk to find the disk-class device interface
    // whose underlying physical drive matches `disk_number` and retain
    // its DevInst for the parent-USB lookup that follows.
    let (_, disk_devinst) = find_disk_device_by_number(disk_number)?;

    // Phase 3: Cfgmgr32 walk from the disk device to its USB parent,
    // then parse PID_XXXX from the parent's instance ID. Failure here
    // is non-fatal — without PID we lose libgpod's model lookup but
    // the FirewireGuid alone is enough to identify the device.
    let usb_parent = usb_parent_instance_id(disk_devinst)?;
    ordinary_facts_from_apple_usb_parent(&usb_parent, capacity_bytes)
}

// =========================================================================
// Native Win32 helpers for ordinary USB fact collection.
//
// Replaces a PowerShell shellout (`Get-Volume | Get-Partition | Get-Disk`
// pipeline + `Get-CimInstance Win32_PnPEntity` lookup) with direct IOCTL
// + SetupAPI + Cfgmgr32 calls. Tradeoff: ~150 LOC of FFI here in exchange
// for ~300-500ms saved on every iPod plug-in's cold-path identification
// and no PowerShell dependency on the user's machine.
// =========================================================================

/// Open `\\.\<letter>:` with zero `dwDesiredAccess` so the OS treats
/// it as an IOCTL-only handle (no read or write data path granted).
/// This is the documented non-admin path for device-information queries.
#[cfg(windows)]
fn open_volume_for_query(drive_letter: char) -> Option<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::FromRawHandle;
    use std::ptr;
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let path = format!(r"\\.\{}:", drive_letter.to_ascii_uppercase());
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let h: HANDLE = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: CreateFileW returned a valid handle we own exclusively.
    Some(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(h as *mut _) })
}

/// `IOCTL_STORAGE_GET_DEVICE_NUMBER` → physical drive number for the
/// volume. Stable Win32 ABI; pinned locally rather than chased through
/// windows-sys re-export hierarchy.
#[cfg(windows)]
fn query_storage_device_number(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<u32> {
    use std::ffi::c_void;
    use std::ptr;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x002D_1080;

    #[repr(C)]
    struct StorageDeviceNumber {
        device_type: u32,
        device_number: u32,
        partition_number: u32,
    }

    let mut sdn: StorageDeviceNumber = unsafe { std::mem::zeroed() };
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            ptr::null_mut(),
            0,
            &mut sdn as *mut _ as *mut c_void,
            std::mem::size_of::<StorageDeviceNumber>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    Some(sdn.device_number)
}

/// `IOCTL_DISK_GET_LENGTH_INFO` → volume length in bytes. Works through
/// the volume handle (no need to open the underlying physical drive).
#[cfg(windows)]
fn query_disk_length(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
    use std::ffi::c_void;
    use std::ptr;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;

    #[repr(C)]
    struct GetLengthInformation {
        length: i64,
    }

    let mut info: GetLengthInformation = unsafe { std::mem::zeroed() };
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_DISK_GET_LENGTH_INFO,
            ptr::null_mut(),
            0,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<GetLengthInformation>() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    if info.length < 0 {
        return None;
    }
    Some(info.length as u64)
}

/// GUID_DEVINTERFACE_DISK: device-interface class GUID for disk
/// devices. Pinned locally rather than depending on windows-sys'
/// constants module (the constant moves around between feature sets
/// and minor versions).
#[cfg(windows)]
const GUID_DEVINTERFACE_DISK: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x53F5_6307,
    data2: 0xB6BF,
    data3: 0x11D0,
    data4: [0x94, 0xF2, 0x00, 0xA0, 0xC9, 0x1E, 0xFB, 0x8B],
};

/// Enumerate present disk-class device interfaces, find the one whose
/// underlying physical drive number matches `target_disk_number`, and
/// return `(device_path, DevInst)`. The device path is the USBSTOR
/// interface string (contains the FirewireGuid); the DevInst is the
/// handle Cfgmgr32 uses for parent-lookup.
#[cfg(windows)]
fn find_disk_device_by_number(target_disk_number: u32) -> Option<(String, u32)> {
    use std::ptr;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
        SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
        SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
    };

    // HDEVINFO is an isize handle; -1 is the documented invalid value.
    const INVALID_HDEVINFO: isize = -1;
    let hdev = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_DISK,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if hdev == INVALID_HDEVINFO {
        return None;
    }

    let mut result: Option<(String, u32)> = None;
    let mut index: u32 = 0;
    loop {
        let mut iface: SP_DEVICE_INTERFACE_DATA = unsafe { std::mem::zeroed() };
        iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        let ok = unsafe {
            SetupDiEnumDeviceInterfaces(
                hdev,
                ptr::null_mut(),
                &GUID_DEVINTERFACE_DISK,
                index,
                &mut iface,
            )
        };
        if ok == 0 {
            break;
        }
        index += 1;

        // First call: get required buffer size for the detail data.
        let mut required: u32 = 0;
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &iface,
                ptr::null_mut(),
                0,
                &mut required,
                ptr::null_mut(),
            );
        }
        if required == 0 {
            continue;
        }

        // Second call: real fetch. cbSize at the head of the buffer
        // is the size of the *struct header* (not the buffer); on
        // 64-bit Windows that's 8 (DWORD cbSize + WCHAR DevicePath[1]
        // padded to 8). std::mem::size_of mirrors what the windows-sys
        // binding's C layout dictates, so it stays correct across
        // architectures.
        let mut buf = vec![0u8; required as usize];
        unsafe {
            *(buf.as_mut_ptr() as *mut u32) =
                std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        }
        let mut devinfo: SP_DEVINFO_DATA = unsafe { std::mem::zeroed() };
        devinfo.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        let ok = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &iface,
                buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W,
                required,
                ptr::null_mut(),
                &mut devinfo,
            )
        };
        if ok == 0 {
            continue;
        }

        // The DevicePath field begins immediately after cbSize. Read
        // it as a wide string, find the null terminator.
        let path_offset = std::mem::size_of::<u32>();
        let path_bytes = &buf[path_offset..];
        let path_u16: &[u16] = unsafe {
            std::slice::from_raw_parts(path_bytes.as_ptr() as *const u16, path_bytes.len() / 2)
        };
        let nul = path_u16
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(path_u16.len());
        let device_path = String::from_utf16_lossy(&path_u16[..nul]);

        // Open the device, IOCTL_STORAGE_GET_DEVICE_NUMBER, compare
        // against the target. Zero-access open + IOCTL works here
        // exactly like it does for the volume handle.
        let dev_disk_number = match query_disk_number_for_device_path(&device_path) {
            Some(n) => n,
            None => continue,
        };
        if dev_disk_number == target_disk_number {
            result = Some((device_path, devinfo.DevInst));
            break;
        }
        // Mark `buf` read so the borrow ends before the next iteration.
        let _ = buf;
    }

    unsafe {
        SetupDiDestroyDeviceInfoList(hdev);
    }
    result
}

/// Open an arbitrary device path with zero access (IOCTL-only) and
/// query its STORAGE_DEVICE_NUMBER. Used by the disk-class
/// enumeration to filter for the volume we care about.
#[cfg(windows)]
fn query_disk_number_for_device_path(device_path: &str) -> Option<u32> {
    use std::os::windows::io::FromRawHandle;
    use std::ptr;
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide: Vec<u16> = device_path
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let h: HANDLE = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    let owned = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(h as *mut _) };
    let raw = std::os::windows::io::AsRawHandle::as_raw_handle(&owned) as HANDLE;
    query_storage_device_number(raw)
}

/// Walk from a disk device's DevInst to its USB parent, return the
/// parent's instance ID (e.g. `USB\VID_05AC&PID_1261\000A27002138B0A8`).
/// `None` if the disk has no Cfgmgr32 parent, or the parent's ID
/// query fails.
#[cfg(windows)]
fn usb_parent_instance_id(disk_devinst: u32) -> Option<String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Device_IDW, CM_Get_Device_ID_Size, CM_Get_Parent,
    };

    const CR_SUCCESS: u32 = 0;

    let mut parent: u32 = 0;
    let cr = unsafe { CM_Get_Parent(&mut parent, disk_devinst, 0) };
    if cr != CR_SUCCESS {
        return None;
    }

    let mut id_size: u32 = 0;
    let cr = unsafe { CM_Get_Device_ID_Size(&mut id_size, parent, 0) };
    if cr != CR_SUCCESS || id_size == 0 {
        return None;
    }

    // +1 for the null terminator that CM_Get_Device_IDW writes but
    // CM_Get_Device_ID_Size doesn't count.
    let mut buf = vec![0u16; (id_size + 1) as usize];
    let cr = unsafe { CM_Get_Device_IDW(parent, buf.as_mut_ptr(), buf.len() as u32, 0) };
    if cr != CR_SUCCESS {
        return None;
    }
    let nul = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..nul]))
}

// =========================================================================
// Linux USB-descriptor recovery via /proc/self/mountinfo + sysfs walk.
//
// The Linux kernel exposes USB device metadata under /sys/bus/usb/devices/
// (and symlinked into /sys/block/<dev>/device's ancestor chain). For a
// USB-attached block device we:
//
//   1. Resolve mount path → /dev/<block> via /proc/self/mountinfo.
//   2. Strip partition suffix (sdb1 → sdb; nvme0n1p1 → nvme0n1) to get
//      the disk device.
//   3. Walk /sys/block/<disk>/device's parent chain looking for a
//      directory containing `idVendor` + `idProduct` + `serial` files
//      — that's the USB device node.
//   4. Validate idVendor == 0x05AC (Apple), pull idProduct (the PID we
//      already use for libgpod's model lookup) and serial (the iPod's
//      FirewireGuid, burnt into iSerialNumber).
//   5. Capacity from /sys/block/<disk>/size, multiplied by the 512-byte
//      sector unit the kernel reports in.
//
// =========================================================================

#[cfg(target_os = "linux")]
fn linux_ordinary_usb_facts(mount: &std::path::Path) -> Option<OrdinaryUsbFacts> {
    let block_dev = linux_block_device_for_mount(mount)?;
    let disk_name = linux_strip_partition_suffix(&block_dev)?;
    let usb_dir = linux_find_usb_parent(
        &std::path::PathBuf::from("/sys/block")
            .join(&disk_name)
            .join("device"),
    )?;

    // idVendor / idProduct are 4-digit lowercase hex (no 0x prefix).
    let id_vendor = linux_read_sysfs_hex_u16(&usb_dir.join("idVendor"))?;
    if id_vendor != 0x05AC {
        tracing::debug!(
            "device: USB parent {} has idVendor={:04x} (not Apple); skipping",
            usb_dir.display(),
            id_vendor
        );
        return None;
    }
    let id_product = linux_read_sysfs_hex_u16(&usb_dir.join("idProduct"));
    let raw_usb_iserial = std::fs::read_to_string(usb_dir.join("serial"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| format!("0x{}", value.to_uppercase()));

    // /sys/block/<disk>/size is in 512-byte sectors regardless of the
    // device's logical block size (a kernel-stable contract; see
    // Documentation/block/stat.rst).
    let capacity_bytes = std::fs::read_to_string(format!("/sys/block/{disk_name}/size"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|sectors| sectors * 512);

    Some(OrdinaryUsbFacts {
        raw_usb_iserial,
        usb_product_id: id_product,
        capacity_bytes,
    })
}

/// Find the block device backing `mount` by parsing `/proc/self/mountinfo`.
/// Format per proc(5) (excerpt of fields we care about):
///   `mount-id parent-id major:minor root mount-point mount-opts ... - fs-type source super-opts`
/// We match on the 5th field (mount point) and return the field two slots
/// past the `-` separator (the device source).
#[cfg(target_os = "linux")]
fn linux_block_device_for_mount(mount: &std::path::Path) -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let mount_str = mount.to_string_lossy();
    let mount_norm = mount_str.trim_end_matches('/');
    for line in mounts.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 7 {
            continue;
        }
        let mp = parts[4].trim_end_matches('/');
        if mp != mount_norm {
            continue;
        }
        // Find the "-" separator marking the end of optional fields.
        let dash = parts.iter().position(|&p| p == "-")?;
        // After "-": fs-type, source, super-opts. Source is at dash+2.
        let source = parts.get(dash + 2)?;
        return Some((*source).to_string());
    }
    None
}

/// Strip the partition suffix from a block-device path. Handles both
/// the sd/hd/vd naming (digits trail directly, e.g. sdb1 → sdb) and
/// nvme/mmcblk naming (partition prefixed with `p`, e.g. nvme0n1p1
/// → nvme0n1). Returns just the disk basename (no `/dev/` prefix).
///
/// iPods always present as USB Mass Storage devices, so in practice
/// they come up as `sdX` on Linux — but the NVMe/mmcblk handling is
/// here for robustness (e.g. user's home filesystem is on NVMe and
/// they're poking around with --ipod).
#[cfg(target_os = "linux")]
fn linux_strip_partition_suffix(dev_path: &str) -> Option<String> {
    let basename = std::path::Path::new(dev_path).file_name()?.to_str()?;
    // nvme0n1p1 / mmcblk0p1: partition is `p<digits>` at the tail; the
    // disk name itself ends in digits, so a generic "strip trailing
    // digits" rule would over-strip.
    if basename.starts_with("nvme") || basename.starts_with("mmcblk") {
        if let Some(p_pos) = basename.rfind('p') {
            if basename[p_pos + 1..].chars().all(|c| c.is_ascii_digit()) {
                return Some(basename[..p_pos].to_string());
            }
        }
        return Some(basename.to_string());
    }
    // sdX / hdX / vdX: trailing digits are the partition.
    Some(
        basename
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .to_string(),
    )
}

/// Walk up the symlink chain from `/sys/block/<disk>/device` until we
/// find a directory containing `idVendor` + `idProduct` files — that's
/// the USB device node. Returns `None` if we hit the sysfs root or any
/// step fails to canonicalize (device unplugged mid-walk).
#[cfg(target_os = "linux")]
fn linux_find_usb_parent(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut current = start.canonicalize().ok()?;
    let stop = std::path::Path::new("/sys");
    loop {
        if current.join("idVendor").is_file() && current.join("idProduct").is_file() {
            return Some(current);
        }
        if current == stop || current.as_os_str() == "/" {
            return None;
        }
        current = current.parent()?.to_path_buf();
    }
}

#[cfg(target_os = "linux")]
fn linux_read_sysfs_hex_u16(path: &std::path::Path) -> Option<u16> {
    let s = std::fs::read_to_string(path).ok()?;
    u16::from_str_radix(s.trim(), 16).ok()
}

#[cfg(target_os = "macos")]
fn macos_ordinary_usb_facts(mount: &std::path::Path) -> Option<OrdinaryUsbFacts> {
    let ident = crate::ipod::macos_iokit::identity_for_mount(mount)?;
    Some(OrdinaryUsbFacts {
        raw_usb_iserial: ident.firewire_guid,
        usb_product_id: ident.pid,
        capacity_bytes: ident.capacity_bytes,
    })
}

/// Parse the 4-hex-digit USB Product ID out of an Apple USB device
/// instance path like `USB\VID_05AC&PID_1261\000A27002138B0A8`.
/// Case-insensitive on the `PID_` token because Windows surfaces the
/// instance ID in either case depending on which API the caller used.
#[allow(dead_code)] // Only called from Windows ordinary USB fact collection;
                    // kept platform-neutral so its parser tests still run on Linux/macOS CI.
fn extract_pid_from_apple_usb_path(path: &str) -> Option<u16> {
    let upper = path.to_ascii_uppercase();
    let mut components = upper.split(['\\', '&', '#']);
    if !components.clone().any(|component| component == "VID_05AC") {
        return None;
    }
    let pid = components.find_map(|component| component.strip_prefix("PID_"))?;
    if pid.len() != 4 || !pid.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(pid, 16).ok()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn ordinary_facts_from_apple_usb_parent(
    parent_instance_id: &str,
    capacity_bytes: Option<u64>,
) -> Option<OrdinaryUsbFacts> {
    let upper = parent_instance_id.to_ascii_uppercase();
    if !upper
        .split(['\\', '&', '#'])
        .any(|component| component == "VID_05AC")
    {
        return None;
    }

    Some(OrdinaryUsbFacts {
        raw_usb_iserial: extract_firewire_guid_from_usb_path(parent_instance_id),
        usb_product_id: extract_pid_from_apple_usb_path(parent_instance_id),
        capacity_bytes,
    })
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
#[allow(dead_code)] // See IpodIdentity — only consumed from Windows but the PID
                    // disambiguation logic is platform-neutral and unit-tested.
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
                model_num: "MB029", // CLASSIC_1 80GB silver
                label: "iPod Classic (1st gen, 80GB)",
            },
            Some(g) if g < 140 => IpodIdentity {
                model_num: "MB562", // CLASSIC_2 120GB silver
                label: "iPod Classic (2nd gen, 120GB)",
            },
            Some(_) => IpodIdentity {
                model_num: "MC293", // CLASSIC_3 160GB silver — hash-neutral
                // pick (CLASSIC_1 thick 160GB also valid)
                label: "iPod Classic (160GB)", // no gen claim — ambiguous
            },
            None => return None, // No capacity → can't safely pick a
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
        0x1240 => Some(IpodIdentity {
            model_num: "A350",
            label: "iPod Nano (1st gen)",
        }),
        0x1260 => Some(IpodIdentity {
            model_num: "A477",
            label: "iPod Nano (2nd gen)",
        }),
        0x1262 => Some(IpodIdentity {
            model_num: "A978",
            label: "iPod Nano (3rd gen)",
        }),
        0x1263 => Some(IpodIdentity {
            model_num: "B480",
            label: "iPod Nano (4th gen)",
        }),
        0x1265 => Some(IpodIdentity {
            model_num: "C027",
            label: "iPod Nano (5th gen)",
        }),
        0x1266 => Some(IpodIdentity {
            model_num: "C525",
            label: "iPod Nano (6th gen)",
        }),
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
            label: "iPod Mini", // no gen claim — PID shared 1G/2G
        }),
        0x1209 => Some(IpodIdentity {
            model_num: "A002", // VIDEO_1 30GB white
            label: "iPod Video (5th gen)",
        }),
        0x1206 => Some(IpodIdentity {
            model_num: "A444", // VIDEO_2 30GB white
            label: "iPod Video (5.5 gen)",
        }),
        0x1204 => Some(IpodIdentity {
            model_num: "9829", // PHOTO 30GB
            label: "iPod Photo",
        }),
        // PID 0x1202 covers iPod 1G + 2G (no in-USB distinguisher).
        // PID 0x1201 = iPod 3G. PID 0x1203 = iPod 4G. We pick a 1G
        // ModelNumStr as the catch-all (8513 = 1G 5GB) — none of
        // these generations sign the DB so the choice is cosmetic.
        // Caller's xPID fallback covers the rare case where we want
        // more precision later.
        0x1202 => Some(IpodIdentity {
            model_num: "8513", // FIRST 5GB (PID shared 1G/2G)
            label: "iPod (1st/2nd gen)",
        }),
        0x1201 => Some(IpodIdentity {
            model_num: "8976", // THIRD 10GB
            label: "iPod (3rd gen)",
        }),
        0x1203 => Some(IpodIdentity {
            model_num: "9282", // FOURTH 20GB
            label: "iPod (4th gen)",
        }),
        0x1300 => Some(IpodIdentity {
            model_num: "9724", // SHUFFLE_1 512MB
            label: "iPod Shuffle (1st gen)",
        }),
        0x1301 => Some(IpodIdentity {
            model_num: "A546", // SHUFFLE_2 1GB silver
            label: "iPod Shuffle (2nd gen)",
        }),
        0x1302 => Some(IpodIdentity {
            model_num: "C306", // SHUFFLE_3 2GB silver
            label: "iPod Shuffle (3rd gen)",
        }),
        0x1303 => Some(IpodIdentity {
            model_num: "C584", // SHUFFLE_4 2GB silver
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
#[allow(dead_code)] // Windows-only consumer; kept platform-neutral for its tests.
fn extract_firewire_guid_from_usb_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 16 {
        return None;
    }
    for start in 0..=bytes.len() - 16 {
        let window = &bytes[start..start + 16];
        if !window.iter().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        if start > 0 && bytes[start - 1].is_ascii_hexdigit() {
            continue;
        }
        if start + 16 < bytes.len() && bytes[start + 16].is_ascii_hexdigit() {
            continue;
        }
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
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
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

/// Find every mounted volume that looks like an iPod and return the
/// unique mount path. Errors if zero or more than one match — caller
/// must then ask for `--ipod <drive>` to disambiguate.
pub fn detect_ipod_mount() -> Result<String> {
    let candidates = candidate_mount_points()
        .into_iter()
        .filter(|p| crate::ipod::layout::is_ipod_mount(p))
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    pick_mount(candidates)
}

/// Enumerate mount-point candidates that might host an iPod. The caller
/// applies `is_ipod_mount` (which checks for `iPod_Control/Device/SysInfo`
/// + `iPod_Control/iTunes/iTunesDB`) to reject non-iPod candidates, so
/// false positives here are cheap. Native enumeration per OS:
///
/// **Windows:** `GetLogicalDrives` (one bitmask call returning which
/// drive letters are currently present) + per-present-letter
/// `GetDriveTypeW` lookup, keeping only removable / fixed volumes.
/// Avoids hanging on slow network shares (`DRIVE_REMOTE`) or empty
/// optical drives (`DRIVE_CDROM`).
///
/// **Linux:** Parse `/proc/mounts`, filter out pseudo-FSes (proc, sysfs,
/// cgroup, tmpfs, etc.) that can't host iPod content. Real iPod mounts
/// land here as `vfat` (Classic, Nano, Shuffle) or `hfsplus` (older
/// Mac-formatted iPods on Linux with hfsplus-utils).
///
/// **macOS:** Enumerate `/Volumes/<name>/`. macOS auto-mounts every
/// removable volume there by name; the boot disk is at `/` and
/// intentionally not in `/Volumes`.
///
/// **Other Unix (BSD, etc.):** Empty for now. The TUI still works when
/// the user passes `--ipod <path>` explicitly.
///
/// FUTURE: on Windows, swap for `FindFirstVolumeW` +
/// `GetVolumePathNamesForVolumeNameW` to support folder-mounted iPods
/// (`C:\Mounts\iPod`) and surface the stable `\\?\Volume{GUID}\` path
/// for persisted config keyed on volume identity rather than the
/// shufflable drive letter.
fn candidate_mount_points() -> Vec<std::path::PathBuf> {
    #[cfg(windows)]
    {
        windows_drive_letters_for_mountable_volumes()
            .into_iter()
            .map(|letter| std::path::PathBuf::from(format!("{letter}:\\")))
            .collect()
    }
    #[cfg(target_os = "linux")]
    {
        linux_mount_candidates()
    }
    #[cfg(target_os = "macos")]
    {
        macos_volume_candidates()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Vec::new()
    }
}

/// Parse `/proc/mounts` and return the mount points of every "real"
/// filesystem — skipping the long list of kernel pseudo-FSes that can
/// never host an iPod.
///
/// Per-line layout (procfs(5)): `device mountpoint fstype options dump pass`.
/// Whitespace-separated, but paths containing spaces are escaped as `\040`;
/// since iPod mounts almost never have spaces in their path (and the
/// `is_ipod_mount` probe just fails on the escaped path, no harm done) we
/// don't bother unescaping for this filter pass.
#[cfg(target_os = "linux")]
fn linux_mount_candidates() -> Vec<std::path::PathBuf> {
    let body = match std::fs::read_to_string("/proc/mounts") {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!("device: cannot read /proc/mounts: {e}; auto-detect disabled");
            return Vec::new();
        }
    };
    body.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let _device = parts.next()?;
            let mount = parts.next()?;
            let fstype = parts.next()?;
            // Kernel pseudo-FSes — these can never host iPod content.
            // The set isn't exhaustive but covers what Ubuntu / Fedora /
            // Arch / WSL report by default; an unknown pseudo-FS just
            // takes the slow path through is_ipod_mount, no correctness
            // issue.
            if matches!(
                fstype,
                "proc"
                    | "sysfs"
                    | "cgroup"
                    | "cgroup2"
                    | "tmpfs"
                    | "devpts"
                    | "devtmpfs"
                    | "rpc_pipefs"
                    | "binfmt_misc"
                    | "mqueue"
                    | "hugetlbfs"
                    | "fusectl"
                    | "configfs"
                    | "pstore"
                    | "tracefs"
                    | "securityfs"
                    | "debugfs"
                    | "bpf"
                    | "autofs"
                    | "nsfs"
                    | "selinuxfs"
                    | "ramfs"
                    | "squashfs"
                    | "overlay"
            ) {
                return None;
            }
            // FUSE mounts under fuse.* are usually app-specific (gvfs,
            // portal, snap-fuse, sshfs) and not iPod hosts. Skip the
            // common ones; a real iPod mounted via fuse (rare) would
            // still pass.
            if fstype.starts_with("fuse.") {
                return None;
            }
            Some(std::path::PathBuf::from(mount))
        })
        .collect()
}

/// Enumerate `/Volumes/<name>/` — macOS's standard mount point for
/// removable volumes. The system disk lives at `/` and is intentionally
/// excluded. Failed reads (Volumes missing, permission denied) return
/// empty rather than panicking — auto-detect just degrades to "no iPod
/// found" and the user can pass `--ipod` explicitly.
#[cfg(target_os = "macos")]
fn macos_volume_candidates() -> Vec<std::path::PathBuf> {
    match std::fs::read_dir("/Volumes") {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(e) => {
            tracing::debug!("device: cannot read /Volumes: {e}; auto-detect disabled");
            Vec::new()
        }
    }
}

/// Return drive letters that exist AND are removable or fixed. Skips
/// `DRIVE_REMOTE` (UNC mounts can wedge the polling watcher on probe),
/// `DRIVE_CDROM` (mounted ISOs / USB-CD adapters), and the absence
/// types (`DRIVE_UNKNOWN`, `DRIVE_NO_ROOT_DIR`).
///
/// iPod Classic 7G reports as `DRIVE_FIXED` (USB-attached HDD); Nano /
/// Shuffle / flash-based families report as `DRIVE_REMOVABLE`. Both
/// pass the filter.
#[cfg(windows)]
fn windows_drive_letters_for_mountable_volumes() -> Vec<char> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

    // Stable Win32 ABI values for `GetDriveTypeW`'s return code (see
    // `<fileapi.h>`). windows-sys 0.59 doesn't re-export them, so we
    // pin the values directly rather than chase a feature-flag/version
    // mismatch.
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;

    // GetLogicalDrives returns a bitmask where bit N = drive (b'A' + N).
    // Returns 0 only on outright failure (which on modern Windows is
    // essentially never — the API is non-failing for a normal user).
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = std::ffi::OsStr::new(&root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let dt = unsafe { GetDriveTypeW(wide.as_ptr()) };
        if dt == DRIVE_REMOVABLE || dt == DRIVE_FIXED {
            out.push(letter);
        }
    }
    out
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
        assert!(
            err.to_string().contains("E:") && err.to_string().contains("G:"),
            "error message must enumerate the candidates"
        );
        assert!(
            err.to_string().contains("--ipod"),
            "error must hint at --ipod flag"
        );
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
        assert_eq!(
            guid.len(),
            18,
            "expected 0x + 16 hex chars, got len {}: {guid}",
            guid.len()
        );
        assert!(
            guid[2..].chars().all(|c| c.is_ascii_hexdigit()),
            "expected hex digits, got: {guid}"
        );
    }

    #[test]
    fn errors_on_missing_key() {
        let sysinfo = "ModelNumStr: MB029\nOther: value\n";
        assert!(extract_firewire_guid(sysinfo).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn resolve_identity_reads_on_disk_sysinfo() {
        // Layer 1 (on-disk SysInfo present) returns without touching USB
        // recovery — pure, no hardware.
        let dir = std::env::temp_dir().join(format!("classick-sysinfo-{}", std::process::id()));
        let device_dir = dir.join("iPod_Control").join("Device");
        std::fs::create_dir_all(&device_dir).unwrap();
        std::fs::write(
            device_dir.join("SysInfo"),
            "FirewireGuid: 0x000A27002138B0A8\nModelNumStr: MC293\n",
        )
        .unwrap();
        let id = resolve_libgpod_identity(&dir).unwrap();
        assert_eq!(id.firewire_guid, "0x000A27002138B0A8");
        assert_eq!(id.model_num_str, "MC293");
        std::fs::remove_dir_all(&dir).ok();
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
        )
        .unwrap();
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
        assert!(guid[2..]
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
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

    #[test]
    fn pid_extraction_rejects_non_apple_usb_parent() {
        assert!(
            extract_pid_from_apple_usb_path(r"USB\VID_1234&PID_1261\000A27002138B0A8").is_none()
        );
    }

    #[test]
    fn apple_usb_parent_facts_reject_non_apple_identity_and_retain_partial_facts() {
        assert_eq!(
            ordinary_facts_from_apple_usb_parent(
                r"USB\VID_1234&PID_1261\000A27002138B0A8",
                Some(160_000_000_000),
            ),
            None
        );
        assert_eq!(
            ordinary_facts_from_apple_usb_parent(
                r"USB\VID_05AC&PID_1261\000A27002138B0A8",
                Some(160_000_000_000),
            ),
            Some(OrdinaryUsbFacts {
                raw_usb_iserial: Some("0x000A27002138B0A8".to_owned()),
                usb_product_id: Some(0x1261),
                capacity_bytes: Some(160_000_000_000),
            })
        );
        assert_eq!(
            ordinary_facts_from_apple_usb_parent(
                r"USB\VID_05AC&PID_1261\not-a-guid",
                Some(160_000_000_000),
            ),
            Some(OrdinaryUsbFacts {
                raw_usb_iserial: None,
                usb_product_id: Some(0x1261),
                capacity_bytes: Some(160_000_000_000),
            })
        );
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
        assert_eq!(
            id_160.model_num, "MC293",
            "160GB defaults to 3G ModelNumStr (hash-neutral with 1G thick)"
        );
        assert_eq!(
            id_160.label, "iPod Classic (160GB)",
            "label must NOT claim a specific generation — we genuinely can't tell"
        );
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
        assert_eq!(
            identify_ipod(0x1240, None).unwrap().label,
            "iPod Nano (1st gen)"
        );
        assert_eq!(
            identify_ipod(0x1260, None).unwrap().label,
            "iPod Nano (2nd gen)"
        );
        assert_eq!(
            identify_ipod(0x1262, None).unwrap().label,
            "iPod Nano (3rd gen)"
        );
        assert_eq!(
            identify_ipod(0x1263, None).unwrap().label,
            "iPod Nano (4th gen)"
        );
        assert_eq!(
            identify_ipod(0x1265, None).unwrap().label,
            "iPod Nano (5th gen)"
        );
        assert_eq!(
            identify_ipod(0x1266, None).unwrap().label,
            "iPod Nano (6th gen)"
        );
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
    /// not in the table — it's outside classick's scope per SPEC §7.
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
        // Older versions of classick wrote `xPID_XXXX` into synthetic
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
        let tmp =
            std::env::temp_dir().join(format!("ipod-scan-partial-test-{}", std::process::id()));
        let sysinfo_dir = tmp.join("iPod_Control").join("Device");
        std::fs::create_dir_all(&sysinfo_dir).unwrap();
        std::fs::write(
            sysinfo_dir.join("SysInfo"),
            "FirewireGuid: 0xEXAMPLE1234\nModelNumStr: xMB029\n",
        )
        .unwrap();
        assert!(
            scan_drive_for_ipod(&tmp).is_none(),
            "SysInfo without iTunesDB must not be classified as an iPod"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
