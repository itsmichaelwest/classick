//! Pure planning for one-time legacy host configuration import.

use super::host_cache::HostCache;
use super::outbox::{PendingDeviceOutbox, PendingMutation, OUTBOX_SCHEMA_VERSION};
use super::profile::{
    MutationId, PlaylistSlug, PortableProfile, SelectionMode, SelectionRule, SelectionValue,
    SettingsValue, SubscriptionsValue,
};
use super::profile_values::COMPONENT_SCHEMA_VERSION;
use crate::daemon::device_registry_v2::LegacyImportEligibility;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashSet;

const LEGACY_SELECTION_VERSION: u32 = 1;
const LEGACY_SETTINGS_VERSION: u32 = 1;
const LEGACY_SUBSCRIPTIONS_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy)]
pub struct LegacyHostFiles<'a> {
    pub selection: Option<&'a [u8]>,
    pub settings: Option<&'a [u8]>,
    pub subscriptions: Option<&'a [u8]>,
    pub managed_playlists: Option<&'a [u8]>,
}

#[derive(Debug, Clone, Copy)]
pub enum ResolvedLegacySelection<'a> {
    /// Neither the per-device nor shared legacy selection file exists.
    All,
    /// Exact bytes from the shared legacy selection file.
    SharedFile(&'a [u8]),
}

#[derive(Debug, Clone, Copy)]
pub enum ResolvedLegacySettings<'a> {
    /// No global daemon table exists, so the shipped daemon defaults apply.
    DaemonDefaults,
    /// Effective values resolved from the retained global config bytes.
    GlobalConfig {
        auto_sync: bool,
        rockbox_compat: bool,
        source_bytes: &'a [u8],
    },
}

#[derive(Debug, Clone, Copy)]
pub struct LegacyHostFallbacks<'a> {
    pub selection: ResolvedLegacySelection<'a>,
    pub settings: ResolvedLegacySettings<'a>,
}

