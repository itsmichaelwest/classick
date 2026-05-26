//! Parser for the iPod's `SysInfoExtended` XML plist.
//!
//! The plist is obtained from the device via SCSI INQUIRY (see
//! `crate::scsi_inquiry::read_sysinfo_extended`). It contains the
//! authoritative model identity the iPod's firmware reports about
//! itself — `ModelNumStr`, `SerialNumber`, `FamilyID`, supported
//! artwork/codec formats, etc. This is what iTunes uses to identify
//! the device, and it's the same data libgpod's hash72/hashAB code
//! paths need.
//!
//! Two surfaces:
//! - [`ParsedSysInfo`]: a strongly-typed Rust view of the fields we
//!   actually consume for daemon identification + UI display. We
//!   don't try to deserialise the entire plist (it has ~40 keys we
//!   don't need).
//! - [`write_to_ipod`]: persist the raw XML to
//!   `iPod_Control/Device/SysInfoExtended` so libgpod picks it up
//!   automatically via `itdb_device_read_sysinfo`. This is also what
//!   pre-2010 iTunes did, so we know iPod firmware tolerates the
//!   file's presence (modern iTunes just reads SCSI on demand and
//!   doesn't bother writing the file).

use anyhow::{anyhow, Context, Result};
use plist::Value;
use std::path::Path;

/// Fields we care about from the SysInfoExtended XML. Everything is
/// optional because libgpod's table fallback handles missing keys,
/// and we want a parse failure on one key to not poison the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedSysInfo {
    /// Apple model number with the `M` prefix stripped — e.g.
    /// `"C293"` for an MC293 (iPod Classic 3G 160GB silver). This is
    /// what `libgpod`'s `ipod_info_table` is keyed on (the lookup
    /// helpfully also strips a leading alpha char, so values written
    /// with or without the prefix both resolve).
    pub model_num_str: Option<String>,
    /// Apple's 11-character serial number printed on the back of the
    /// device — e.g. `"EXAMPLE1234"`. Last 3-4 chars are an Apple
    /// model/config code that libgpod can use as a secondary lookup
    /// (`itdb_ipod_info_from_serial`).
    pub serial_number: Option<String>,
    /// Hex string identifier libgpod reads to compute the hash58 key.
    /// Should match what we read from the USB iSerialNumber
    /// descriptor; mismatch would indicate a bug in one of the paths.
    pub firewire_guid: Option<String>,
    /// Apple's per-model family ID. Used by libgpod's hash72 path for
    /// per-device key derivation.
    pub family_id: Option<i64>,
    /// Firmware build identifier. Useful only for telemetry / debug
    /// logging; libgpod doesn't gate on it.
    pub build_id: Option<String>,
    /// Marketing capacity in GB (raw integer from the plist). May
    /// differ from the visible disk capacity by formatting overhead.
    pub capacity_gb: Option<i64>,
}

impl ParsedSysInfo {
    /// Parse the UTF-8 XML returned by `scsi_inquiry::read_sysinfo_extended`.
    ///
    /// We treat individual missing keys as `None` rather than parse
    /// errors — the plist contains 30+ keys and we only need a handful.
    /// A malformed XML root or non-dict top-level IS an error
    /// (indicates the SCSI transport gave us garbage).
    pub fn from_xml(xml: &str) -> Result<Self> {
        let value: Value = plist::from_bytes(xml.as_bytes())
            .with_context(|| "parsing SysInfoExtended XML plist")?;
        let dict = value
            .as_dictionary()
            .ok_or_else(|| anyhow!("SysInfoExtended root is not a dictionary"))?;

        let model_num_str = dict
            .get("ModelNumStr")
            .and_then(|v| v.as_string())
            .map(str::to_string);
        let serial_number = dict
            .get("SerialNumber")
            .and_then(|v| v.as_string())
            .map(str::to_string);
        let firewire_guid = dict
            .get("FireWireGUID")
            .or_else(|| dict.get("FirewireGuid"))
            .and_then(|v| v.as_string())
            .map(str::to_string);
        let family_id = dict.get("FamilyID").and_then(|v| v.as_signed_integer());
        let build_id = dict
            .get("BuildID")
            .and_then(|v| v.as_string())
            .map(str::to_string);
        // Several different keys observed across firmware revisions:
        // newer firmware writes "VisibleCapacity" in raw bytes; older
        // wrote "Capacity" in GB. Try both.
        let capacity_gb = dict
            .get("Capacity")
            .and_then(|v| v.as_signed_integer())
            .or_else(|| {
                dict.get("VisibleCapacity")
                    .and_then(|v| v.as_signed_integer())
                    .map(|bytes| bytes / 1_000_000_000)
            });

        Ok(Self {
            model_num_str,
            serial_number,
            firewire_guid,
            family_id,
            build_id,
            capacity_gb,
        })
    }
}

