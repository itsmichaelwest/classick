use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

pub const CAPABILITY_PROFILE_SCHEMA_VERSION: u32 = 1;

const ID_REQUIREMENT: &str =
    "capability profile ID must be lowercase ASCII letters or digits separated by single hyphens";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityProfileId(String);

impl CapabilityProfileId {
    pub fn parse(value: &str) -> Result<Self, CapabilityProfileIdError> {
        let valid_character = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
        let bytes = value.as_bytes();
        if bytes.first().is_none_or(|byte| !valid_character(byte))
            || bytes.last().is_none_or(|byte| !valid_character(byte))
            || bytes
                .iter()
                .any(|byte| !valid_character(byte) && *byte != b'-')
            || bytes.windows(2).any(|pair| pair == b"--")
        {
            return Err(CapabilityProfileIdError);
        }

        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityProfileIdError;

impl fmt::Display for CapabilityProfileIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(ID_REQUIREMENT)
    }
}

impl std::error::Error for CapabilityProfileIdError {}

impl FromStr for CapabilityProfileId {
    type Err = CapabilityProfileIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for CapabilityProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for CapabilityProfileId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CapabilityProfileId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityProfile {
    pub schema_version: u32,
    pub profile_id: CapabilityProfileId,
    pub family_id: u32,
    pub db_version: u32,
    pub supports_sparse_artwork: bool,
    pub sqlite_db: bool,
    pub album_art: Vec<ImageFormat>,
    pub image_specifications: Vec<ImageFormat>,
    pub chapter_image_specs: Vec<ImageFormat>,
}

impl CapabilityProfile {
    pub fn from_json(json: &str) -> Result<Self, CapabilityProfileError> {
        let profile: Self = serde_json::from_str(json)?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self).map(|mut json| {
            json.push('\n');
            json
        })
    }

    pub(super) fn validate(&self) -> Result<(), CapabilityProfileError> {
        if self.schema_version != CAPABILITY_PROFILE_SCHEMA_VERSION {
            return Err(CapabilityProfileError::Invalid(format!(
                "unsupported capability profile schema {}",
                self.schema_version
            )));
        }

        validate_formats("album_art", &self.album_art)?;
        validate_formats("image_specifications", &self.image_specifications)?;
        validate_formats("chapter_image_specs", &self.chapter_image_specs)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageFormat {
    pub format_id: u32,
    pub render_width: u32,
    pub render_height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_width: Option<u32>,
    pub pixel_format: String,
    pub interlaced: bool,
    pub crop: bool,
    pub align_row_bytes: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back_color: Option<String>,
    pub color_adjustment: i32,
    pub gamma_adjustment: f64,
    pub associated_format: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded_formats: Option<i64>,
}

impl ImageFormat {
    fn validate(&self, collection: &str) -> Result<(), CapabilityProfileError> {
        if self.format_id == 0 || self.render_width == 0 || self.render_height == 0 {
            return Err(CapabilityProfileError::Invalid(format!(
                "{collection} format {} has a zero ID or render dimension",
                self.format_id
            )));
        }
        if self.display_width == Some(0) {
            return Err(CapabilityProfileError::Invalid(format!(
                "{collection} format {} has a zero display width",
                self.format_id
            )));
        }
        if !is_upper_hex_word(&self.pixel_format) {
            return Err(CapabilityProfileError::Invalid(format!(
                "{collection} format {} has an invalid pixel format",
                self.format_id
            )));
        }
        if self
            .back_color
            .as_deref()
            .is_some_and(|color| !is_upper_hex_word(color))
        {
            return Err(CapabilityProfileError::Invalid(format!(
                "{collection} format {} has an invalid background colour",
                self.format_id
            )));
        }
        if !self.gamma_adjustment.is_finite() || self.gamma_adjustment <= 0.0 {
            return Err(CapabilityProfileError::Invalid(format!(
                "{collection} format {} has an invalid gamma adjustment",
                self.format_id
            )));
        }
        Ok(())
    }
}

fn validate_formats(
    collection: &str,
    formats: &[ImageFormat],
) -> Result<(), CapabilityProfileError> {
    if formats.is_empty() {
        return Err(CapabilityProfileError::Invalid(format!(
            "{collection} must contain at least one complete format"
        )));
    }

    let mut format_ids = HashSet::with_capacity(formats.len());
    for format in formats {
        if !format_ids.insert(format.format_id) {
            return Err(CapabilityProfileError::Invalid(format!(
                "{collection} repeats format ID {}",
                format.format_id
            )));
        }
        format.validate(collection)?;
    }
    Ok(())
}

fn is_upper_hex_word(value: &str) -> bool {
    value.len() == 8
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
}

#[derive(Debug)]
pub enum CapabilityProfileError {
    Json(serde_json::Error),
    Invalid(String),
}

impl fmt::Display for CapabilityProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid capability profile JSON: {error}"),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CapabilityProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<serde_json::Error> for CapabilityProfileError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
