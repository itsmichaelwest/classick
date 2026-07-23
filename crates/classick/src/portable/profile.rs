use crate::device::DeviceId;
use crate::ipod::CapabilityProfileId;
use anyhow::{bail, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;

pub use super::profile_scalars::{
    ContentHash, ManagedRockboxFilename, MutationId, PlaylistSlug, ProfilePath,
};
use super::profile_values::COMPONENT_SCHEMA_VERSION;
pub use super::profile_values::{
    Revised as ProfileComponent, SelectionMode, SelectionRule, SelectionValue, SettingsValue,
    SubscriptionsValue, TranscodeProfile,
};

pub const PORTABLE_PROFILE_SCHEMA_VERSION: u32 = 1;
pub const COMPANION_AUTHORITY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplePlaylistKind {
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedRockboxPlaylist {
    pub relative_filename: ManagedRockboxFilename,
    pub content_hash: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedPlaylist {
    pub slug: PlaylistSlug,
    pub apple_playlist_id: u64,
    pub apple_kind: ApplePlaylistKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rockbox: Option<OwnedRockboxPlaylist>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompanionAuthority {
    Manifest {
        schema_version: u32,
        relative_path: ProfilePath,
        content_hash: ContentHash,
    },
    PlaylistDefinition {
        slug: PlaylistSlug,
        schema_version: u32,
        relative_path: ProfilePath,
        content_hash: ContentHash,
    },
}

impl CompanionAuthority {
    fn schema_version(&self) -> u32 {
        match self {
            Self::Manifest { schema_version, .. }
            | Self::PlaylistDefinition { schema_version, .. } => *schema_version,
        }
    }

    fn relative_path(&self) -> &ProfilePath {
        match self {
            Self::Manifest { relative_path, .. }
            | Self::PlaylistDefinition { relative_path, .. } => relative_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableProfile {
    pub schema_version: u32,
    #[serde(deserialize_with = "deserialize_canonical_device_id")]
    pub device_id: DeviceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_profile_id: Option<CapabilityProfileId>,
    pub selection: ProfileComponent<SelectionValue>,
    pub settings: ProfileComponent<SettingsValue>,
    pub subscriptions: ProfileComponent<SubscriptionsValue>,
    pub owned_playlists: Vec<OwnedPlaylist>,
    pub companion_authorities: Vec<CompanionAuthority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_sysinfo_extended_hash: Option<ContentHash>,
}

impl PortableProfile {
    pub fn from_json(json: &str) -> Result<Self> {
        let profile: Self = serde_json::from_str(json)?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn to_json_pretty(&self) -> Result<String> {
        self.validate()?;
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != PORTABLE_PROFILE_SCHEMA_VERSION {
            bail!(
                "unsupported portable profile schema {}",
                self.schema_version
            );
        }
        validate_profile_components(&self.selection, &self.settings, &self.subscriptions)?;
        if self.generated_sysinfo_extended_hash.is_some() && self.capability_profile_id.is_none() {
            bail!("owned generated SysInfoExtended requires a capability profile");
        }
        validate_ownership_and_authorities(self)
    }
}

pub(crate) fn validate_profile_components(
    selection: &ProfileComponent<SelectionValue>,
    settings: &ProfileComponent<SettingsValue>,
    subscriptions: &ProfileComponent<SubscriptionsValue>,
) -> Result<()> {
    validate_component("selection", selection, selection.value.schema_version)?;
    validate_component("settings", settings, settings.value.schema_version)?;
    validate_component(
        "subscriptions",
        subscriptions,
        subscriptions.value.schema_version,
    )?;
    let mut mutation_ids = HashSet::new();
    for mutation_id in [
        &selection.mutation_id,
        &settings.mutation_id,
        &subscriptions.mutation_id,
    ] {
        if !mutation_ids.insert(mutation_id) {
            bail!("duplicate profile mutation ID {mutation_id}");
        }
    }
    let mut slugs = HashSet::new();
    if subscriptions
        .value
        .playlists
        .iter()
        .any(|slug| !slugs.insert(slug))
    {
        bail!("duplicate subscribed playlist slug");
    }
    Ok(())
}

fn deserialize_canonical_device_id<'de, D>(
    deserializer: D,
) -> std::result::Result<DeviceId, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let device_id = DeviceId::parse(&value).map_err(serde::de::Error::custom)?;
    if value != device_id.as_str() {
        return Err(serde::de::Error::custom(
            "portable profile device ID must use its canonical uppercase spelling",
        ));
    }
    Ok(device_id)
}

fn validate_component<T>(
    name: &str,
    component: &ProfileComponent<T>,
    schema_version: u32,
) -> Result<()> {
    if component.revision == 0 {
        bail!("{name} revision must be nonzero");
    }
    if schema_version != COMPONENT_SCHEMA_VERSION {
        bail!("unsupported {name} schema {schema_version}");
    }
    Ok(())
}

fn validate_ownership_and_authorities(profile: &PortableProfile) -> Result<()> {
    let mut owned_slugs = HashSet::new();
    let mut apple_ids = HashSet::new();
    let mut rockbox_path_claims = HashSet::new();
    for owned in &profile.owned_playlists {
        if !owned_slugs.insert(&owned.slug) {
            bail!("duplicate owned playlist slug {}", owned.slug);
        }
        if owned.apple_playlist_id == 0 {
            bail!("owned playlist {} has a zero Apple playlist ID", owned.slug);
        }
        if !apple_ids.insert(owned.apple_playlist_id) {
            bail!(
                "duplicate owned Apple playlist ID {}",
                owned.apple_playlist_id
            );
        }
        if let Some(rockbox) = &owned.rockbox {
            insert_path_claim(&mut rockbox_path_claims, rockbox.relative_filename.as_str())?;
        }
    }

    let mut manifest_seen = false;
    let mut definition_slugs = HashSet::new();
    let mut authority_path_claims = HashSet::new();
    for authority in &profile.companion_authorities {
        if authority.schema_version() != COMPANION_AUTHORITY_SCHEMA_VERSION {
            bail!(
                "unsupported companion authority schema {}",
                authority.schema_version()
            );
        }
        insert_path_claim(
            &mut authority_path_claims,
            authority.relative_path().as_str(),
        )?;
        match authority {
            CompanionAuthority::Manifest { relative_path, .. } => {
                if relative_path.as_str() != "manifest.json" {
                    bail!("manifest authority path must be manifest.json");
                }
                if manifest_seen {
                    bail!("duplicate manifest authority");
                }
                manifest_seen = true;
            }
            CompanionAuthority::PlaylistDefinition {
                slug,
                relative_path,
                ..
            } => {
                let manual = format!("playlists/{slug}.m3u8");
                let smart = format!("playlists/{slug}.rules.json");
                if relative_path.as_str() != manual && relative_path.as_str() != smart {
                    bail!("playlist definition path does not match slug {slug}");
                }
                if !definition_slugs.insert(slug) {
                    bail!("duplicate playlist definition authority for {slug}");
                }
            }
        }
    }

    let subscriptions: HashSet<_> = profile.subscriptions.value.playlists.iter().collect();
    if subscriptions != definition_slugs {
        bail!("playlist definition authorities must exactly match subscriptions");
    }
    Ok(())
}

fn insert_path_claim(paths: &mut HashSet<String>, path: &str) -> Result<()> {
    let case_folded = path.to_ascii_lowercase();
    if !paths.insert(case_folded) {
        bail!("duplicate portable path claim {}", path);
    }
    Ok(())
}
