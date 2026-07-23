use super::profile_scalars::{MutationId, PlaylistSlug};
use serde::{Deserialize, Serialize};

pub const COMPONENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revised<T> {
    pub revision: u64,
    pub mutation_id: MutationId,
    pub value: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    All,
    Include,
    Exclude,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionRule {
    Artist { name: String },
    Album { artist: String, album: String },
    Genre { name: String },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum TranscodeProfile {
    #[default]
    #[value(name = "alac")]
    Alac,
    #[serde(rename = "aac_256")]
    #[value(name = "aac_256")]
    Aac256,
    #[serde(rename = "aac_192")]
    #[value(name = "aac_192")]
    Aac192,
    #[serde(rename = "aac_128")]
    #[value(name = "aac_128")]
    Aac128,
}

impl TranscodeProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alac => "alac",
            Self::Aac256 => "aac_256",
            Self::Aac192 => "aac_192",
            Self::Aac128 => "aac_128",
        }
    }

    pub fn aac_bitrate_kbps(self) -> Option<u32> {
        match self {
            Self::Alac => None,
            Self::Aac256 => Some(256),
            Self::Aac192 => Some(192),
            Self::Aac128 => Some(128),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionValue {
    pub schema_version: u32,
    pub mode: SelectionMode,
    pub rules: Vec<SelectionRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsValue {
    pub schema_version: u32,
    pub auto_sync: bool,
    pub rockbox_compat: bool,
    pub transcode_profile: TranscodeProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionsValue {
    pub schema_version: u32,
    pub playlists: Vec<PlaylistSlug>,
}

#[cfg(test)]
mod tests {
    use super::{SettingsValue, TranscodeProfile};

    #[test]
    fn transcode_profiles_have_stable_wire_values() {
        for (profile, expected) in [
            (TranscodeProfile::Alac, "\"alac\""),
            (TranscodeProfile::Aac256, "\"aac_256\""),
            (TranscodeProfile::Aac192, "\"aac_192\""),
            (TranscodeProfile::Aac128, "\"aac_128\""),
        ] {
            assert_eq!(serde_json::to_string(&profile).unwrap(), expected);
            assert_eq!(
                serde_json::from_str::<TranscodeProfile>(expected).unwrap(),
                profile
            );
        }
    }

    #[test]
    fn device_settings_require_a_transcode_profile() {
        let error = serde_json::from_str::<SettingsValue>(
            r#"{"schema_version":1,"auto_sync":true,"rockbox_compat":false}"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("transcode_profile"));
    }
}
