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

// Embedded per-model SysInfoExtended templates (CC0, see data/.../ATTRIBUTION.md).
const CLASSIC_LATE2009: &[u8] = include_bytes!("../../data/sysinfo-extended/classic-late2009.plist");
const CLASSIC_6G: &[u8] = include_bytes!("../../data/sysinfo-extended/classic-6g.plist");
const VIDEO_5G: &[u8] = include_bytes!("../../data/sysinfo-extended/video-5g.plist");
const PHOTO_4G: &[u8] = include_bytes!("../../data/sysinfo-extended/photo-4g.plist");
const NANO_1G: &[u8] = include_bytes!("../../data/sysinfo-extended/nano-1g.plist");
const NANO_2G: &[u8] = include_bytes!("../../data/sysinfo-extended/nano-2g.plist");
const NANO_3G: &[u8] = include_bytes!("../../data/sysinfo-extended/nano-3g.plist");
const NANO_4G: &[u8] = include_bytes!("../../data/sysinfo-extended/nano-4g.plist");

/// `(ModelNumStr, template)` — ModelNumStr values transcribed from libgpod's
/// `ipod_info_table` (src/itdb_device.c). That table stores each model code
/// *without* its leading letter (libgpod's `get_ipod_info_from_model_number`
/// strips one leading alpha byte before the lookup, e.g. table entry `C293`
/// matches on-device `ModelNumStr` `MC293`), so every row here is `"M" +
/// <table code>` to match the real on-device string. MC293 (Classic
/// Late-2009) is the hardware-verified entry; the rest are best-effort
/// transcriptions (see spec §"Model set"). Add rows here to support more
/// models — one line + one vendored plist.
const TABLE: &[(&str, &[u8])] = &[
    // iPod Classic G3 / Late-2009 (VERIFIED on-device).
    ("MC293", CLASSIC_LATE2009), // 160GB silver
    ("MC297", CLASSIC_LATE2009), // 160GB black
    // iPod Classic G1 (80GB/160GB) + G2 (120GB) — dstaley's repo ships one
    // SysInfoExtended for both under the "6th generation" folder.
    ("MB029", CLASSIC_6G), // G1 80GB silver
    ("MB147", CLASSIC_6G), // G1 80GB black
    ("MB145", CLASSIC_6G), // G1 160GB silver
    ("MB150", CLASSIC_6G), // G1 160GB black
    ("MB562", CLASSIC_6G), // G2 120GB silver
    ("MB565", CLASSIC_6G), // G2 120GB black
    // iPod Video 5G (Fifth Generation) + 5.5G (Sixth Generation) — one
    // SysInfoExtended shared under dstaley's "5th generation" folder.
    ("MA002", VIDEO_5G), // 5G 30GB white
    ("MA146", VIDEO_5G), // 5G 30GB black
    ("MA003", VIDEO_5G), // 5G 60GB white
    ("MA147", VIDEO_5G), // 5G 60GB black
    ("MA452", VIDEO_5G), // 5G 30GB U2
    ("MA444", VIDEO_5G), // 5.5G 30GB white
    ("MA446", VIDEO_5G), // 5.5G 30GB black
    ("MA664", VIDEO_5G), // 5.5G 30GB U2
    ("MA448", VIDEO_5G), // 5.5G 80GB white
    ("MA450", VIDEO_5G), // 5.5G 80GB black
    // iPod Photo / Fourth Generation (color screen; the plain 4G click
    // wheel iPod has no color screen and no ImageSpecifications at all).
    ("MA079", PHOTO_4G), // 20GB
    ("MA127", PHOTO_4G), // 20GB U2
    ("M9829", PHOTO_4G), // 30GB
    ("M9585", PHOTO_4G), // 40GB
    ("M9830", PHOTO_4G), // 60GB
    ("M9586", PHOTO_4G), // 60GB
    ("MS492", PHOTO_4G), // HP-branded 30GB
    // iPod nano 1G.
    ("MA350", NANO_1G), // 1GB white
    ("MA352", NANO_1G), // 1GB black
    ("MA004", NANO_1G), // 2GB white
    ("MA099", NANO_1G), // 2GB black
    ("MA005", NANO_1G), // 4GB white
    ("MA107", NANO_1G), // 4GB black
    // iPod nano 2G.
    ("MA477", NANO_2G), // 2GB silver
    ("MA426", NANO_2G), // 4GB silver
    ("MA428", NANO_2G), // 4GB blue
    ("MA487", NANO_2G), // 4GB green
    ("MA489", NANO_2G), // 4GB pink
    ("MA725", NANO_2G), // 4GB red
    ("MA726", NANO_2G), // 8GB red
    ("MA497", NANO_2G), // 8GB black
    // iPod nano 3G.
    ("MA978", NANO_3G), // 4GB silver
    ("MA980", NANO_3G), // 8GB silver
    ("MB261", NANO_3G), // 8GB black
    ("MB249", NANO_3G), // 8GB blue
    ("MB253", NANO_3G), // 8GB green
    ("MB257", NANO_3G), // 8GB red
    // iPod nano 4G.
    ("MB480", NANO_4G), // 4GB silver
    ("MB651", NANO_4G), // 4GB blue
    ("MB654", NANO_4G), // 4GB pink
    ("MB657", NANO_4G), // 4GB purple
    ("MB660", NANO_4G), // 4GB orange
    ("MB663", NANO_4G), // 4GB green
    ("MB666", NANO_4G), // 4GB yellow
    ("MB598", NANO_4G), // 8GB silver
    ("MB732", NANO_4G), // 8GB blue
    ("MB735", NANO_4G), // 8GB pink
    ("MB739", NANO_4G), // 8GB purple
    ("MB742", NANO_4G), // 8GB orange
    ("MB745", NANO_4G), // 8GB green
    ("MB748", NANO_4G), // 8GB yellow
    ("MB751", NANO_4G), // 8GB red
    ("MB754", NANO_4G), // 8GB black
    ("MB903", NANO_4G), // 16GB silver
    ("MB905", NANO_4G), // 16GB blue
    ("MB907", NANO_4G), // 16GB pink
    ("MB909", NANO_4G), // 16GB purple
    ("MB911", NANO_4G), // 16GB orange
    ("MB913", NANO_4G), // 16GB green
    ("MB915", NANO_4G), // 16GB yellow
    ("MB917", NANO_4G), // 16GB red
    ("MB918", NANO_4G), // 16GB black
];

