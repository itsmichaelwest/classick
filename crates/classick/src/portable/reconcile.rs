//! Pure portable-profile reconciliation and exact readback confirmation.

use super::host_cache::HostCache;
use super::outbox::{PendingDeviceOutbox, PendingMutation};
use super::profile::{
    CompanionAuthority, ContentHash, PortableProfile, ProfileComponent, ProfilePath,
    SelectionValue, SettingsValue, SubscriptionsValue, PORTABLE_PROFILE_SCHEMA_VERSION,
};
use crate::device::DeviceId;
use crate::ipod::CapabilityProfileId;
use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, Copy)]
pub enum DeviceProfileObservation<'a> {
    Absent,
    Valid(&'a PortableProfile),
    Invalid(&'a str),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfilePublicationContext {
    pub capability_profile_id: Option<CapabilityProfileId>,
    pub companion_authorities: Vec<CompanionAuthority>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortableReconciliationPlan {
    ImportDevice {
        cache: HostCache,
    },
    PublishPending {
        publication: PlannedPortablePublication,
    },
    Blocked {
        diagnostic: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPortablePublication {
    candidate_profile: PortableProfile,
    retained_outbox: PendingDeviceOutbox,
}

impl PlannedPortablePublication {
    pub fn candidate_profile(&self) -> &PortableProfile {
        &self.candidate_profile
    }

    pub fn retained_outbox(&self) -> &PendingDeviceOutbox {
        &self.retained_outbox
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalOutboxClear {
    expected_current: PendingDeviceOutbox,
}

impl ConditionalOutboxClear {
    pub fn apply_to(&self, current: &PendingDeviceOutbox) -> Result<PendingDeviceOutbox> {
        current.validate()?;
        if current != &self.expected_current {
            bail!("durable host outbox changed after device publication was planned");
        }
        Ok(PendingDeviceOutbox::empty(current.device_id.clone()))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CompanionFileReadback<'a> {
    pub relative_path: &'a ProfilePath,
    pub bytes: Option<&'a [u8]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitConfirmation {
    Confirmed {
        cache: HostCache,
        outbox_clear: ConditionalOutboxClear,
    },
    Pending {
        retained_outbox: PendingDeviceOutbox,
        diagnostic: String,
    },
}

pub fn plan_portable_reconciliation(
    device_id: &DeviceId,
    device_profile: DeviceProfileObservation<'_>,
    outbox: &PendingDeviceOutbox,
    publication: &ProfilePublicationContext,
) -> PortableReconciliationPlan {
    match plan(device_id, device_profile, outbox, publication) {
        Ok(plan) => plan,
        Err(error) => PortableReconciliationPlan::Blocked {
            diagnostic: format!("{error:#}"),
        },
    }
}

fn plan(
    device_id: &DeviceId,
    device_profile: DeviceProfileObservation<'_>,
    outbox: &PendingDeviceOutbox,
    publication: &ProfilePublicationContext,
) -> Result<PortableReconciliationPlan> {
    outbox.validate().context("validate host outbox")?;
    if &outbox.device_id != device_id {
        bail!("host outbox belongs to another device");
    }

    let profile = match device_profile {
        DeviceProfileObservation::Invalid(diagnostic) => {
            bail!("portable profile is present but invalid: {diagnostic}")
        }
        DeviceProfileObservation::Valid(profile) => {
            profile.validate().context("validate portable profile")?;
            if &profile.device_id != device_id {
                bail!("portable profile belongs to another device");
            }
            Some(profile)
        }
        DeviceProfileObservation::Absent => None,
    };

    if outbox.mutations.is_empty() {
        let profile =
            profile.context("portable profile is absent and no host adoption is pending")?;
        return Ok(PortableReconciliationPlan::ImportDevice {
            cache: HostCache::new(device_id.clone(), Some(profile.clone()))?,
        });
    }

    let candidate_profile = match profile {
        Some(profile) => apply_pending(profile, outbox, publication)?,
        None => build_initial_profile(device_id, outbox, publication)?,
    };
    Ok(PortableReconciliationPlan::PublishPending {
        publication: PlannedPortablePublication {
            candidate_profile,
            retained_outbox: outbox.clone(),
        },
    })
}

fn apply_pending(
    current: &PortableProfile,
    outbox: &PendingDeviceOutbox,
    publication: &ProfilePublicationContext,
) -> Result<PortableProfile> {
    let mut candidate = current.clone();
    let mut subscriptions_changed = false;
    for mutation in &outbox.mutations {
        match mutation {
            PendingMutation::Selection {
                mutation_id,
                desired,
                ..
            } => {
                apply_component(&mut candidate.selection, mutation_id, desired)?;
            }
            PendingMutation::Settings {
                mutation_id,
                desired,
                ..
            } => {
                apply_component(&mut candidate.settings, mutation_id, desired)?;
            }
            PendingMutation::Subscriptions {
                mutation_id,
                desired,
                ..
            } => {
                subscriptions_changed =
                    apply_component(&mut candidate.subscriptions, mutation_id, desired)?;
            }
        }
    }
    if subscriptions_changed {
        candidate.companion_authorities.retain(|authority| {
            !matches!(authority, CompanionAuthority::PlaylistDefinition { .. })
        });
        candidate.companion_authorities.extend(
            publication
                .companion_authorities
                .iter()
                .filter(|authority| {
                    matches!(authority, CompanionAuthority::PlaylistDefinition { .. })
                })
                .cloned(),
        );
    }
    candidate
        .validate()
        .context("validate reconciled profile")?;
    Ok(candidate)
}

fn apply_component<T: Clone + PartialEq>(
    current: &mut ProfileComponent<T>,
    mutation_id: &super::profile::MutationId,
    desired: &T,
) -> Result<bool> {
    if &current.mutation_id == mutation_id {
        if &current.value == desired {
            return Ok(false);
        }
        bail!("mutation ID was reused with different desired state");
    }
    current.revision = current
        .revision
        .checked_add(1)
        .context("portable profile component revision overflow")?;
    current.mutation_id = mutation_id.clone();
    current.value = desired.clone();
    Ok(true)
}

fn build_initial_profile(
    device_id: &DeviceId,
    outbox: &PendingDeviceOutbox,
    publication: &ProfilePublicationContext,
) -> Result<PortableProfile> {
    let mut selection: Option<ProfileComponent<SelectionValue>> = None;
    let mut settings: Option<ProfileComponent<SettingsValue>> = None;
    let mut subscriptions: Option<ProfileComponent<SubscriptionsValue>> = None;
    for mutation in &outbox.mutations {
        match mutation {
            PendingMutation::Selection {
                mutation_id,
                desired,
                last_imported_device_revision,
                ..
            } => {
                require_initial_revision(*last_imported_device_revision)?;
                selection = Some(initial_component(mutation_id.clone(), desired.clone()));
            }
            PendingMutation::Settings {
                mutation_id,
                desired,
                last_imported_device_revision,
                ..
            } => {
                require_initial_revision(*last_imported_device_revision)?;
                settings = Some(initial_component(mutation_id.clone(), desired.clone()));
            }
            PendingMutation::Subscriptions {
                mutation_id,
                desired,
                last_imported_device_revision,
                ..
            } => {
                require_initial_revision(*last_imported_device_revision)?;
                subscriptions = Some(initial_component(mutation_id.clone(), desired.clone()));
            }
        }
    }
    let profile = PortableProfile {
        schema_version: PORTABLE_PROFILE_SCHEMA_VERSION,
        device_id: device_id.clone(),
        capability_profile_id: publication.capability_profile_id.clone(),
        selection: selection.context("initial adoption requires a selection mutation")?,
        settings: settings.context("initial adoption requires a settings mutation")?,
        subscriptions: subscriptions
            .context("initial adoption requires a subscriptions mutation")?,
        owned_playlists: Vec::new(),
        companion_authorities: publication.companion_authorities.clone(),
        generated_sysinfo_extended_hash: None,
    };
    profile
        .validate()
        .context("validate initial portable profile")?;
    Ok(profile)
}

fn require_initial_revision(last_imported_device_revision: u64) -> Result<()> {
    if last_imported_device_revision != 0 {
        bail!("absent portable profile conflicts with host state based on an imported revision");
    }
    Ok(())
}

fn initial_component<T>(mutation_id: super::profile::MutationId, value: T) -> ProfileComponent<T> {
    ProfileComponent {
        revision: 1,
        mutation_id,
        value,
    }
}

pub fn confirm_reconciled_profile(
    device_id: &DeviceId,
    publication: &PlannedPortablePublication,
    readback: DeviceProfileObservation<'_>,
    companion_readback: &[CompanionFileReadback<'_>],
) -> CommitConfirmation {
    match confirm(device_id, publication, readback, companion_readback) {
        Ok(confirmation) => confirmation,
        Err(error) => CommitConfirmation::Pending {
            retained_outbox: publication.retained_outbox.clone(),
            diagnostic: format!("{error:#}"),
        },
    }
}

fn confirm(
    device_id: &DeviceId,
    publication: &PlannedPortablePublication,
    readback: DeviceProfileObservation<'_>,
    companion_readback: &[CompanionFileReadback<'_>],
) -> Result<CommitConfirmation> {
    let expected = &publication.candidate_profile;
    let outbox = &publication.retained_outbox;
    expected.validate().context("validate expected profile")?;
    outbox.validate().context("validate retained host outbox")?;
    if &expected.device_id != device_id || &outbox.device_id != device_id {
        bail!("confirmation authority belongs to another device");
    }
    let actual = match readback {
        DeviceProfileObservation::Valid(actual) => actual,
        DeviceProfileObservation::Absent => bail!("portable profile readback is absent"),
        DeviceProfileObservation::Invalid(diagnostic) => {
            bail!("portable profile readback is invalid: {diagnostic}")
        }
    };
    actual
        .validate()
        .context("validate portable profile readback")?;
    if actual != expected {
        bail!("portable profile readback does not exactly match the candidate");
    }
    verify_companion_readback(expected, companion_readback)?;
    if !outbox_is_reflected_in_profile(outbox, expected) {
        bail!("retained host outbox contains intent not reflected in the candidate");
    }
    Ok(CommitConfirmation::Confirmed {
        cache: HostCache::new(device_id.clone(), Some(expected.clone()))?,
        outbox_clear: ConditionalOutboxClear {
            expected_current: outbox.clone(),
        },
    })
}

fn verify_companion_readback(
    profile: &PortableProfile,
    readback: &[CompanionFileReadback<'_>],
) -> Result<()> {
    if readback.len() != profile.companion_authorities.len() {
        bail!("companion readback count does not match candidate authorities");
    }
    for authority in &profile.companion_authorities {
        let (path, expected_hash) = match authority {
            CompanionAuthority::Manifest {
                relative_path,
                content_hash,
                ..
            }
            | CompanionAuthority::PlaylistDefinition {
                relative_path,
                content_hash,
                ..
            } => (relative_path, content_hash),
        };
        let matches: Vec<_> = readback
            .iter()
            .filter(|observation| observation.relative_path == path)
            .collect();
        if matches.len() != 1 {
            bail!("companion readback is missing or duplicates {path}");
        }
        let bytes = matches[0]
            .bytes
            .with_context(|| format!("companion readback is absent for {path}"))?;
        let actual_hash = ContentHash::parse(blake3::hash(bytes).to_hex().as_str())
            .expect("BLAKE3 always yields a canonical content hash");
        if &actual_hash != expected_hash {
            bail!("companion readback hash does not match {path}");
        }
    }
    Ok(())
}

fn outbox_is_reflected_in_profile(outbox: &PendingDeviceOutbox, profile: &PortableProfile) -> bool {
    outbox.mutations.iter().all(|mutation| match mutation {
        PendingMutation::Selection {
            mutation_id,
            desired,
            ..
        } => profile.selection.mutation_id == *mutation_id && profile.selection.value == *desired,
        PendingMutation::Settings {
            mutation_id,
            desired,
            ..
        } => profile.settings.mutation_id == *mutation_id && profile.settings.value == *desired,
        PendingMutation::Subscriptions {
            mutation_id,
            desired,
            ..
        } => {
            profile.subscriptions.mutation_id == *mutation_id
                && profile.subscriptions.value == *desired
        }
    })
}
