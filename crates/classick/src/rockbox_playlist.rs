use crate::ipod::playlist_ownership::{RockboxProjectionRecord, VerifiedPlaylistMembership};
use anyhow::{bail, Result};

pub const ROCKBOX_PLAYLIST_DIR: &str = "Playlists/Classick";
pub const ROCKBOX_STEM_UTF16_LIMIT: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedRockboxPlaylist {
    pub relative_filename: String,
    pub bytes: Vec<u8>,
    pub content_hash: String,
}

pub fn validate_recorded_filename(value: &str) -> Result<()> {
    if value.is_empty() || value == "." || value == ".." {
        bail!("Rockbox projection filename is empty or relative");
    }
    if !value.ends_with(".m3u8") {
        bail!("Rockbox projection filename must end in lowercase .m3u8");
    }
    if value.chars().any(|ch| {
        ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
    }) {
        bail!("Rockbox projection filename contains an unsafe character");
    }
    if value.ends_with([' ', '.']) {
        bail!("Rockbox projection filename has an unsafe trailing character");
    }
    Ok(())
}

pub fn validate_projection_record(record: &RockboxProjectionRecord) -> Result<()> {
    validate_recorded_filename(&record.relative_filename)?;
    if record.content_hash.len() != 64
        || !record
            .content_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("Rockbox projection content hash is not lowercase BLAKE3 hex");
    }
    Ok(())
}

pub fn candidate_filename(display_name: &str, slug: &str, collision_index: u32) -> String {
    let mut stem = sanitize_stem(display_name);
    if is_dos_reserved_stem(&stem) {
        stem.insert(0, '_');
    }
    stem = truncate_utf16(&stem, ROCKBOX_STEM_UTF16_LIMIT);

    let hash = if collision_index == 0 {
        blake3::hash(slug.as_bytes())
    } else {
        let mut hasher = blake3::Hasher::new();
        hasher.update(slug.as_bytes());
        hasher.update(&[0]);
        hasher.update(&collision_index.to_le_bytes());
        hasher.finalize()
    };
    let hash = hash.to_hex();
    format!("{stem}--{}.m3u8", &hash.as_str()[..10])
}

pub fn render_verified_paths(membership: &VerifiedPlaylistMembership) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for raw_path in &membership.ordered_ipod_paths {
        if raw_path.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n')) {
            bail!("verified iPod path contains a line-breaking character");
        }

        let stripped = raw_path.trim_start_matches(['/', '\\']);
        let components: Vec<&str> = stripped.split(['/', '\\']).collect();
        if components.len() < 3
            || components
                .iter()
                .any(|part| part.is_empty() || matches!(*part, "." | ".."))
            || !components[0].eq_ignore_ascii_case("iPod_Control")
            || !components[1].eq_ignore_ascii_case("Music")
        {
            bail!("verified path is outside iPod_Control/Music: {raw_path:?}");
        }

        bytes.push(b'/');
        bytes.extend_from_slice(components.join("/").as_bytes());
        bytes.push(b'\n');
    }
    Ok(bytes)
}

pub fn render_verified_playlist(
    display_name: &str,
    membership: &VerifiedPlaylistMembership,
    collision_index: u32,
) -> Result<RenderedRockboxPlaylist> {
    let bytes = render_verified_paths(membership)?;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();
    Ok(RenderedRockboxPlaylist {
        relative_filename: candidate_filename(display_name, &membership.slug, collision_index),
        bytes,
        content_hash,
    })
}

fn sanitize_stem(display_name: &str) -> String {
    let mut stem = String::new();
    let mut pending_separator = false;

    for ch in display_name.chars() {
        let replace = ch.is_whitespace()
            || ch.is_ascii_control()
            || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*');
        if replace {
            pending_separator = !stem.is_empty();
        } else {
            if pending_separator {
                stem.push('-');
                pending_separator = false;
            }
            stem.push(ch);
        }
    }

    let stem = stem.trim_matches([' ', '.', '-']);
    if stem.is_empty() {
        "Playlist".to_string()
    } else {
        stem.to_string()
    }
}

fn is_dos_reserved_stem(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || matches_dos_numbered_name(&upper, "COM")
        || matches_dos_numbered_name(&upper, "LPT")
}

fn matches_dos_numbered_name(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix)
        .is_some_and(|suffix| matches!(suffix.as_bytes(), [b'1'..=b'9']))
}

