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

    let mut serial = parse_sysinfo_field(&text, "FirewireGuid");
    let mut model_num = parse_sysinfo_field(&text, "ModelNumStr").unwrap_or_default();
    // Friendly model label fallback used when describe_model can't
    // produce something better from model_num.
    let mut model_label_override: Option<String> = None;

    let need_serial_recovery = serial.as_deref().map(str::is_empty).unwrap_or(true);
    let need_model_recovery = model_num.is_empty();

    if need_serial_recovery || need_model_recovery {
        // SysInfo is missing FirewireGuid (post-reformat case) and/or
        // missing ModelNumStr (synthetic-SysInfo case from an earlier
        // partial recovery). Either way we shell to Windows once,
        // grab whatever the USB stack exposes, and rewrite SysInfo so
        // subsequent polls find both fields on disk and skip this
        // path entirely.
        #[cfg(windows)]
        if let Some(letter) = drive_letter(drive) {
            if let Some(recovered) = recover_ipod_info_from_usb(letter) {
                tracing::info!(
                    "ipod: USB recovery for {}:\\ → guid={}, pid={:?}, model={:?}",
                    letter, recovered.firewire_guid, recovered.pid, recovered.model_label
                );
                if need_serial_recovery {
                    serial = Some(recovered.firewire_guid.clone());
                }
                if need_model_recovery {
                    // Round-trippable marker: describe_model decodes
                    // the xPID_XXXX form back to the friendly label
                    // so on-disk SysInfo is fully self-sufficient.
                    if let Some(pid) = recovered.pid {
                        model_num = format!("xPID_{:04X}", pid);
                    }
                    if let Some(label) = recovered.model_label {
                        model_label_override = Some(label.to_string());
                    }
                }
                // Persist whatever we have so we don't re-shell. Use
                // the recovered FirewireGuid if SysInfo lacked one;
                // otherwise the value we just parsed out of SysInfo.
                let persist_guid = serial.as_deref().unwrap_or(&recovered.firewire_guid);
                if let Err(e) = write_synthesized_sysinfo(&sysinfo, persist_guid, &model_num) {
                    tracing::warn!("ipod: failed to write synthesized SysInfo at {}: {e}", sysinfo.display());
                }
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
/// `firewire_guid` is the value libgpod needs to sign the iTunesDB.
/// `pid` is the USB Product ID (carried so we can round-trip it
/// through SysInfo as `ModelNumStr: xPID_XXXX` and avoid re-shelling
/// on every poll). `model_label` is the friendly mapping for `pid`.
#[cfg(windows)]
struct UsbIpodInfo {
    firewire_guid: String,
    pid: Option<u16>,
    model_label: Option<&'static str>,
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
    // Single script: get storage-class path, extract the 16-hex
    // FirewireGuid, then look up the matching USB-class PnP entity
    // (Service=AppleIPod) and emit its PNPDeviceID. Output: two
    // pipe-separated paths.
    let script = format!(
        "$disk = Get-Volume -DriveLetter {0} | Get-Partition | Get-Disk; \
         $diskPath = $disk.Path; \
         $usbPath = ''; \
         if ($diskPath -match '[0-9a-fA-F]{{16}}') {{ \
             $guid = $matches[0]; \
             $usbPath = (Get-CimInstance Win32_PnPEntity -Filter \"Service='AppleIPod'\" | \
                         Where-Object {{ $_.PNPDeviceID -like \"*$guid*\" }} | \
                         Select-Object -First 1 -ExpandProperty PNPDeviceID); \
         }} \
         Write-Output \"$diskPath|$usbPath\"",
        drive_letter
    );
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    if !output.status.success() { return None; }
    let combined = String::from_utf8_lossy(&output.stdout);
    let mut parts = combined.split('|');
    let disk_path = parts.next()?;
    let usb_path = parts.next().unwrap_or("");

    let firewire_guid = extract_firewire_guid_from_usb_path(disk_path)?;
    let pid = extract_pid_from_apple_usb_path(usb_path);
    let model_label = pid.and_then(model_label_for_pid);
    Some(UsbIpodInfo { firewire_guid, pid, model_label })
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

/// Map Apple's USB Product IDs (vendor 0x05AC) to friendly model
/// labels. Covers the iPod families ipod-sync targets; unknown PIDs
/// fall back to the generic "iPod (model unknown)" label in
/// `describe_model`.
fn model_label_for_pid(pid: u16) -> Option<&'static str> {
    match pid {
        0x1240 => Some("iPod Nano (1st gen)"),
        0x1242 => Some("iPod Nano (2nd gen)"),
        0x1260 => Some("iPod Nano (3rd gen)"),
        0x1261 => Some("iPod Classic"),
        0x1262 => Some("iPod Nano (4th gen)"),
        0x1263 => Some("iPod Classic 120GB"),
        0x1265 => Some("iPod Classic 160GB"),
        0x1266 => Some("iPod Nano (5th gen)"),
        0x1268 => Some("iPod Nano (6th gen)"),
        0x1269 => Some("iPod Nano (7th gen)"),
        0x129E => Some("iPod Touch (1st gen)"),
        _      => None,
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

/// Write a minimal SysInfo body with the recovered FirewireGuid (and
/// ModelNumStr if we know it). Mirrors the format iTunes writes so
/// libgpod's existing `read_firewire_guid` path picks it up
/// unchanged.
#[cfg(windows)]
fn write_synthesized_sysinfo(
    path: &std::path::Path,
    firewire_guid: &str,
    model_num: &str,
) -> std::io::Result<()> {
    let body = if model_num.is_empty() {
        format!("FirewireGuid: {}\n", firewire_guid)
    } else {
        format!("FirewireGuid: {}\nModelNumStr: {}\n", firewire_guid, model_num)
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, body)
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

/// Best-effort human-friendly label from ModelNumStr. M5 will replace
/// this with libgpod's full model lookup.
///
/// Round-trips the `xPID_XXXX` marker ipod-sync's USB recovery path
/// writes into synthetic SysInfo, so a freshly-formatted iPod whose
/// SysInfo we rebuilt still reads back as e.g. "iPod Classic" on
/// every subsequent poll without re-shelling to PowerShell.
fn describe_model(model_num: &str) -> String {
    let upper = model_num.trim_start_matches('x').to_uppercase();
    if let Some(hex) = upper.strip_prefix("PID_") {
        if let Ok(pid) = u16::from_str_radix(hex, 16) {
            if let Some(label) = model_label_for_pid(pid) {
                return label.to_string();
            }
        }
    }
    match upper.as_str() {
        "MB029" | "MB147" | "MB565" => format!("iPod Classic 7G ({upper})"),
        _ if !upper.is_empty() => format!("iPod ({upper})"),
        _ => "iPod (model unknown)".to_string(),
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

    #[test]
    fn known_pids_map_to_models() {
        assert_eq!(model_label_for_pid(0x1261), Some("iPod Classic"));
        assert_eq!(model_label_for_pid(0x1265), Some("iPod Classic 160GB"));
        assert_eq!(model_label_for_pid(0x1240), Some("iPod Nano (1st gen)"));
        assert_eq!(model_label_for_pid(0xFFFF), None);
    }

    #[test]
    fn describe_model_round_trips_synthetic_pid_marker() {
        // SysInfo we wrote during USB recovery uses xPID_XXXX so
        // describe_model can re-derive the friendly label without
        // shelling out on every poll.
        assert_eq!(describe_model("xPID_1261"), "iPod Classic");
        assert_eq!(describe_model("xPID_1265"), "iPod Classic 160GB");
        // Lowercase variant the parser also accepts.
        assert_eq!(describe_model("xpid_1261"), "iPod Classic");
        // Unknown PID falls through to the generic formatter.
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
