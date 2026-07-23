use super::device_store::{read_profile, OwnedDeviceProfile};
use super::reconcile::{
    confirm_reconciled_profile, plan_portable_reconciliation, CommitConfirmation,
    PortableReconciliationPlan, ProfilePublicationContext,
};
use super::state_store::{PortableHostState, PortableStateStore};
use crate::device::{DeviceId, DeviceReadiness};
use crate::device_coordination::DeviceMutationSession;
use crate::ipod::{
    assess_sysinfo_for_artwork, project_sysinfo_extended, resolve_validated_capability_profile,
    validated_capability_profile_id_for_model, OwnedSysInfoAuthority, SysInfoArtworkAdmission,
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectedReconciliation {
    Imported(PortableHostState),
    DeviceCommitted(PortableHostState),
    Blocked(String),
}

pub fn config_snapshot(
    state: &PortableHostState,
    last_failure: Option<String>,
) -> Result<crate::wire::DeviceConfigSnapshot> {
    let device_id = state.outbox.device_id.clone();
    let profile = state
        .cache
        .as_ref()
        .and_then(|cache| cache.last_imported_profile.as_ref());
    let pending_delivery = crate::wire::ConfigDelivery::PendingDevice { last_failure };

    let selection_pending = state
        .outbox
        .mutations
        .iter()
        .find_map(|mutation| match mutation {
            super::outbox::PendingMutation::Selection {
                mutation_id,
                desired,
                last_imported_device_revision,
                ..
            } => Some((
                mutation_id.clone(),
                desired.clone(),
                last_imported_device_revision.saturating_add(1).max(1),
            )),
            _ => None,
        });
    let settings_pending = state
        .outbox
        .mutations
        .iter()
        .find_map(|mutation| match mutation {
            super::outbox::PendingMutation::Settings {
                mutation_id,
                desired,
                last_imported_device_revision,
                ..
            } => Some((
                mutation_id.clone(),
                desired.clone(),
                last_imported_device_revision.saturating_add(1).max(1),
            )),
            _ => None,
        });
    let subscriptions_pending = state
        .outbox
        .mutations
        .iter()
        .find_map(|mutation| match mutation {
            super::outbox::PendingMutation::Subscriptions {
                mutation_id,
                desired,
                last_imported_device_revision,
                ..
            } => Some((
                mutation_id.clone(),
                desired.clone(),
                last_imported_device_revision.saturating_add(1).max(1),
            )),
            _ => None,
        });

    let selection = match (selection_pending, profile) {
        (Some((mutation_id, value, revision)), _) => crate::wire::DeliveredComponent {
            revision,
            mutation_id,
            value,
            delivery: pending_delivery.clone(),
        },
        (None, Some(profile)) => committed_component(&profile.selection),
        (None, None) => anyhow::bail!("portable selection state is unavailable"),
    };
    let settings = match (settings_pending, profile) {
        (Some((mutation_id, value, revision)), _) => crate::wire::DeliveredComponent {
            revision,
            mutation_id,
            value,
            delivery: pending_delivery.clone(),
        },
        (None, Some(profile)) => committed_component(&profile.settings),
        (None, None) => anyhow::bail!("portable settings state is unavailable"),
    };
    let subscriptions = match (subscriptions_pending, profile) {
        (Some((mutation_id, value, revision)), _) => crate::wire::DeliveredComponent {
            revision,
            mutation_id,
            value,
            delivery: pending_delivery,
        },
        (None, Some(profile)) => committed_component(&profile.subscriptions),
        (None, None) => anyhow::bail!("portable subscriptions state is unavailable"),
    };
    Ok(crate::wire::DeviceConfigSnapshot {
        device_id,
        selection,
        settings,
        subscriptions,
    })
}

pub fn committed_config_snapshot(
    profile: &super::profile::PortableProfile,
) -> crate::wire::DeviceConfigSnapshot {
    crate::wire::DeviceConfigSnapshot {
        device_id: profile.device_id.clone(),
        selection: committed_component(&profile.selection),
        settings: committed_component(&profile.settings),
        subscriptions: committed_component(&profile.subscriptions),
    }
}

fn committed_component<T: Clone>(
    component: &super::profile::ProfileComponent<T>,
) -> crate::wire::DeliveredComponent<T> {
    crate::wire::DeliveredComponent {
        revision: component.revision,
        mutation_id: component.mutation_id.clone(),
        value: component.value.clone(),
        delivery: crate::wire::ConfigDelivery::DeviceCommitted,
    }
}

pub fn reconcile_connected(
    host_root: &Path,
    session: &DeviceMutationSession,
    readiness: DeviceReadiness,
    model_code: Option<&str>,
) -> Result<ConnectedReconciliation> {
    if crate::pending_session::has_sync_transaction_material(session.mount())? {
        anyhow::bail!(
            "pending sync transaction must be recovered before portable device publication"
        );
    }
    super::device_transaction::recover(session)
        .context("recover pending portable device transaction")?;
    let store = PortableStateStore::new(host_root);
    let host = store.load(session.device_id())?;
    let current = read_profile(session.mount())?;
    let sysinfo = prepare_sysinfo(
        session.device_id(),
        readiness,
        model_code,
        &current,
        session.mount(),
    )?;
    let publication_context = ProfilePublicationContext {
        capability_profile_id: sysinfo
            .as_ref()
            .map(|prepared| prepared.capability_profile_id.clone()),
        generated_sysinfo_extended_hash: sysinfo
            .as_ref()
            .and_then(|prepared| prepared.owned_hash.clone()),
        companion_authorities: Vec::new(),
    };

    match plan_portable_reconciliation(
        session.device_id(),
        current.as_observation(),
        &host.outbox,
        &publication_context,
    ) {
        PortableReconciliationPlan::ImportDevice { cache } => Ok(
            ConnectedReconciliation::Imported(store.import_device(&cache)?),
        ),
        PortableReconciliationPlan::Blocked { diagnostic } => {
            Ok(ConnectedReconciliation::Blocked(diagnostic))
        }
        PortableReconciliationPlan::PublishPending { publication } => {
            let candidate = publication.candidate_profile();
            super::device_transaction::publish(
                session,
                candidate,
                sysinfo
                    .as_ref()
                    .and_then(|prepared| prepared.candidate_bytes.as_deref()),
            )?;
            let published = match read_profile(session.mount())? {
                OwnedDeviceProfile::Valid(profile) => profile,
                OwnedDeviceProfile::Absent => {
                    anyhow::bail!("portable profile is absent after publication")
                }
                OwnedDeviceProfile::Invalid(diagnostic) => {
                    anyhow::bail!("portable profile is invalid after publication: {diagnostic}")
                }
            };
            let companion_bytes = published
                .companion_authorities
                .iter()
                .map(|authority| {
                    let relative_path = match authority {
                        super::profile::CompanionAuthority::Manifest { relative_path, .. }
                        | super::profile::CompanionAuthority::PlaylistDefinition {
                            relative_path,
                            ..
                        } => relative_path.clone(),
                    };
                    let path = session
                        .mount()
                        .join("iPod_Control/classick")
                        .join(relative_path.as_str());
                    let bytes = match std::fs::read(&path) {
                        Ok(bytes) => Some(bytes),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!("read companion authority {}", path.display())
                            });
                        }
                    };
                    Ok((relative_path, bytes))
                })
                .collect::<Result<Vec<_>>>()?;
            let companion_readback = companion_bytes
                .iter()
                .map(
                    |(relative_path, bytes)| super::reconcile::CompanionFileReadback {
                        relative_path,
                        bytes: bytes.as_deref(),
                    },
                )
                .collect::<Vec<_>>();
            let confirmation = confirm_reconciled_profile(
                session.device_id(),
                &publication,
                super::reconcile::DeviceProfileObservation::Valid(&published),
                &companion_readback,
            );
            match confirmation {
                CommitConfirmation::Confirmed {
                    cache,
                    outbox_clear,
                } => Ok(ConnectedReconciliation::DeviceCommitted(
                    store.confirm_device_commit(&cache, &outbox_clear)?,
                )),
                CommitConfirmation::Pending { diagnostic, .. } => {
                    Ok(ConnectedReconciliation::Blocked(diagnostic))
                }
            }
        }
    }
}

