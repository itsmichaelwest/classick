use crate::portable::profile::SelectionRule;
use crate::portable_path::PortablePath;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryAlbum {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    pub tracks: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryArtist {
    pub name: String,
    pub albums: Vec<LibraryAlbum>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryGenre {
    pub name: String,
    pub tracks: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibrarySnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_root: Option<super::SourceRoot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_at_unix_secs: Option<u64>,
    pub artists: Vec<LibraryArtist>,
    pub genres: Vec<LibraryGenre>,
    pub total_tracks: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionPreview {
    pub selected_tracks: u64,
    pub selected_bytes: u64,
    pub adds: u64,
    pub removes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevicePreview {
    pub selected_tracks: u64,
    pub selected_bytes: u64,
    pub playlist_extra_tracks: u64,
    pub playlist_extra_bytes: u64,
    pub projected_free_bytes: Option<u64>,
    pub unresolved_subscriptions: Vec<crate::portable::profile::PlaylistSlug>,
}

impl LibrarySnapshot {
    pub(super) fn validate(&self) -> Result<()> {
        if self.source_root.is_none()
            && (self.scanned_at_unix_secs.is_some()
                || self.total_tracks != 0
                || self.total_bytes != 0
                || !self.artists.is_empty()
                || !self.genres.is_empty())
        {
            bail!("unconfigured library snapshot cannot contain indexed content");
        }
        if self.scanned_at_unix_secs.is_none()
            && (self.total_tracks != 0
                || self.total_bytes != 0
                || !self.artists.is_empty()
                || !self.genres.is_empty())
        {
            bail!("never-scanned library snapshot cannot contain indexed content");
        }
        for artist in &self.artists {
            validate_display_label(&artist.name, "library artist")?;
            for album in &artist.albums {
                validate_display_label(&album.name, "library album")?;
                if let Some(genre) = &album.genre {
                    validate_display_label(genre, "library album genre")?;
                }
            }
        }
        for genre in &self.genres {
            validate_display_label(&genre.name, "library genre")?;
        }
        Ok(())
    }
}

pub(super) fn validate_selection_rules(rules: &[SelectionRule]) -> Result<()> {
    if rules.iter().any(|rule| match rule {
        SelectionRule::Artist { name } | SelectionRule::Genre { name } => name.is_empty(),
        SelectionRule::Album { artist, album } => artist.is_empty() || album.is_empty(),
    }) {
        bail!("selection rules require non-empty labels");
    }
    Ok(())
}

pub(super) fn validate_paths(paths: &[PortablePath]) -> Result<()> {
    if paths.windows(2).any(|pair| pair[0] >= pair[1]) {
        bail!("resolved track paths must be unique and lexicographically sorted");
    }
    Ok(())
}

fn validate_display_label(value: &str, kind: &str) -> Result<()> {
    if value.chars().any(char::is_control) {
        bail!("{kind} must not contain control characters");
    }
    Ok(())
}