/// Persist the raw SysInfoExtended XML to
/// `<mount>/iPod_Control/Device/SysInfoExtended` so `libgpod` picks it
/// up via its standard `itdb_device_read_sysinfo` path. This is the
/// same file pre-2010 iTunes wrote (modern iTunes skips it because it
/// just re-queries SCSI on demand) — the iPod firmware tolerates its
/// presence and libgpod's `device->sysinfo_extended` struct gets
/// populated with the full per-device info needed for the hash72 /
/// hashAB code paths.
///
/// Atomic write via a `.tmp` intermediate + rename so an interrupted
/// write doesn't leave a half-formed file libgpod might try to parse.
pub fn write_to_ipod(ipod_mount: &Path, xml: &str) -> Result<()> {
    let dir = ipod_mount.join("iPod_Control").join("Device");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let dst = dir.join("SysInfoExtended");
    let tmp = dst.with_extension("SysInfoExtended.tmp");
    std::fs::write(&tmp, xml.as_bytes())
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &dst)
        .with_context(|| format!("renaming {} → {}", tmp.display(), dst.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal SysInfoExtended fixture covering the keys we extract,
    /// modelled after a real iPod Classic 3G (160GB silver, MC293)
    /// dump from `github.com/dstaley/ipod-sysinfo`.
    const SAMPLE_CLASSIC_3G_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>ModelNumStr</key>
    <string>C293</string>
    <key>SerialNumber</key>
    <string>EXAMPLE1234</string>
    <key>FireWireGUID</key>
    <string>0x000A27002138B0A8</string>
    <key>FamilyID</key>
    <integer>19</integer>
    <key>BuildID</key>
    <string>0x40000000</string>
    <key>Capacity</key>
    <integer>160</integer>
</dict>
</plist>"#;

    #[test]
    fn parses_all_fields_from_classic_3g_fixture() {
        let parsed = ParsedSysInfo::from_xml(SAMPLE_CLASSIC_3G_XML).expect("parse");
        assert_eq!(parsed.model_num_str.as_deref(), Some("C293"));
        assert_eq!(parsed.serial_number.as_deref(), Some("EXAMPLE1234"));
        assert_eq!(parsed.firewire_guid.as_deref(), Some("0x000A27002138B0A8"));
        assert_eq!(parsed.family_id, Some(19));
        assert_eq!(parsed.build_id.as_deref(), Some("0x40000000"));
        assert_eq!(parsed.capacity_gb, Some(160));
    }

    /// Missing keys must not poison the rest — the plist has ~30
    /// keys we don't extract, and any of them could be absent on
    /// older firmware revisions.
    #[test]
    fn missing_keys_are_none_not_error() {
        let minimal = r#"<?xml version="1.0"?><plist version="1.0"><dict>
            <key>ModelNumStr</key><string>C293</string>
        </dict></plist>"#;
        let parsed = ParsedSysInfo::from_xml(minimal).expect("parse");
        assert_eq!(parsed.model_num_str.as_deref(), Some("C293"));
        assert!(parsed.serial_number.is_none());
        assert!(parsed.family_id.is_none());
    }

    /// Newer firmware uses `VisibleCapacity` in raw bytes — we must
    /// convert to GB so the field stays comparable to the older
    /// `Capacity` key (which is in marketed GB directly).
    #[test]
    fn visible_capacity_in_bytes_converts_to_gb() {
        let newer = r#"<?xml version="1.0"?><plist version="1.0"><dict>
            <key>VisibleCapacity</key><integer>160000000000</integer>
        </dict></plist>"#;
        let parsed = ParsedSysInfo::from_xml(newer).expect("parse");
        assert_eq!(parsed.capacity_gb, Some(160));
    }

    /// Some plist exports use lowercase 'g' — accept both spellings
    /// of the FirewireGuid key so an Apple firmware change between
    /// revisions doesn't silently null the field.
    #[test]
    fn firewire_guid_accepts_camel_and_pascal_case() {
        let pascal = r#"<?xml version="1.0"?><plist version="1.0"><dict>
            <key>FireWireGUID</key><string>0xABC</string>
        </dict></plist>"#;
        let camel = r#"<?xml version="1.0"?><plist version="1.0"><dict>
            <key>FirewireGuid</key><string>0xDEF</string>
        </dict></plist>"#;
        assert_eq!(
            ParsedSysInfo::from_xml(pascal).unwrap().firewire_guid.as_deref(),
            Some("0xABC"),
        );
        assert_eq!(
            ParsedSysInfo::from_xml(camel).unwrap().firewire_guid.as_deref(),
            Some("0xDEF"),
        );
    }

    /// Garbage in = error out (not a None-filled struct). Catches
    /// the case where SCSI transport returned non-XML bytes.
    #[test]
    fn malformed_xml_errors() {
        assert!(ParsedSysInfo::from_xml("definitely not xml").is_err());
    }

    /// A plist whose root is an array (not the expected dict) must
    /// error rather than silently returning an empty ParsedSysInfo.
    #[test]
    fn non_dict_root_errors() {
        let array_root = r#"<?xml version="1.0"?><plist version="1.0">
            <array><string>nope</string></array>
        </plist>"#;
        assert!(ParsedSysInfo::from_xml(array_root).is_err());
    }
}
