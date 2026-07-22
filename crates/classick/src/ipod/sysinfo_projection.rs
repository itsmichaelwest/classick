//! Pure `SysInfoExtended` projection and existing-file classification.
//!
//! This module does not read or write a mounted device. Publication belongs to
//! the coordinated device transaction.

use super::{CapabilityProfileId, ImageFormat, ValidatedCapabilityProfile};
use crate::device::DeviceId;
use crate::portable::profile::ContentHash;

const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n";

/// Deterministic plist bytes and their lowercase BLAKE3 hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysInfoExtendedProjection {
    device_id: DeviceId,
    capability_profile_id: CapabilityProfileId,
    bytes: Vec<u8>,
    content_hash: ContentHash,
}

impl SysInfoExtendedProjection {
    pub fn device_id(&self) -> &DeviceId {
        &self.device_id
    }

    pub fn capability_profile_id(&self) -> &CapabilityProfileId {
        &self.capability_profile_id
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }
}

/// A read-only decision about an existing mounted-device file.
///
/// Every non-owned outcome retains the exact input bytes for the transaction
/// layer to preserve. This type never grants ownership by parsing content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysInfoExtendedDecision<'a> {
    EligibleToGenerate,
    PreserveForeign { existing_bytes: &'a [u8] },
    ExistingOwnedValid,
    OwnedConflict { existing_bytes: &'a [u8] },
    OwnershipMismatch { existing_bytes: &'a [u8] },
}

/// Project a validated capability profile into the stable libgpod plist shape.
pub fn project_sysinfo_extended(
    device_id: &DeviceId,
    validated_profile: &ValidatedCapabilityProfile,
) -> Result<SysInfoExtendedProjection, super::capability::CapabilityProfileError> {
    let profile = validated_profile.profile();
    profile.validate()?;

    let mut xml = String::from(XML_HEADER);
    // The pinned libgpod parser reads this as an opaque string. Emitting the
    // canonical DeviceId spelling avoids persisting a second `0x` spelling.
    push_string(&mut xml, 1, "FireWireGUID", device_id.as_str());
    push_integer(&mut xml, 1, "FamilyID", i64::from(profile.family_id));
    push_integer(&mut xml, 1, "DBVersion", i64::from(profile.db_version));
    push_formats(&mut xml, "AlbumArt", &profile.album_art);
    push_formats(
        &mut xml,
        "ImageSpecifications",
        &profile.image_specifications,
    );
    push_formats(&mut xml, "ChapterImageSpecs", &profile.chapter_image_specs);
    push_boolean(
        &mut xml,
        1,
        "SupportsSparseArtwork",
        profile.supports_sparse_artwork,
    );
    push_boolean(&mut xml, 1, "SQLiteDB", profile.sqlite_db);
    xml.push_str("</dict>\n</plist>\n");

    let bytes = xml.into_bytes();
    let hash = blake3::hash(&bytes).to_hex();
    let content_hash = ContentHash::parse(hash.as_str())
        .expect("BLAKE3 always returns a 64-character lowercase hexadecimal hash");
    Ok(SysInfoExtendedProjection {
        device_id: device_id.clone(),
        capability_profile_id: profile.profile_id.clone(),
        bytes,
        content_hash,
    })
}

/// Classify existing bytes against an owned hash from a validated portable
/// profile.
///
/// The caller remains responsible for publishing any eligible bytes within the
/// coordinated device transaction.
pub fn decide_sysinfo_extended<'a>(
    existing: Option<&'a [u8]>,
    expected: &SysInfoExtendedProjection,
    owned_hash: Option<&ContentHash>,
) -> SysInfoExtendedDecision<'a> {
    let Some(existing_bytes) = existing else {
        return SysInfoExtendedDecision::EligibleToGenerate;
    };
    let Some(owned_hash) = owned_hash else {
        return SysInfoExtendedDecision::PreserveForeign { existing_bytes };
    };

    let actual_hash = blake3::hash(existing_bytes).to_hex();
    if actual_hash.as_str() != owned_hash.as_str() {
        return SysInfoExtendedDecision::OwnershipMismatch { existing_bytes };
    }
    if existing_bytes == expected.bytes() {
        SysInfoExtendedDecision::ExistingOwnedValid
    } else {
        SysInfoExtendedDecision::OwnedConflict { existing_bytes }
    }
}

fn push_formats(xml: &mut String, key: &str, formats: &[ImageFormat]) {
    push_key(xml, 1, key);
    xml.push_str("  <array>\n");
    for format in formats {
        push_format(xml, format);
    }
    xml.push_str("  </array>\n");
}

fn push_format(xml: &mut String, format: &ImageFormat) {
    xml.push_str("    <dict>\n");
    push_integer(xml, 3, "FormatId", i64::from(format.format_id));
    push_integer(xml, 3, "RenderWidth", i64::from(format.render_width));
    push_integer(xml, 3, "RenderHeight", i64::from(format.render_height));
    if let Some(display_width) = format.display_width {
        push_integer(xml, 3, "DisplayWidth", i64::from(display_width));
    }
    push_string(xml, 3, "PixelFormat", &format.pixel_format);
    push_boolean(xml, 3, "Interlaced", format.interlaced);
    push_boolean(xml, 3, "Crop", format.crop);
    push_boolean(xml, 3, "AlignRowBytes", format.align_row_bytes);
    if let Some(rotation) = format.rotation {
        push_integer(xml, 3, "Rotation", i64::from(rotation));
    }
    if let Some(back_color) = &format.back_color {
        push_string(xml, 3, "BackColor", back_color);
    }
    push_integer(
        xml,
        3,
        "ColorAdjustment",
        i64::from(format.color_adjustment),
    );
    push_real(xml, 3, "GammaAdjustment", format.gamma_adjustment);
    push_integer(
        xml,
        3,
        "AssociatedFormat",
        i64::from(format.associated_format),
    );
    if let Some(excluded_formats) = format.excluded_formats {
        push_integer(xml, 3, "ExcludedFormats", excluded_formats);
    }
    xml.push_str("    </dict>\n");
}

fn push_key(xml: &mut String, indent: usize, key: &str) {
    push_indent(xml, indent);
    xml.push_str("<key>");
    xml.push_str(key);
    xml.push_str("</key>\n");
}

fn push_string(xml: &mut String, indent: usize, key: &str, value: &str) {
    push_key(xml, indent, key);
    push_indent(xml, indent);
    xml.push_str("<string>");
    xml.push_str(value);
    xml.push_str("</string>\n");
}

fn push_integer(xml: &mut String, indent: usize, key: &str, value: i64) {
    push_key(xml, indent, key);
    push_indent(xml, indent);
    xml.push_str("<integer>");
    xml.push_str(&value.to_string());
    xml.push_str("</integer>\n");
}

fn push_real(xml: &mut String, indent: usize, key: &str, value: f64) {
    push_key(xml, indent, key);
    push_indent(xml, indent);
    xml.push_str("<real>");
    xml.push_str(&format!("{value:?}"));
    xml.push_str("</real>\n");
}

fn push_boolean(xml: &mut String, indent: usize, key: &str, value: bool) {
    push_key(xml, indent, key);
    push_indent(xml, indent);
    xml.push_str(if value { "<true/>\n" } else { "<false/>\n" });
}

fn push_indent(xml: &mut String, indent: usize) {
    for _ in 0..indent {
        xml.push_str("  ");
    }
}
