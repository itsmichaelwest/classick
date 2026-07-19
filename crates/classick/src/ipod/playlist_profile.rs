use crate::ipod::playlist_audit::{
    PlaylistSnapshot, SmartPreferencesSnapshot, SmartRuleSnapshot, SmartRulesHeaderSnapshot,
};
use crate::ipod::playlist_ownership::{ManagedPlaylistKind, ManagedPlaylistOwnership};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FirmwareProfileId {
    IpodClassicVideoKindV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FirmwarePlaylistProfile {
    pub profile_id: FirmwareProfileId,
    pub is_master: bool,
    pub is_podcast: bool,
    pub is_smart: bool,
    pub member_count: usize,
    pub preferences: SmartPreferencesSnapshot,
    pub rules_header: SmartRulesHeaderSnapshot,
    pub rules: Vec<SmartRuleSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForeignReason {
    UnrecordedId,
    InvalidManagedTarget,
    UnknownSystemSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaylistClassification {
    Managed { slug: String },
    FirmwareSystem { profile: FirmwareProfileId },
    Foreign { reason: ForeignReason },
}

pub fn firmware_profile(id: FirmwareProfileId) -> &'static FirmwarePlaylistProfile {
    static IPOD_CLASSIC_VIDEO_KIND_V1: OnceLock<FirmwarePlaylistProfile> = OnceLock::new();
    match id {
        FirmwareProfileId::IpodClassicVideoKindV1 => IPOD_CLASSIC_VIDEO_KIND_V1.get_or_init(|| {
            serde_json::from_str(include_str!(
                "../../tests/fixtures/ipod-classic-video-kind-v1.json"
            ))
            .expect("bundled firmware playlist profile must be valid")
        }),
    }
}

pub fn match_firmware_profile(playlist: &PlaylistSnapshot) -> Option<FirmwareProfileId> {
    let id = FirmwareProfileId::IpodClassicVideoKindV1;
    matches_profile(playlist, firmware_profile(id)).then_some(id)
}

pub fn classify_playlist(
    playlist: &PlaylistSnapshot,
    managed: &ManagedPlaylistOwnership,
) -> PlaylistClassification {
    if let Some((slug, entry)) = managed
        .playlists
        .iter()
        .find(|(_, entry)| entry.apple_playlist_id == playlist.id)
    {
        let valid = entry.expected_kind == ManagedPlaylistKind::Normal
            && !playlist.is_master
            && !playlist.is_podcast
            && !playlist.is_smart;
        return if valid {
            PlaylistClassification::Managed { slug: slug.clone() }
        } else {
            PlaylistClassification::Foreign {
                reason: ForeignReason::InvalidManagedTarget,
            }
        };
    }
    if let Some(profile) = match_firmware_profile(playlist) {
        return PlaylistClassification::FirmwareSystem { profile };
    }
    let reason = if playlist.is_master || playlist.is_podcast || playlist.is_smart {
        ForeignReason::UnknownSystemSignature
    } else {
        ForeignReason::UnrecordedId
    };
    PlaylistClassification::Foreign { reason }
}

fn matches_profile(playlist: &PlaylistSnapshot, profile: &FirmwarePlaylistProfile) -> bool {
    playlist.is_master == profile.is_master
        && playlist.is_podcast == profile.is_podcast
        && playlist.is_smart == profile.is_smart
        && playlist.member_count == profile.member_count
        && playlist.preferences == profile.preferences
        && playlist.rules_header == profile.rules_header
        && playlist.rules == profile.rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipod::playlist_ownership::{
        ManagedPlaylistEntry, MANAGED_PLAYLIST_OWNERSHIP_VERSION,
    };
    use std::collections::BTreeMap;

    #[test]
    fn exact_profile_matches_independent_of_name_id_and_timestamp() {
        let mut first = fixture_snapshot("Videos", 7, 100);
        let mut localized = fixture_snapshot("Videos locales", 99, 200);
        first.name = Some("Videos".into());
        localized.name = Some("Vidéos".into());
        assert_eq!(
            match_firmware_profile(&first),
            Some(FirmwareProfileId::IpodClassicVideoKindV1)
        );
        assert_eq!(
            match_firmware_profile(&localized),
            Some(FirmwareProfileId::IpodClassicVideoKindV1)
        );
    }

    #[test]
    fn every_near_match_is_foreign() {
        for (field, mutate) in near_match_mutations() {
            let mut snapshot = fixture_snapshot("Videos", 7, 100);
            mutate(&mut snapshot);
            assert_eq!(
                match_firmware_profile(&snapshot),
                None,
                "matched after mutating {field}"
            );
        }
    }

    #[test]
    fn managed_requires_exact_id_and_normal_structure() {
        let ownership = ownership("SERIAL", "mix", 42);
        let normal = normal_snapshot("Mix", 42);
        let mut smart = normal.clone();
        smart.is_smart = true;
        assert!(matches!(
            classify_playlist(&normal, &ownership),
            PlaylistClassification::Managed { slug } if slug == "mix"
        ));
        assert!(matches!(
            classify_playlist(&smart, &ownership),
            PlaylistClassification::Foreign {
                reason: ForeignReason::InvalidManagedTarget
            }
        ));
    }

    #[test]
    fn managed_id_is_considered_before_firmware_signature() {
        let snapshot = fixture_snapshot("Videos", 42, 100);
        let ownership = ownership("SERIAL", "mix", 42);
        assert!(matches!(
            classify_playlist(&snapshot, &ownership),
            PlaylistClassification::Foreign {
                reason: ForeignReason::InvalidManagedTarget
            }
        ));
    }

    fn fixture_snapshot(name: &str, id: u64, timestamp: i64) -> PlaylistSnapshot {
        let profile = firmware_profile(FirmwareProfileId::IpodClassicVideoKindV1);
        PlaylistSnapshot {
            id,
            name: Some(name.into()),
            timestamp,
            member_count: profile.member_count,
            sort_order: 0,
            is_master: profile.is_master,
            is_podcast: profile.is_podcast,
            is_smart: profile.is_smart,
            preferences: profile.preferences.clone(),
            rules_header: profile.rules_header.clone(),
            rules: profile.rules.clone(),
        }
    }

    fn normal_snapshot(name: &str, id: u64) -> PlaylistSnapshot {
        let mut snapshot = fixture_snapshot(name, id, 0);
        snapshot.is_smart = false;
        snapshot.preferences = empty_preferences();
        snapshot.rules_header = empty_rules_header();
        snapshot.rules.clear();
        snapshot
    }

    fn ownership(serial: &str, slug: &str, id: u64) -> ManagedPlaylistOwnership {
        ManagedPlaylistOwnership {
            schema_version: MANAGED_PLAYLIST_OWNERSHIP_VERSION,
            device_serial: serial.into(),
            playlists: BTreeMap::from([(
                slug.into(),
                ManagedPlaylistEntry {
                    apple_playlist_id: id,
                    expected_kind: ManagedPlaylistKind::Normal,
                    rockbox: None,
                },
            )]),
        }
    }

    fn empty_preferences() -> SmartPreferencesSnapshot {
        SmartPreferencesSnapshot {
            liveupdate: 0,
            checkrules: 0,
            checklimits: 0,
            limittype: 0,
            limitsort: 0,
            limitvalue: 0,
            matchcheckedonly: 0,
            reserved_int1: 0,
            reserved_int2: 0,
            reserved1_is_null: true,
            reserved2_is_null: true,
        }
    }

    fn empty_rules_header() -> SmartRulesHeaderSnapshot {
        SmartRulesHeaderSnapshot {
            unk004: 0,
            match_operator: 0,
            reserved_int1: 0,
            reserved_int2: 0,
            reserved1_is_null: true,
            reserved2_is_null: true,
        }
    }

    type Mutation = (&'static str, fn(&mut PlaylistSnapshot));

    fn near_match_mutations() -> Vec<Mutation> {
        vec![
            ("member_count", |s| s.member_count += 1),
            ("is_master", |s| s.is_master = !s.is_master),
            ("is_podcast", |s| s.is_podcast = !s.is_podcast),
            ("is_smart", |s| s.is_smart = !s.is_smart),
            ("preferences.liveupdate", |s| s.preferences.liveupdate ^= 1),
            ("preferences.checkrules", |s| s.preferences.checkrules ^= 1),
            ("preferences.checklimits", |s| {
                s.preferences.checklimits ^= 1
            }),
            ("preferences.limittype", |s| s.preferences.limittype += 1),
            ("preferences.limitsort", |s| s.preferences.limitsort += 1),
            ("preferences.limitvalue", |s| s.preferences.limitvalue += 1),
            ("preferences.matchcheckedonly", |s| {
                s.preferences.matchcheckedonly ^= 1
            }),
            ("preferences.reserved_int1", |s| {
                s.preferences.reserved_int1 += 1
            }),
            ("preferences.reserved_int2", |s| {
                s.preferences.reserved_int2 += 1
            }),
            ("preferences.reserved1", |s| {
                s.preferences.reserved1_is_null = false
            }),
            ("preferences.reserved2", |s| {
                s.preferences.reserved2_is_null = false
            }),
            ("rules_header.unk004", |s| s.rules_header.unk004 += 1),
            ("rules_header.match_operator", |s| {
                s.rules_header.match_operator += 1
            }),
            ("rules_header.reserved_int1", |s| {
                s.rules_header.reserved_int1 += 1
            }),
            ("rules_header.reserved_int2", |s| {
                s.rules_header.reserved_int2 += 1
            }),
            ("rules_header.reserved1", |s| {
                s.rules_header.reserved1_is_null = false
            }),
            ("rules_header.reserved2", |s| {
                s.rules_header.reserved2_is_null = false
            }),
            ("rules.count", |s| {
                s.rules.pop();
            }),
            ("rules.order", |s| s.rules.swap(0, 1)),
            ("rule.field", |s| s.rules[0].field += 1),
            ("rule.action", |s| s.rules[0].action += 1),
            ("rule.string", |s| s.rules[0].string = Some("near".into())),
            ("rule.fromvalue", |s| s.rules[0].fromvalue += 1),
            ("rule.fromdate", |s| s.rules[0].fromdate += 1),
            ("rule.fromunits", |s| s.rules[0].fromunits += 1),
            ("rule.tovalue", |s| s.rules[0].tovalue += 1),
            ("rule.todate", |s| s.rules[0].todate += 1),
            ("rule.tounits", |s| s.rules[0].tounits += 1),
            ("rule.unk052", |s| s.rules[0].unk052 += 1),
            ("rule.unk056", |s| s.rules[0].unk056 += 1),
            ("rule.unk060", |s| s.rules[0].unk060 += 1),
            ("rule.unk064", |s| s.rules[0].unk064 += 1),
            ("rule.unk068", |s| s.rules[0].unk068 += 1),
            ("rule.reserved_int1", |s| s.rules[0].reserved_int1 += 1),
            ("rule.reserved_int2", |s| s.rules[0].reserved_int2 += 1),
            ("rule.reserved1", |s| s.rules[0].reserved1_is_null = false),
            ("rule.reserved2", |s| s.rules[0].reserved2_is_null = false),
        ]
    }
}
