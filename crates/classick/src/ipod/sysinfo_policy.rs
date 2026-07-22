//! Operation-specific admission policy for existing `SysInfoExtended` state.

use super::{
    decide_sysinfo_extended, inspect_foreign_sysinfo_extended, CapabilityProfileId,
    ForeignImageFormat, ForeignSysInfoInspection, ForeignSysInfoIssue, ForeignSysInfoStableField,
    SysInfoExtendedDecision, SysInfoExtendedProjection,
};
use crate::device::{DeviceId, DeviceReadiness};
use crate::portable::profile::{ContentHash, PortableProfile};
use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedSysInfoAuthority {
    device_id: DeviceId,
    capability_profile_id: CapabilityProfileId,
    content_hash: ContentHash,
}

impl OwnedSysInfoAuthority {
    pub fn from_portable_profile(profile: &PortableProfile) -> Result<Option<Self>> {
        profile.validate()?;
        let (Some(capability_profile_id), Some(content_hash)) = (
            profile.capability_profile_id.clone(),
            profile.generated_sysinfo_extended_hash.clone(),
        ) else {
            return Ok(None);
        };
        Ok(Some(Self {
            device_id: profile.device_id.clone(),
            capability_profile_id,
            content_hash,
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysInfoArtworkBlockReason {
    DeviceNotReady,
    ProjectionDeviceMismatch,
    OwnershipAuthorityMismatch,
    ForeignMalformed,
    ForeignIdentityInvalid,
    ForeignArtworkFactsInvalid,
    ForeignAlbumArtIncomplete,
    OwnedProjectionNeedsReplacement,
    OwnedHashMismatch,
}

/// Whether an Apple artwork operation can safely consume current capability
/// state. Authorizing outcomes carry the exact assessed projection or parsed
/// foreign capability snapshot; this decision never writes or claims bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum SysInfoArtworkAdmission<'a> {
    GenerateInTransaction {
        projection: &'a SysInfoExtendedProjection,
    },
    UseOwnedProjection {
        projection: &'a SysInfoExtendedProjection,
    },
    UseForeign {
        existing_bytes: &'a [u8],
        album_art: Vec<ForeignImageFormat>,
    },
    Blocked {
        existing_bytes: Option<&'a [u8]>,
        reason: SysInfoArtworkBlockReason,
    },
}

pub fn assess_sysinfo_for_artwork<'a>(
    device_id: &DeviceId,
    readiness: DeviceReadiness,
    existing: Option<&'a [u8]>,
    expected: &'a SysInfoExtendedProjection,
    owned_authority: Option<&OwnedSysInfoAuthority>,
) -> SysInfoArtworkAdmission<'a> {
    if readiness != DeviceReadiness::Ready {
        return blocked(existing, SysInfoArtworkBlockReason::DeviceNotReady);
    }
    if expected.device_id() != device_id {
        return blocked(
            existing,
            SysInfoArtworkBlockReason::ProjectionDeviceMismatch,
        );
    }
    let owned_hash = match owned_authority {
        Some(authority)
            if &authority.device_id == device_id
                && &authority.capability_profile_id == expected.capability_profile_id() =>
        {
            Some(&authority.content_hash)
        }
        Some(_) => {
            return blocked(
                existing,
                SysInfoArtworkBlockReason::OwnershipAuthorityMismatch,
            );
        }
        None => None,
    };

    match decide_sysinfo_extended(existing, expected, owned_hash) {
        SysInfoExtendedDecision::EligibleToGenerate => {
            SysInfoArtworkAdmission::GenerateInTransaction {
                projection: expected,
            }
        }
        SysInfoExtendedDecision::ExistingOwnedValid => {
            SysInfoArtworkAdmission::UseOwnedProjection {
                projection: expected,
            }
        }
        SysInfoExtendedDecision::OwnedConflict { existing_bytes } => {
            SysInfoArtworkAdmission::Blocked {
                existing_bytes: Some(existing_bytes),
                reason: SysInfoArtworkBlockReason::OwnedProjectionNeedsReplacement,
            }
        }
        SysInfoExtendedDecision::OwnershipMismatch { existing_bytes } => {
            SysInfoArtworkAdmission::Blocked {
                existing_bytes: Some(existing_bytes),
                reason: SysInfoArtworkBlockReason::OwnedHashMismatch,
            }
        }
        SysInfoExtendedDecision::PreserveForeign { existing_bytes } => {
            assess_foreign_artwork(device_id, existing_bytes)
        }
    }
}

fn assess_foreign_artwork<'a>(
    device_id: &DeviceId,
    existing_bytes: &'a [u8],
) -> SysInfoArtworkAdmission<'a> {
    let ForeignSysInfoInspection::Parsed {
        capability, issues, ..
    } = inspect_foreign_sysinfo_extended(existing_bytes, device_id)
    else {
        return blocked(
            Some(existing_bytes),
            SysInfoArtworkBlockReason::ForeignMalformed,
        );
    };

    if issues.iter().any(is_identity_issue) {
        return blocked(
            Some(existing_bytes),
            SysInfoArtworkBlockReason::ForeignIdentityInvalid,
        );
    }
    if issues.iter().any(|issue| {
        matches!(
            issue,
            ForeignSysInfoIssue::InvalidStableField(
                ForeignSysInfoStableField::SupportsSparseArtwork
            )
        )
    }) {
        return blocked(
            Some(existing_bytes),
            SysInfoArtworkBlockReason::ForeignArtworkFactsInvalid,
        );
    }
    match capability.album_art {
        Some(album_art) => SysInfoArtworkAdmission::UseForeign {
            existing_bytes,
            album_art,
        },
        None => blocked(
            Some(existing_bytes),
            SysInfoArtworkBlockReason::ForeignAlbumArtIncomplete,
        ),
    }
}

fn is_identity_issue(issue: &ForeignSysInfoIssue) -> bool {
    matches!(
        issue,
        ForeignSysInfoIssue::IdentityMismatch { .. }
            | ForeignSysInfoIssue::MissingStableField(ForeignSysInfoStableField::FireWireGuid)
            | ForeignSysInfoIssue::InvalidStableField(ForeignSysInfoStableField::FireWireGuid)
    )
}

fn blocked<'a>(
    existing_bytes: Option<&'a [u8]>,
    reason: SysInfoArtworkBlockReason,
) -> SysInfoArtworkAdmission<'a> {
    SysInfoArtworkAdmission::Blocked {
        existing_bytes,
        reason,
    }
}