pub fn publish_manifest_authority(session: &DeviceMutationSession) -> Result<bool> {
    let mut profile = match read_profile(session.mount())? {
        OwnedDeviceProfile::Valid(profile) => profile,
        OwnedDeviceProfile::Absent => return Ok(false),
        OwnedDeviceProfile::Invalid(diagnostic) => {
            anyhow::bail!("portable profile is invalid: {diagnostic}")
        }
    };
    let path = crate::device_state::portable_manifest_path(session.mount());
    let bytes = std::fs::read(&path)
        .with_context(|| format!("read published manifest {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).context("decode published manifest authority")?;
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(2) {
        anyhow::bail!("published manifest authority is not schema version 2");
    }
    let authority = super::profile::CompanionAuthority::Manifest {
        schema_version: super::profile::COMPANION_AUTHORITY_SCHEMA_VERSION,
        relative_path: super::profile::ProfilePath::parse("manifest.json")
            .expect("canonical manifest authority path"),
        content_hash: super::profile::ContentHash::parse(blake3::hash(&bytes).to_hex().as_str())
            .expect("BLAKE3 yields a canonical content hash"),
    };
    if profile
        .companion_authorities
        .iter()
        .any(|existing| existing == &authority)
    {
        return Ok(false);
    }
    profile.companion_authorities.retain(|existing| {
        !matches!(
            existing,
            super::profile::CompanionAuthority::Manifest { .. }
        )
    });
    profile.companion_authorities.push(authority);
    profile.validate()?;
    super::device_transaction::publish(session, &profile, None)?;
    match read_profile(session.mount())? {
        OwnedDeviceProfile::Valid(published) if published == profile => Ok(true),
        _ => anyhow::bail!("portable manifest authority readback differs"),
    }
}

struct PreparedSysInfo {
    capability_profile_id: crate::ipod::CapabilityProfileId,
    owned_hash: Option<super::profile::ContentHash>,
    candidate_bytes: Option<Vec<u8>>,
}

fn prepare_sysinfo(
    device_id: &DeviceId,
    readiness: DeviceReadiness,
    model_code: Option<&str>,
    current_profile: &OwnedDeviceProfile,
    mount: &Path,
) -> Result<Option<PreparedSysInfo>> {
    let Some(profile_id) = model_code.and_then(validated_capability_profile_id_for_model) else {
        return Ok(None);
    };
    let validated = resolve_validated_capability_profile(&profile_id)
        .context("resolve validated capability profile")?
        .context("validated capability profile is unavailable")?;
    let projection =
        project_sysinfo_extended(device_id, &validated).context("project SysInfoExtended")?;
    let existing = match std::fs::read(sysinfo_path(mount)) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("read existing SysInfoExtended"),
    };
    let owned = match current_profile {
        OwnedDeviceProfile::Valid(profile) => {
            OwnedSysInfoAuthority::from_portable_profile(profile)?
        }
        OwnedDeviceProfile::Absent | OwnedDeviceProfile::Invalid(_) => None,
    };
    let admission = assess_sysinfo_for_artwork(
        device_id,
        readiness,
        existing.as_deref(),
        &projection,
        owned.as_ref(),
    );
    match admission {
        SysInfoArtworkAdmission::GenerateInTransaction { .. } => Ok(Some(PreparedSysInfo {
            capability_profile_id: profile_id,
            owned_hash: Some(projection.content_hash().clone()),
            candidate_bytes: Some(projection.bytes().to_vec()),
        })),
        SysInfoArtworkAdmission::UseOwnedProjection { .. } => Ok(Some(PreparedSysInfo {
            capability_profile_id: profile_id,
            owned_hash: Some(projection.content_hash().clone()),
            candidate_bytes: None,
        })),
        SysInfoArtworkAdmission::UseForeign { .. } => Ok(Some(PreparedSysInfo {
            capability_profile_id: profile_id,
            owned_hash: None,
            candidate_bytes: None,
        })),
        SysInfoArtworkAdmission::Blocked { reason, .. } => {
            anyhow::bail!("SysInfoExtended blocks safe artwork capability use: {reason:?}")
        }
    }
}

fn sysinfo_path(mount: &Path) -> PathBuf {
    mount.join("iPod_Control/Device/SysInfoExtended")
}
