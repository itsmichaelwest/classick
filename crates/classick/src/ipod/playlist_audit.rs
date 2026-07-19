use crate::ffi;
use crate::ipod::playlist_ownership::ManagedPlaylistOwnership;
use crate::ipod::playlist_profile::{classify_playlist, PlaylistClassification};
use crate::ipod::OwnedDb;
use serde::{Deserialize, Serialize};
use std::ffi::CStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmartPreferencesSnapshot {
    pub liveupdate: u8,
    pub checkrules: u8,
    pub checklimits: u8,
    pub limittype: u32,
    pub limitsort: u32,
    pub limitvalue: u32,
    pub matchcheckedonly: u8,
    pub reserved_int1: i32,
    pub reserved_int2: i32,
    pub reserved1_is_null: bool,
    pub reserved2_is_null: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmartRulesHeaderSnapshot {
    pub unk004: u32,
    pub match_operator: u32,
    pub reserved_int1: i32,
    pub reserved_int2: i32,
    pub reserved1_is_null: bool,
    pub reserved2_is_null: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmartRuleSnapshot {
    pub field: u32,
    pub action: u32,
    pub string: Option<String>,
    pub fromvalue: u64,
    pub fromdate: i64,
    pub fromunits: u64,
    pub tovalue: u64,
    pub todate: i64,
    pub tounits: u64,
    pub unk052: u32,
    pub unk056: u32,
    pub unk060: u32,
    pub unk064: u32,
    pub unk068: u32,
    pub reserved_int1: i32,
    pub reserved_int2: i32,
    pub reserved1_is_null: bool,
    pub reserved2_is_null: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistSnapshot {
    pub id: u64,
    pub name: Option<String>,
    pub timestamp: i64,
    pub member_count: usize,
    pub sort_order: u32,
    pub is_master: bool,
    pub is_podcast: bool,
    pub is_smart: bool,
    pub preferences: SmartPreferencesSnapshot,
    pub rules_header: SmartRulesHeaderSnapshot,
    pub rules: Vec<SmartRuleSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClassifiedPlaylistSnapshot {
    #[serde(flatten)]
    pub playlist: PlaylistSnapshot,
    pub classification: PlaylistClassification,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InternalCategoryVisibility {
    UnsupportedByVendoredLibgpod,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistAudit {
    pub playlists: Vec<ClassifiedPlaylistSnapshot>,
    pub internal_mhsd5_categories: InternalCategoryVisibility,
}

pub fn audit_playlists(db: &OwnedDb, managed: &ManagedPlaylistOwnership) -> PlaylistAudit {
    let playlists = snapshot_playlists(db)
        .into_iter()
        .map(|playlist| ClassifiedPlaylistSnapshot {
            classification: classify_playlist(&playlist, managed),
            playlist,
        })
        .collect();
    PlaylistAudit {
        playlists,
        internal_mhsd5_categories: InternalCategoryVisibility::UnsupportedByVendoredLibgpod,
    }
}

pub fn snapshot_playlists(db: &OwnedDb) -> Vec<PlaylistSnapshot> {
    let mut snapshots = Vec::new();
    unsafe {
        let mut node = (*db.as_ptr()).playlists;
        while !node.is_null() {
            let playlist = (*node).data as *mut ffi::Itdb_Playlist;
            if !playlist.is_null() {
                snapshots.push(snapshot_playlist(playlist));
            }
            node = (*node).next;
        }
    }
    snapshots
}

unsafe fn snapshot_playlist(playlist: *mut ffi::Itdb_Playlist) -> PlaylistSnapshot {
    let playlist = unsafe { &*playlist };
    let mut member_count = 0;
    let mut member = playlist.members;
    while !member.is_null() {
        member_count += 1;
        member = unsafe { (*member).next };
    }
    let mut rules = Vec::new();
    let mut rule_node = playlist.splrules.rules;
    while !rule_node.is_null() {
        let rule = unsafe { (*rule_node).data as *const ffi::Itdb_SPLRule };
        if !rule.is_null() {
            rules.push(unsafe { snapshot_rule(&*rule) });
        }
        rule_node = unsafe { (*rule_node).next };
    }
    PlaylistSnapshot {
        id: playlist.id,
        name: unsafe { copy_string(playlist.name) },
        timestamp: playlist.timestamp as i64,
        member_count,
        sort_order: playlist.sortorder,
        is_master: unsafe { ffi::itdb_playlist_is_mpl(playlist as *const _ as *mut _) != 0 },
        is_podcast: unsafe { ffi::itdb_playlist_is_podcasts(playlist as *const _ as *mut _) != 0 },
        is_smart: playlist.is_spl != 0,
        preferences: SmartPreferencesSnapshot {
            liveupdate: playlist.splpref.liveupdate,
            checkrules: playlist.splpref.checkrules,
            checklimits: playlist.splpref.checklimits,
            limittype: playlist.splpref.limittype,
            limitsort: playlist.splpref.limitsort,
            limitvalue: playlist.splpref.limitvalue,
            matchcheckedonly: playlist.splpref.matchcheckedonly,
            reserved_int1: playlist.splpref.reserved_int1,
            reserved_int2: playlist.splpref.reserved_int2,
            reserved1_is_null: playlist.splpref.reserved1.is_null(),
            reserved2_is_null: playlist.splpref.reserved2.is_null(),
        },
        rules_header: SmartRulesHeaderSnapshot {
            unk004: playlist.splrules.unk004,
            match_operator: playlist.splrules.match_operator,
            reserved_int1: playlist.splrules.reserved_int1,
            reserved_int2: playlist.splrules.reserved_int2,
            reserved1_is_null: playlist.splrules.reserved1.is_null(),
            reserved2_is_null: playlist.splrules.reserved2.is_null(),
        },
        rules,
    }
}

unsafe fn snapshot_rule(rule: &ffi::Itdb_SPLRule) -> SmartRuleSnapshot {
    SmartRuleSnapshot {
        field: rule.field,
        action: rule.action,
        string: unsafe { copy_string(rule.string) },
        fromvalue: rule.fromvalue,
        fromdate: rule.fromdate,
        fromunits: rule.fromunits,
        tovalue: rule.tovalue,
        todate: rule.todate,
        tounits: rule.tounits,
        unk052: rule.unk052,
        unk056: rule.unk056,
        unk060: rule.unk060,
        unk064: rule.unk064,
        unk068: rule.unk068,
        reserved_int1: rule.reserved_int1,
        reserved_int2: rule.reserved_int2,
        reserved1_is_null: rule.reserved1.is_null(),
        reserved2_is_null: rule.reserved2.is_null(),
    }
}

unsafe fn copy_string(value: *const std::os::raw::c_char) -> Option<String> {
    (!value.is_null()).then(|| {
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_dtos_serialize_without_pointer_fields() {
        let json = serde_json::to_string(&SmartRuleSnapshot {
            field: 60,
            action: 1024,
            string: Some("owned".into()),
            fromvalue: 1,
            fromdate: 2,
            fromunits: 3,
            tovalue: 4,
            todate: 5,
            tounits: 6,
            unk052: 7,
            unk056: 8,
            unk060: 9,
            unk064: 10,
            unk068: 11,
            reserved_int1: 12,
            reserved_int2: 13,
            reserved1_is_null: true,
            reserved2_is_null: false,
        })
        .unwrap();
        assert!(json.contains("\"string\":\"owned\""));
        assert!(!json.contains("pointer"));
    }
}