fn truncate_utf16(value: &str, limit: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|ch| {
            let next = units + ch.len_utf16();
            if next <= limit {
                units = next;
                true
            } else {
                false
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_readable_fat_safe_reserved_safe_and_stable() {
        let expected_hash = &blake3::hash(b"road-trip").to_hex().to_string()[..10];
        assert_eq!(
            candidate_filename("Road: Trip?", "road-trip", 0),
            format!("Road-Trip--{expected_hash}.m3u8")
        );
        assert!(candidate_filename("CON", "console", 0).starts_with("_CON--"));
        assert!(
            candidate_filename("日本語 プレイリスト", "jp", 0).starts_with("日本語-プレイリスト--")
        );
        let long = "🎵".repeat(81);
        let stem = candidate_filename(&long, "long", 0)
            .split("--")
            .next()
            .unwrap()
            .to_string();
        assert!(stem.encode_utf16().count() <= ROCKBOX_STEM_UTF16_LIMIT);
        assert_ne!(
            candidate_filename("Road Trip", "road-trip", 0),
            candidate_filename("Road Trip", "road-trip", 1)
        );

        let mut collision_hasher = blake3::Hasher::new();
        collision_hasher.update(b"road-trip");
        collision_hasher.update(&[0]);
        collision_hasher.update(&1_u32.to_le_bytes());
        let collision_hash = collision_hasher.finalize().to_hex();
        assert!(candidate_filename("Road Trip", "road-trip", 1)
            .ends_with(&format!("--{}.m3u8", &collision_hash.as_str()[..10])));
    }

    #[test]
    fn recorded_filename_rejects_every_escape_shape() {
        for bad in [
            "",
            ".",
            "..",
            "/Gym.m3u8",
            "C:\\Gym.m3u8",
            "a/b.m3u8",
            "a\\b.m3u8",
            "Gym.m3u",
            "Gym.M3U8",
            "Gym\n.m3u8",
            "Gym\0.m3u8",
        ] {
            assert!(validate_recorded_filename(bad).is_err(), "accepted {bad:?}");
        }
        assert!(validate_recorded_filename("Gym--0123456789.m3u8").is_ok());
    }

    #[test]
    fn recorded_hash_requires_exact_lowercase_blake3_hex() {
        let valid = RockboxProjectionRecord {
            relative_filename: "Gym--0123456789.m3u8".into(),
            content_hash: "a".repeat(64),
        };
        assert!(validate_projection_record(&valid).is_ok());
        for bad in [
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            format!("{}g", "a".repeat(63)),
        ] {
            let mut record = valid.clone();
            record.content_hash = bad;
            assert!(validate_projection_record(&record).is_err());
        }
    }

    #[test]
    fn render_is_utf8_without_bom_absolute_slash_ordered_and_hashed() {
        let membership = VerifiedPlaylistMembership {
            slug: "mix".into(),
            apple_playlist_id: 41,
            ordered_dbids: vec![102, 101],
            ordered_ipod_paths: vec![
                r"iPod_Control\Music\F02\B.m4a".into(),
                "iPod_Control/Music/F00/A.m4a".into(),
            ],
        };
        let rendered = render_verified_playlist("Mix", &membership, 0).unwrap();
        assert_eq!(
            rendered.bytes,
            b"/iPod_Control/Music/F02/B.m4a\n/iPod_Control/Music/F00/A.m4a\n"
        );
        assert!(!rendered.bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        assert_eq!(
            rendered.content_hash,
            blake3::hash(&rendered.bytes).to_hex().to_string()
        );
    }

    #[test]
    fn render_accepts_leading_separators_and_preserves_duplicates_and_case() {
        let membership = VerifiedPlaylistMembership {
            slug: "repeat".into(),
            apple_playlist_id: 42,
            ordered_dbids: vec![7, 7],
            ordered_ipod_paths: vec![
                r"\ipod_control\music\F03\Same.m4a".into(),
                "/ipod_control/music/F03/Same.m4a".into(),
            ],
        };

        assert_eq!(
            render_verified_paths(&membership).unwrap(),
            b"/ipod_control/music/F03/Same.m4a\n/ipod_control/music/F03/Same.m4a\n"
        );
    }

    #[test]
    fn render_empty_is_a_valid_zero_byte_playlist() {
        let membership = VerifiedPlaylistMembership {
            slug: "empty".into(),
            apple_playlist_id: 9,
            ordered_dbids: vec![],
            ordered_ipod_paths: vec![],
        };
        assert_eq!(
            render_verified_playlist("Empty", &membership, 0)
                .unwrap()
                .bytes,
            Vec::<u8>::new()
        );
    }

    #[test]
    fn render_rejects_host_traversal_and_line_injection() {
        for path in [
            "/Users/me/Music/a.flac",
            r"C:\Music\a.flac",
            "iPod_Control/Music/../Device/SysInfo",
            "iPod_Control/Music/F00/a\n.m4a",
            "iPod_Control/Music//a.m4a",
            "iPod_Control/Music",
        ] {
            let membership = VerifiedPlaylistMembership {
                slug: "bad".into(),
                apple_playlist_id: 1,
                ordered_dbids: vec![1],
                ordered_ipod_paths: vec![path.into()],
            };
            assert!(
                render_verified_playlist("Bad", &membership, 0).is_err(),
                "accepted {path:?}"
            );
        }
    }
}