/// All embedded templates, for the validity test.
#[cfg(test)]
pub(super) const ALL_TEMPLATES: &[(&str, &[u8])] = &[
    ("classic-late2009", CLASSIC_LATE2009), ("classic-6g", CLASSIC_6G),
    ("video-5g", VIDEO_5G), ("photo-4g", PHOTO_4G),
    ("nano-1g", NANO_1G), ("nano-2g", NANO_2G), ("nano-3g", NANO_3G), ("nano-4g", NANO_4G),
];

/// Embedded SysInfoExtended template for a resolved `ModelNumStr`, or `None`
/// for a model we don't ship a template for (caller skips + warns; never
/// substitute a near-model — a wrong template is worse than none).
pub fn template_for_model(model_num_str: &str) -> Option<&'static [u8]> {
    TABLE.iter().find(|(m, _)| *m == model_num_str).map(|(_, t)| *t)
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

    #[test]
    fn resolves_mc293_to_classic_late2009_template() {
        let t = template_for_model("MC293").expect("MC293 must resolve");
        let xml = std::str::from_utf8(t).unwrap();
        assert!(xml.contains("<key>ImageSpecifications</key>"));
        // Late-2009 Classic exposes the F1069 cover format that the firmware reads.
        assert!(xml.contains("<integer>1069</integer>"));
    }

    #[test]
    fn unknown_model_resolves_to_none() {
        assert!(template_for_model("XPID_9999").is_none());
        assert!(template_for_model("").is_none());
    }

    #[test]
    fn every_embedded_template_has_image_specifications() {
        for (model, bytes) in super::ALL_TEMPLATES {
            let xml = std::str::from_utf8(bytes)
                .unwrap_or_else(|_| panic!("{model}: template not UTF-8"));
            assert!(
                xml.contains("<key>ImageSpecifications</key>"),
                "{model}: template missing ImageSpecifications"
            );
        }
    }
}
