use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortablePath(String);

struct WindowsAbsolutePath {
    anchor: String,
    components: Vec<String>,
}

impl PortablePath {
    pub fn parse(value: &str) -> Result<Self> {
        if value.is_empty() {
            bail!("portable path is empty");
        }
        if value.starts_with('/') {
            bail!("portable path must be relative");
        }
        if value.contains('\\') {
            bail!("portable path must use forward slashes");
        }
        if value.as_bytes().get(1) == Some(&b':') && value.as_bytes()[0].is_ascii_alphabetic() {
            bail!("portable path must not contain a drive prefix");
        }
        if value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        {
            bail!("portable path contains an invalid component");
        }
        Ok(Self(value.to_owned()))
    }

    pub fn from_absolute(root: &Path, path: &Path) -> Result<Self> {
        match (
            WindowsAbsolutePath::parse(root)?,
            WindowsAbsolutePath::parse(path)?,
        ) {
            (Some(root), Some(path)) => return Self::from_windows_absolute(&root, &path),
            (Some(_), None) | (None, Some(_)) => {
                bail!("source root and path use different path styles")
            }
            (None, None) => {}
        }
        if !root.is_absolute() || !path.is_absolute() {
            bail!("source root and path must be absolute");
        }
        let relative = path.strip_prefix(root).with_context(|| {
            format!(
                "path {} is outside source root {}",
                path.display(),
                root.display()
            )
        })?;
        let portable = relative
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .map(str::to_owned)
                    .context("source path is not valid UTF-8")
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        Self::parse(&portable)
    }

    fn from_windows_absolute(
        root: &WindowsAbsolutePath,
        path: &WindowsAbsolutePath,
    ) -> Result<Self> {
        let same_anchor = root.anchor.eq_ignore_ascii_case(&path.anchor);
        let beneath_root = path.components.len() >= root.components.len()
            && root
                .components
                .iter()
                .zip(&path.components)
                .all(|(root, path)| root.eq_ignore_ascii_case(path));
        if !same_anchor || !beneath_root {
            bail!("path is outside source root");
        }
        Self::parse(&path.components[root.components.len()..].join("/"))
    }

    pub fn resolve(&self, root: &Path) -> PathBuf {
        root.join(&self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl WindowsAbsolutePath {
    fn parse(path: &Path) -> Result<Option<Self>> {
        let value = path.to_str().context("source path is not valid UTF-8")?;
        let normalized = value.replace('\\', "/");

        if normalized.len() >= 3
            && normalized.as_bytes()[0].is_ascii_alphabetic()
            && normalized.as_bytes()[1] == b':'
            && normalized.as_bytes()[2] == b'/'
        {
            return Ok(Some(Self {
                anchor: normalized[..2].to_ascii_lowercase(),
                components: split_windows_components(&normalized[3..]),
            }));
        }

        if let Some(remainder) = normalized.strip_prefix("//") {
            let mut components = remainder.split('/');
            let host = components.next().unwrap_or_default();
            let share = components.next().unwrap_or_default();
            if host.is_empty() || share.is_empty() {
                bail!("UNC path must include a host and share");
            }
            return Ok(Some(Self {
                anchor: format!(
                    "//{}/{}",
                    host.to_ascii_lowercase(),
                    share.to_ascii_lowercase()
                ),
                components: components
                    .filter(|component| !component.is_empty())
                    .map(str::to_owned)
                    .collect(),
            }));
        }

        Ok(None)
    }
}

fn split_windows_components(value: &str) -> Vec<String> {
    value
        .split('/')
        .filter(|component| !component.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Serialize for PortablePath {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PortablePath {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn rejects_hostile_or_non_portable_paths() {
        for value in [
            "",
            "/absolute/track.flac",
            "//server/share/track.flac",
            r"\\server\share\track.flac",
            "C:/music/track.flac",
            r"album\track.flac",
            ".",
            "./track.flac",
            "album/../track.flac",
            "album//track.flac",
            "album/",
        ] {
            assert!(PortablePath::parse(value).is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn preserves_full_unicode_source_relative_paths() {
        let value = "Björk/日本語 🎵/03 – I’m So Free.flac";

        let path = PortablePath::parse(value).unwrap();

        assert_eq!(path.as_str(), value);
        assert_eq!(
            path.resolve(Path::new("/Volumes/Music")),
            PathBuf::from("/Volumes/Music/Björk/日本語 🎵/03 – I’m So Free.flac")
        );
    }

    #[test]
    fn rebases_on_macos_and_windows_roots() {
        let relative = PortablePath::parse("Birdy/Beautiful Lies/01.flac").unwrap();

        assert_eq!(
            relative.resolve(Path::new("/Volumes/data/media/music")),
            PathBuf::from("/Volumes/data/media/music/Birdy/Beautiful Lies/01.flac")
        );
        assert_eq!(
            relative.resolve(Path::new("D:/Music")),
            PathBuf::from("D:/Music/Birdy/Beautiful Lies/01.flac")
        );
    }

    #[test]
    fn derives_a_portable_path_from_an_absolute_path() {
        let relative = PortablePath::from_absolute(
            Path::new("/Volumes/data/media/music"),
            Path::new("/Volumes/data/media/music/Beck/Colors/01.flac"),
        )
        .unwrap();

        assert_eq!(relative.as_str(), "Beck/Colors/01.flac");
    }

    #[test]
    fn derives_from_windows_drive_paths_on_any_host_os() {
        let relative = PortablePath::from_absolute(
            Path::new(r"D:\Music"),
            Path::new(r"d:\music\Beck\Colors\01.flac"),
        )
        .unwrap();

        assert_eq!(relative.as_str(), "Beck/Colors/01.flac");
    }

    #[test]
    fn derives_from_windows_unc_paths_on_any_host_os() {
        let relative = PortablePath::from_absolute(
            Path::new(r"\\JUPITER\Data\media\music"),
            Path::new(r"\\jupiter\data\media\music\Birdy\Beautiful Lies\01.flac"),
        )
        .unwrap();

        assert_eq!(relative.as_str(), "Birdy/Beautiful Lies/01.flac");
    }

    #[test]
    fn refuses_paths_outside_the_root() {
        assert!(PortablePath::from_absolute(
            Path::new("/Volumes/data/media/music"),
            Path::new("/Volumes/data/media/video/movie.m4v"),
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_non_utf8_components_instead_of_changing_the_path() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = Path::new("/Volumes/data/media/music");
        let path = root.join(OsString::from_vec(vec![0xff, b'.', b'f', b'l', b'a', b'c']));

        assert!(PortablePath::from_absolute(root, &path).is_err());
    }

    #[test]
    fn serde_representation_is_a_portable_string() {
        let path = PortablePath::parse("Artist/Album/01.flac").unwrap();
        let json = serde_json::to_string(&path).unwrap();

        assert_eq!(json, r#""Artist/Album/01.flac""#);
        assert_eq!(serde_json::from_str::<PortablePath>(&json).unwrap(), path);
    }
}