#[derive(Debug, Clone, Copy)]
pub enum PortableProfileObservation<'a> {
    Absent,
    Valid(&'a PortableProfile),
    Invalid(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedLegacyHostFiles {
    pub selection: Option<Vec<u8>>,
    pub settings: Option<Vec<u8>>,
    pub subscriptions: Option<Vec<u8>>,
    pub managed_playlists: Option<Vec<u8>>,
    pub shared_selection: Option<Vec<u8>>,
    pub global_config: Option<Vec<u8>>,
}

impl RetainedLegacyHostFiles {
    fn new(files: LegacyHostFiles<'_>, fallbacks: LegacyHostFallbacks<'_>) -> Self {
        Self {
            selection: files.selection.map(<[u8]>::to_vec),
            settings: files.settings.map(<[u8]>::to_vec),
            subscriptions: files.subscriptions.map(<[u8]>::to_vec),
            managed_playlists: files.managed_playlists.map(<[u8]>::to_vec),
            shared_selection: match fallbacks.selection {
                ResolvedLegacySelection::All => None,
                ResolvedLegacySelection::SharedFile(bytes) => Some(bytes.to_vec()),
            },
            global_config: match fallbacks.settings {
                ResolvedLegacySettings::DaemonDefaults => None,
                ResolvedLegacySettings::GlobalConfig { source_bytes, .. } => {
                    Some(source_bytes.to_vec())
                }
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyMutationIds {
    pub selection: MutationId,
    pub settings: MutationId,
    pub subscriptions: MutationId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyHostImportPlan {
    Ready {
        cache: HostCache,
        outbox: PendingDeviceOutbox,
        retained_legacy: RetainedLegacyHostFiles,
    },
    Blocked {
        retained_legacy: RetainedLegacyHostFiles,
        diagnostics: Vec<String>,
    },
}

/// Plan an all-or-nothing legacy import without reading or writing files.
///
/// A valid connected portable profile remains authoritative. Otherwise the
/// complete effective legacy configuration becomes one initial outbox file
/// containing three component mutations. Legacy bytes remain retained until a
/// coordinator durably publishes the cache and outbox.
pub fn plan_legacy_host_import(
    eligibility: &LegacyImportEligibility,
    portable_profile: PortableProfileObservation<'_>,
    files: LegacyHostFiles<'_>,
    fallbacks: LegacyHostFallbacks<'_>,
    mutation_ids: LegacyMutationIds,
) -> LegacyHostImportPlan {
    let retained_legacy = RetainedLegacyHostFiles::new(files, fallbacks);
    match plan(
        eligibility,
        portable_profile,
        files,
        fallbacks,
        mutation_ids,
    ) {
        Ok((cache, outbox)) => LegacyHostImportPlan::Ready {
            cache,
            outbox,
            retained_legacy,
        },
        Err(error) => LegacyHostImportPlan::Blocked {
            retained_legacy,
            diagnostics: vec![format!("{error:#}")],
        },
    }
}

fn plan(
    eligibility: &LegacyImportEligibility,
    portable_profile: PortableProfileObservation<'_>,
    files: LegacyHostFiles<'_>,
    fallbacks: LegacyHostFallbacks<'_>,
    mutation_ids: LegacyMutationIds,
) -> Result<(HostCache, PendingDeviceOutbox)> {
    let device_id = eligibility.device_id();
    match portable_profile {
        PortableProfileObservation::Valid(profile) => {
            profile
                .validate()
                .context("validate connected portable profile")?;
            if &profile.device_id != device_id {
                bail!("connected portable profile belongs to another device");
            }
            return Ok((
                HostCache::new(device_id.clone(), Some(profile.clone()))?,
                PendingDeviceOutbox::empty(device_id.clone()),
            ));
        }
        PortableProfileObservation::Invalid(diagnostic) => {
            bail!("connected portable profile is present but invalid: {diagnostic}")
        }
        PortableProfileObservation::Absent => {}
    }

    let selection = parse_selection(files.selection, fallbacks.selection)?;
    let settings = parse_settings(files.settings, fallbacks.settings)?;
    let subscriptions = parse_subscriptions(files.subscriptions)?;
    let mutations = vec![
        PendingMutation::selection(mutation_ids.selection, device_id.clone(), selection, 0)?,
        PendingMutation::settings(mutation_ids.settings, device_id.clone(), settings, 0)?,
        PendingMutation::subscriptions(
            mutation_ids.subscriptions,
            device_id.clone(),
            subscriptions,
            0,
        )?,
    ];
    let outbox = PendingDeviceOutbox {
        schema_version: OUTBOX_SCHEMA_VERSION,
        device_id: device_id.clone(),
        mutations,
    };
    outbox.validate()?;
    Ok((HostCache::new(device_id.clone(), None)?, outbox))
}

fn parse_selection(
    bytes: Option<&[u8]>,
    fallback: ResolvedLegacySelection<'_>,
) -> Result<SelectionValue> {
    let legacy = match bytes.or_else(|| match fallback {
        ResolvedLegacySelection::All => None,
        ResolvedLegacySelection::SharedFile(bytes) => Some(bytes),
    }) {
        Some(bytes) => {
            serde_json::from_slice::<LegacySelection>(bytes).context("parse legacy selection")?
        }
        None => LegacySelection::default(),
    };
    if legacy.version != LEGACY_SELECTION_VERSION {
        bail!("unsupported legacy selection version {}", legacy.version);
    }
    Ok(SelectionValue {
        schema_version: COMPONENT_SCHEMA_VERSION,
        mode: legacy.mode,
        rules: legacy.rules.into_iter().map(SelectionRule::from).collect(),
    })
}

fn parse_settings(
    bytes: Option<&[u8]>,
    fallback: ResolvedLegacySettings<'_>,
) -> Result<SettingsValue> {
    let legacy = match bytes {
        Some(bytes) => serde_json::from_slice::<LegacySettings>(bytes)
            .context("parse legacy device settings")?,
        None => match fallback {
            ResolvedLegacySettings::DaemonDefaults => LegacySettings::default(),
            ResolvedLegacySettings::GlobalConfig {
                auto_sync,
                rockbox_compat,
                ..
            } => LegacySettings {
                version: LEGACY_SETTINGS_VERSION,
                auto_sync,
                rockbox_compat,
            },
        },
    };
    if legacy.version != LEGACY_SETTINGS_VERSION {
        bail!(
            "unsupported legacy device settings version {}",
            legacy.version
        );
    }
    Ok(SettingsValue {
        schema_version: COMPONENT_SCHEMA_VERSION,
        auto_sync: legacy.auto_sync,
        rockbox_compat: legacy.rockbox_compat,
    })
}

fn parse_subscriptions(bytes: Option<&[u8]>) -> Result<SubscriptionsValue> {
    let legacy = match bytes {
        Some(bytes) => serde_json::from_slice::<LegacySubscriptions>(bytes)
            .context("parse legacy subscriptions")?,
        None => LegacySubscriptions::default(),
    };
    if legacy.version != LEGACY_SUBSCRIPTIONS_VERSION {
        bail!(
            "unsupported legacy subscriptions version {}",
            legacy.version
        );
    }
    let mut unique = HashSet::new();
    let mut playlists = Vec::with_capacity(legacy.playlists.len());
    for value in legacy.playlists {
        let slug = PlaylistSlug::parse(&value)
            .with_context(|| format!("validate legacy subscription slug {value:?}"))?;
        if !unique.insert(slug.clone()) {
            bail!("duplicate legacy subscription slug {slug}");
        }
        playlists.push(slug);
    }
    Ok(SubscriptionsValue {
        schema_version: COMPONENT_SCHEMA_VERSION,
        playlists,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySelection {
    version: u32,
    #[serde(default = "default_selection_mode")]
    mode: SelectionMode,
    #[serde(default)]
    rules: Vec<LegacySelectionRule>,
}

fn default_selection_mode() -> SelectionMode {
    SelectionMode::All
}

impl Default for LegacySelection {
    fn default() -> Self {
        Self {
            version: LEGACY_SELECTION_VERSION,
            mode: SelectionMode::All,
            rules: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum LegacySelectionRule {
    Artist { name: String },
    Album { artist: String, album: String },
    Genre { name: String },
}

impl From<LegacySelectionRule> for SelectionRule {
    fn from(rule: LegacySelectionRule) -> Self {
        match rule {
            LegacySelectionRule::Artist { name } => Self::Artist { name },
            LegacySelectionRule::Album { artist, album } => Self::Album { artist, album },
            LegacySelectionRule::Genre { name } => Self::Genre { name },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySettings {
    version: u32,
    #[serde(default = "default_true")]
    auto_sync: bool,
    #[serde(default)]
    rockbox_compat: bool,
}

impl Default for LegacySettings {
    fn default() -> Self {
        Self {
            version: LEGACY_SETTINGS_VERSION,
            auto_sync: true,
            rockbox_compat: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySubscriptions {
    version: u32,
    #[serde(default)]
    playlists: Vec<String>,
}

impl Default for LegacySubscriptions {
    fn default() -> Self {
        Self {
            version: LEGACY_SUBSCRIPTIONS_VERSION,
            playlists: Vec::new(),
        }
    }
}
