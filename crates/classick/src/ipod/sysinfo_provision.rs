//! Provision a per-model `SysInfoExtended` onto the iPod so libgpod emits the
//! artwork ithmb format set the firmware reads. See
//! docs/superpowers/specs/2026-07-13-sysinfoextended-provisioning-design.md.

use anyhow::{anyhow, Result};
use std::path::Path;

/// Substitute `firewire_guid` into `template`'s `<key>FireWireGUID</key>` value.
/// The GUID is normalized to the plist form: a leading `0x`/`0X` is stripped and
/// the hex is uppercased (e.g. `0x000a2700…` -> `000A2700…`). The rest of the
/// template — crucially the `ImageSpecifications` — is untouched.
pub fn inject_guid(template: &[u8], firewire_guid: &str) -> Result<Vec<u8>> {
    let xml = std::str::from_utf8(template)
        .map_err(|e| anyhow!("SysInfoExtended template is not UTF-8: {e}"))?;
    let normalized = firewire_guid
        .strip_prefix("0x")
        .or_else(|| firewire_guid.strip_prefix("0X"))
        .unwrap_or(firewire_guid)
        .to_ascii_uppercase();

    let key = "<key>FireWireGUID</key>";
    let key_at = xml
        .find(key)
        .ok_or_else(|| anyhow!("template missing <key>FireWireGUID</key>"))?;
    let open = xml[key_at..]
        .find("<string>")
        .map(|i| key_at + i + "<string>".len())
        .ok_or_else(|| anyhow!("no <string> after FireWireGUID key"))?;
    let close = xml[open..]
        .find("</string>")
        .map(|i| open + i)
        .ok_or_else(|| anyhow!("unterminated <string> for FireWireGUID"))?;

    let mut out = String::with_capacity(xml.len());
    out.push_str(&xml[..open]);
    out.push_str(&normalized);
    out.push_str(&xml[close..]);
    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<plist><dict>\
<key>FireWireGUID</key><string>000A27002150925D</string>\
<key>ImageSpecifications</key><array><key>1069</key><dict><key>FormatId</key><integer>1069</integer></dict></array>\
</dict></plist>";

    #[test]
    fn replaces_guid_and_keeps_image_specs() {
        let out = inject_guid(SAMPLE.as_bytes(), "0x000A27002138B0A8").unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("<key>FireWireGUID</key><string>000A27002138B0A8</string>"));
        assert!(!s.contains("000A27002150925D"));
        // ImageSpecifications (incl. the F1069 format) must be intact.
        assert!(s.contains("<key>ImageSpecifications</key>"));
        assert!(s.contains("<integer>1069</integer>"));
    }

    #[test]
    fn normalizes_lowercase_and_bare_guid() {
        let lower = inject_guid(SAMPLE.as_bytes(), "0x000a27002138b0a8").unwrap();
        assert!(String::from_utf8(lower).unwrap().contains("<string>000A27002138B0A8</string>"));
        let bare = inject_guid(SAMPLE.as_bytes(), "000A27002138B0A8").unwrap();
        assert!(String::from_utf8(bare).unwrap().contains("<string>000A27002138B0A8</string>"));
    }

    #[test]
    fn errors_when_no_guid_key() {
        assert!(inject_guid(b"<plist><dict></dict></plist>", "000A").is_err());
    }
}
