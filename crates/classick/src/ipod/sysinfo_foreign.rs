//! Read-only inspection of a foreign `SysInfoExtended` file.

use crate::device::DeviceId;
use plist::{Dictionary, Value};
use std::collections::HashSet;

#[derive(Debug)]
pub enum ForeignSysInfoInspection {
    Malformed {
        error: plist::Error,
    },
    Parsed {
        stable_facts: ForeignSysInfoStableFacts,
        capability: ForeignSysInfoCapability,
        issues: Vec<ForeignSysInfoIssue>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForeignSysInfoStableFacts {
    pub family_id: Option<u32>,
    pub db_version: Option<u32>,
    pub supports_sparse_artwork: Option<bool>,
    pub sqlite_db: Option<bool>,
}

/// Parsed capability data from a file that remains foreign and read-only.
///
/// Each collection is independently `Some` only when every dictionary in that
/// collection is safe for the pinned libgpod consumer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ForeignSysInfoCapability {
    pub album_art: Option<Vec<ForeignImageFormat>>,
    pub image_specifications: Option<Vec<ForeignImageFormat>>,
    pub chapter_image_specs: Option<Vec<ForeignImageFormat>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForeignImageFormat {
    pub format_id: u32,
    pub render_width: u32,
    pub render_height: u32,
    pub display_width: u32,
    pub pixel_format: ForeignPixelFormat,
    pub interlaced: bool,
    pub crop: bool,
    pub row_bytes_alignment: u32,
    pub rotation: i32,
    pub back_color: [u8; 4],
    pub color_adjustment: i32,
    pub gamma_adjustment: f64,
    pub associated_format: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignPixelFormat {
    UyvyBigEndian,
    Rgb565BigEndian,
    Rgb565LittleEndian,
    I420LittleEndian,
    Rgb555LittleEndian,
    RecombinedRgb555LittleEndian,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignSysInfoStableField {
    FireWireGuid,
    FamilyId,
    DbVersion,
    SupportsSparseArtwork,
    SqliteDb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignSysInfoCollection {
    AlbumArt,
    ImageSpecifications,
    ChapterImageSpecs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignSysInfoFormatField {
    FormatId,
    RenderWidth,
    RenderHeight,
    DisplayWidth,
    PixelFormat,
    PixelOrder,
    Interlaced,
    Crop,
    AlignRowBytes,
    RowBytesAlignment,
    Rotation,
    BackColor,
    ColorAdjustment,
    GammaAdjustment,
    AssociatedFormat,
    ExcludedFormats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForeignSysInfoIssue {
    RootNotDictionary,
    MissingStableField(ForeignSysInfoStableField),
    InvalidStableField(ForeignSysInfoStableField),
    IdentityMismatch {
        actual: DeviceId,
    },
    MissingCollection(ForeignSysInfoCollection),
    InvalidCollection(ForeignSysInfoCollection),
    EmptyCollection(ForeignSysInfoCollection),
    InvalidFormat {
        collection: ForeignSysInfoCollection,
        index: usize,
        field: Option<ForeignSysInfoFormatField>,
    },
    DuplicateFormatId {
        collection: ForeignSysInfoCollection,
        format_id: u32,
    },
}

pub fn inspect_foreign_sysinfo_extended(
    bytes: &[u8],
    expected_device_id: &DeviceId,
) -> ForeignSysInfoInspection {
    let root = match plist::Value::from_reader(std::io::Cursor::new(bytes)) {
        Ok(root) => root,
        Err(error) => return ForeignSysInfoInspection::Malformed { error },
    };

    let Some(root) = root.as_dictionary() else {
        return ForeignSysInfoInspection::Parsed {
            stable_facts: ForeignSysInfoStableFacts::default(),
            capability: ForeignSysInfoCapability::default(),
            issues: vec![ForeignSysInfoIssue::RootNotDictionary],
        };
    };

    let mut issues = Vec::new();
    inspect_identity(root, expected_device_id, &mut issues);
    let stable_facts = ForeignSysInfoStableFacts {
        family_id: read_u32(
            root,
            "FamilyID",
            ForeignSysInfoStableField::FamilyId,
            &mut issues,
        ),
        db_version: read_u32(
            root,
            "DBVersion",
            ForeignSysInfoStableField::DbVersion,
            &mut issues,
        ),
        supports_sparse_artwork: read_bool(
            root,
            "SupportsSparseArtwork",
            ForeignSysInfoStableField::SupportsSparseArtwork,
            &mut issues,
        ),
        sqlite_db: read_bool(
            root,
            "SQLiteDB",
            ForeignSysInfoStableField::SqliteDb,
            &mut issues,
        ),
    };

    let album_art = read_formats(
        root,
        "AlbumArt",
        ForeignSysInfoCollection::AlbumArt,
        &mut issues,
    );
    let image_specifications = read_formats(
        root,
        "ImageSpecifications",
        ForeignSysInfoCollection::ImageSpecifications,
        &mut issues,
    );
    let chapter_image_specs = read_formats(
        root,
        "ChapterImageSpecs",
        ForeignSysInfoCollection::ChapterImageSpecs,
        &mut issues,
    );

    let capability = ForeignSysInfoCapability {
        album_art,
        image_specifications,
        chapter_image_specs,
    };

    ForeignSysInfoInspection::Parsed {
        stable_facts,
        capability,
        issues,
    }
}

fn inspect_identity(root: &Dictionary, expected: &DeviceId, issues: &mut Vec<ForeignSysInfoIssue>) {
    let Some(value) = root.get("FireWireGUID") else {
        issues.push(ForeignSysInfoIssue::MissingStableField(
            ForeignSysInfoStableField::FireWireGuid,
        ));
        return;
    };
    let Some(value) = value.as_string() else {
        issues.push(ForeignSysInfoIssue::InvalidStableField(
            ForeignSysInfoStableField::FireWireGuid,
        ));
        return;
    };
    let Ok(actual) = DeviceId::parse(value) else {
        issues.push(ForeignSysInfoIssue::InvalidStableField(
            ForeignSysInfoStableField::FireWireGuid,
        ));
        return;
    };
    if &actual != expected {
        issues.push(ForeignSysInfoIssue::IdentityMismatch { actual });
    }
}

fn read_u32(
    root: &Dictionary,
    key: &str,
    field: ForeignSysInfoStableField,
    issues: &mut Vec<ForeignSysInfoIssue>,
) -> Option<u32> {
    let Some(value) = root.get(key) else {
        issues.push(ForeignSysInfoIssue::MissingStableField(field));
        return None;
    };
    let value = value
        .as_unsigned_integer()
        .and_then(|value| u32::try_from(value).ok());
    if value.is_none() {
        issues.push(ForeignSysInfoIssue::InvalidStableField(field));
    }
    value
}

fn read_bool(
    root: &Dictionary,
    key: &str,
    field: ForeignSysInfoStableField,
    issues: &mut Vec<ForeignSysInfoIssue>,
) -> Option<bool> {
    let Some(value) = root.get(key) else {
        return Some(false);
    };
    let value = value.as_boolean();
    if value.is_none() {
        issues.push(ForeignSysInfoIssue::InvalidStableField(field));
    }
    value
}

fn read_formats(
    root: &Dictionary,
    key: &str,
    collection: ForeignSysInfoCollection,
    issues: &mut Vec<ForeignSysInfoIssue>,
) -> Option<Vec<ForeignImageFormat>> {
    let Some(value) = root.get(key) else {
        issues.push(ForeignSysInfoIssue::MissingCollection(collection));
        return None;
    };
    let Some(values) = value.as_array() else {
        issues.push(ForeignSysInfoIssue::InvalidCollection(collection));
        return None;
    };
    if values.is_empty() {
        issues.push(ForeignSysInfoIssue::EmptyCollection(collection));
        return None;
    }

    let mut formats = Vec::with_capacity(values.len());
    let mut format_ids = HashSet::with_capacity(values.len());
    let mut complete = true;
    for (index, value) in values.iter().enumerate() {
        let Some(dictionary) = value.as_dictionary() else {
            // The pinned parser deliberately ignores keyed-array labels and
            // other non-dictionary members used by Apple captures.
            continue;
        };
        let Some(format) = parse_format(dictionary, collection, index, issues) else {
            complete = false;
            continue;
        };
        if !format_ids.insert(format.format_id) {
            issues.push(ForeignSysInfoIssue::DuplicateFormatId {
                collection,
                format_id: format.format_id,
            });
            complete = false;
        } else {
            formats.push(format);
        }
    }
    if formats.is_empty() {
        issues.push(ForeignSysInfoIssue::EmptyCollection(collection));
        complete = false;
    }
    complete.then_some(formats)
}

fn parse_format(
    value: &Dictionary,
    collection: ForeignSysInfoCollection,
    index: usize,
    issues: &mut Vec<ForeignSysInfoIssue>,
) -> Option<ForeignImageFormat> {
    let format_id = required_u32(
        value,
        "FormatId",
        ForeignSysInfoFormatField::FormatId,
        collection,
        index,
        issues,
    )?;
    let render_width = required_u32(
        value,
        "RenderWidth",
        ForeignSysInfoFormatField::RenderWidth,
        collection,
        index,
        issues,
    )?;
    let render_height = required_u32(
        value,
        "RenderHeight",
        ForeignSysInfoFormatField::RenderHeight,
        collection,
        index,
        issues,
    )?;
    if format_id == 0 || render_width == 0 || render_height == 0 {
        issues.push(ForeignSysInfoIssue::InvalidFormat {
            collection,
            index,
            field: None,
        });
        return None;
    }

    let pixel_format = value.get("PixelFormat").and_then(Value::as_string);
    let Some(pixel_format) = pixel_format.and_then(|pixel_format| {
        parse_pixel_format(pixel_format, value.contains_key("PixelOrder"))
    }) else {
        invalid_format(
            issues,
            collection,
            index,
            ForeignSysInfoFormatField::PixelFormat,
        );
        return None;
    };

    let explicit_alignment = optional_u32(value, "RowBytesAlignment").unwrap_or(0);
    let row_bytes_alignment = if explicit_alignment == 0
        && value
            .get("AlignRowBytes")
            .and_then(Value::as_boolean)
            .unwrap_or(false)
    {
        4
    } else {
        explicit_alignment
    };

    Some(ForeignImageFormat {
        format_id,
        render_width,
        render_height,
        display_width: optional_u32(value, "DisplayWidth").unwrap_or(0),
        pixel_format,
        interlaced: optional_bool(value, "Interlaced"),
        crop: optional_bool(value, "Crop"),
        row_bytes_alignment,
        rotation: optional_i32(value, "Rotation").unwrap_or(0),
        back_color: parse_back_color(value.get("BackColor").and_then(Value::as_string)),
        color_adjustment: optional_i32(value, "ColorAdjustment").unwrap_or(0),
        gamma_adjustment: value
            .get("GammaAdjustment")
            .and_then(Value::as_real)
            .unwrap_or(0.0),
        associated_format: optional_u32(value, "AssociatedFormat").unwrap_or(0),
    })
}

fn required_u32(
    value: &Dictionary,
    key: &str,
    field: ForeignSysInfoFormatField,
    collection: ForeignSysInfoCollection,
    index: usize,
    issues: &mut Vec<ForeignSysInfoIssue>,
) -> Option<u32> {
    let result = optional_u32(value, key);
    if result.is_none() {
        invalid_format(issues, collection, index, field);
    }
    result
}

fn optional_u32(value: &Dictionary, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_unsigned_integer)
        .and_then(|value| u32::try_from(value).ok())
}

fn optional_i32(value: &Dictionary, key: &str) -> Option<i32> {
    value
        .get(key)
        .and_then(Value::as_signed_integer)
        .and_then(|value| i32::try_from(value).ok())
}

fn optional_bool(value: &Dictionary, key: &str) -> bool {
    value.get(key).and_then(Value::as_boolean).unwrap_or(false)
}

fn parse_pixel_format(value: &str, has_pixel_order: bool) -> Option<ForeignPixelFormat> {
    match value {
        "32767579" => Some(ForeignPixelFormat::UyvyBigEndian),
        "42353635" => Some(ForeignPixelFormat::Rgb565BigEndian),
        "4C353635" => Some(ForeignPixelFormat::Rgb565LittleEndian),
        "79343230" => Some(ForeignPixelFormat::I420LittleEndian),
        "4C353535" if has_pixel_order => Some(ForeignPixelFormat::RecombinedRgb555LittleEndian),
        "4C353535" => Some(ForeignPixelFormat::Rgb555LittleEndian),
        _ => None,
    }
}

fn parse_back_color(value: Option<&str>) -> [u8; 4] {
    let value = value
        .and_then(|value| u32::from_str_radix(value, 16).ok())
        .unwrap_or(0);
    value.to_be_bytes()
}

fn invalid_format(
    issues: &mut Vec<ForeignSysInfoIssue>,
    collection: ForeignSysInfoCollection,
    index: usize,
    field: ForeignSysInfoFormatField,
) {
    issues.push(ForeignSysInfoIssue::InvalidFormat {
        collection,
        index,
        field: Some(field),
    });
}
