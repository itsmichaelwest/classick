//! Read FirewireGUID from the iPod's SysInfo and push it into libgpod's
//! device struct so itdb_write computes a valid signed iTunesDB.

use anyhow::{anyhow, Result};
use std::ffi::CString;
use std::path::Path;

use crate::ffi;

/// Extract the value of the `FirewireGuid:` line from a SysInfo body.
/// Returns just the hex value (typically `0x...`).
///
/// SysInfo is line-oriented `Key: value`. We match the exact key `FirewireGuid`
/// (case-sensitive — matches how iTunes writes it). Lines where the key is a
/// mere prefix of `FirewireGuid` (e.g. `FirewireGuidSomething`) are skipped.
pub fn extract_firewire_guid(sysinfo: &str) -> Result<String> {
    for line in sysinfo.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != "FirewireGuid" {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            return Err(anyhow!("FirewireGuid line has no value: {line:?}"));
        }
        return Ok(value.to_string());
    }
    Err(anyhow!("FirewireGuid key not found in SysInfo"))
}

/// Resolve `<mount>\iPod_Control\Device\SysInfo`, read it, extract FirewireGuid.
pub fn read_firewire_guid(ipod_mount: &Path) -> Result<String> {
    let path = ipod_mount
        .join("iPod_Control")
        .join("Device")
        .join("SysInfo");
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
    let key = CString::new("FirewireGuid").unwrap();
    let value = CString::new(guid)
        .map_err(|_| anyhow!("FirewireGuid contains interior NUL byte"))?;
    ffi::itdb_device_set_sysinfo(device, key.as_ptr(), value.as_ptr());
    Ok(())
}

/// Detected iPod identity returned by drive-scan helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedIpod {
    pub serial: String,
    pub model_label: String,
    pub drive: String,
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
pub fn scan_drive_for_ipod(drive: &std::path::Path) -> Option<DetectedIpod> {
    let sysinfo = drive.join("iPod_Control").join("Device").join("SysInfo");
    if !sysinfo.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&sysinfo).ok()?;
    let serial = parse_sysinfo_field(&text, "FirewireGuid")?;
    let model_num = parse_sysinfo_field(&text, "ModelNumStr").unwrap_or_default();
    let model_label = describe_model(&model_num);
    Some(DetectedIpod {
        serial,
        model_label,
        drive: drive.to_string_lossy().into_owned(),
    })
}

fn parse_sysinfo_field(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace());
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Best-effort human-friendly label from ModelNumStr. M5 will replace
/// this with libgpod's full model lookup.
fn describe_model(model_num: &str) -> String {
    let upper = model_num.trim_start_matches('x').to_uppercase();
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

/// True if `drive` looks like a mounted iPod (has iTunesDB).
fn looks_like_ipod(drive: &String) -> bool {
    std::path::Path::new(drive)
        .join("iPod_Control")
        .join("iTunes")
        .join("iTunesDB")
        .exists()
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
    fn scan_drive_for_ipod_detects_serial_when_sysinfo_present() {
        let tmp = std::env::temp_dir().join(format!("ipod-scan-found-test-{}", std::process::id()));
        let sysinfo_dir = tmp.join("iPod_Control").join("Device");
        std::fs::create_dir_all(&sysinfo_dir).unwrap();
        std::fs::write(
            sysinfo_dir.join("SysInfo"),
            "FirewireGuid: 0xEXAMPLE1234\nModelNumStr: xMB029\n",
        ).unwrap();
        let detected = scan_drive_for_ipod(&tmp).expect("should detect");
        assert_eq!(detected.serial, "0xEXAMPLE1234");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
