use crate::portable_path::PortablePath;
use anyhow::{bail, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

macro_rules! string_scalar_serde {
    ($type:ty) => {
        impl Serialize for $type {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(&value).map_err(serde::de::Error::custom)
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MutationId(String);

impl MutationId {
    pub fn parse(value: &str) -> Result<Self> {
        let valid = value.len() == 36
            && value.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                }
            });
        if !valid {
            bail!("mutation ID must be a lowercase UUID");
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MutationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

string_scalar_serde!(MutationId);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(String);

impl ContentHash {
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            bail!("content hash must be 64 lowercase hexadecimal characters");
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

string_scalar_serde!(ContentHash);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlaylistSlug(String);

impl PlaylistSlug {
    pub fn parse(value: &str) -> Result<Self> {
        let valid_character = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
        let bytes = value.as_bytes();
        if bytes.first().is_none_or(|byte| !valid_character(byte))
            || bytes.last().is_none_or(|byte| !valid_character(byte))
            || bytes
                .iter()
                .any(|byte| !valid_character(byte) && *byte != b'-')
            || bytes.windows(2).any(|pair| pair == b"--")
        {
            bail!("playlist slug must be lowercase ASCII letters or digits separated by single hyphens");
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlaylistSlug {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

string_scalar_serde!(PlaylistSlug);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProfilePath(PortablePath);

impl ProfilePath {
    pub fn parse(value: &str) -> Result<Self> {
        let path = PortablePath::parse(value)?;
        if value.bytes().any(|byte| {
            byte.is_ascii_control()
                || matches!(byte, b':' | b'*' | b'?' | b'"' | b'<' | b'>' | b'|' | b'@')
        }) {
            bail!("profile path contains a non-portable character or credentials");
        }
        if value
            .split('/')
            .any(|component| component.ends_with(' ') || component.ends_with('.'))
        {
            bail!("profile path contains a component with a non-portable suffix");
        }
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for ProfilePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

string_scalar_serde!(ProfilePath);
