use anyhow::{bail, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;
use std::fmt;

pub const WIRE_PROTOCOL_VERSION: &str = "3.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointRole {
    Desktop,
    Daemon,
    Worker,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityName(String);

impl CapabilityName {
    pub fn parse(value: &str) -> Result<Self> {
        let bytes = value.as_bytes();
        if bytes.first().is_none_or(|byte| !byte.is_ascii_lowercase())
            || bytes.last() == Some(&b'_')
            || bytes.windows(2).any(|pair| pair == b"__")
            || bytes
                .iter()
                .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'_')
        {
            bail!("capability name must be lowercase snake_case ASCII");
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CapabilityName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for CapabilityName {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CapabilityName {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireHello {
    protocol_version: String,
    role: EndpointRole,
    software_version: String,
    capabilities: Vec<CapabilityName>,
}

impl WireHello {
    pub fn new(
        role: EndpointRole,
        software_version: impl Into<String>,
        capabilities: impl IntoIterator<Item = CapabilityName>,
    ) -> Result<Self> {
        let mut hello = Self {
            protocol_version: WIRE_PROTOCOL_VERSION.to_owned(),
            role,
            software_version: software_version.into(),
            capabilities: capabilities.into_iter().collect(),
        };
        hello.capabilities.sort();
        hello.validate()?;
        Ok(hello)
    }

    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }

    pub fn role(&self) -> EndpointRole {
        self.role
    }

    pub fn software_version(&self) -> &str {
        &self.software_version
    }

    pub fn capabilities(&self) -> &[CapabilityName] {
        &self.capabilities
    }

    fn validate(&self) -> Result<()> {
        parse_semver_major(&self.protocol_version)?;
        parse_semver_major(&self.software_version)
            .map_err(|_| anyhow::anyhow!("hello software version is not semantic versioning"))?;
        let mut unique = HashSet::new();
        for capability in &self.capabilities {
            if !unique.insert(capability) {
                bail!("hello repeats capability {capability}");
            }
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for WireHello {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawHello {
            protocol_version: String,
            role: EndpointRole,
            software_version: String,
            capabilities: Vec<CapabilityName>,
        }

        let raw = RawHello::deserialize(deserializer)?;
        let mut hello = Self {
            protocol_version: raw.protocol_version,
            role: raw.role,
            software_version: raw.software_version,
            capabilities: raw.capabilities,
        };
        hello.capabilities.sort();
        hello.validate().map_err(serde::de::Error::custom)?;
        Ok(hello)
    }
}

pub fn validate_peer_hello(
    hello: &WireHello,
    expected_role: EndpointRole,
    required_capabilities: &[CapabilityName],
) -> Result<()> {
    hello.validate()?;
    let expected_major = parse_semver_major(WIRE_PROTOCOL_VERSION)?;
    let actual_major = parse_semver_major(&hello.protocol_version)?;
    if actual_major != expected_major {
        bail!("incompatible wire protocol major {actual_major}; expected {expected_major}");
    }
    if hello.role != expected_role {
        bail!(
            "unexpected peer role {:?}; expected {:?}",
            hello.role,
            expected_role
        );
    }
    for required in required_capabilities {
        if !hello.capabilities.contains(required) {
            bail!("peer does not advertise required capability {required}");
        }
    }
    Ok(())
}

fn parse_semver_major(value: &str) -> Result<u64> {
    let (without_build, build) = value
        .split_once('+')
        .map_or((value, None), |(version, build)| (version, Some(build)));
    if without_build.contains('+') {
        bail!("protocol version is not semantic versioning");
    }
    if let Some(build) = build {
        validate_identifiers(build, false)?;
    }
    let (core, pre_release) = without_build
        .split_once('-')
        .map_or((without_build, None), |(core, pre)| (core, Some(pre)));
    if core.contains('-') {
        bail!("protocol version is not semantic versioning");
    }
    if let Some(pre_release) = pre_release {
        validate_identifiers(pre_release, true)?;
    }
    let mut components = core.split('.');
    let major = parse_semver_number(components.next())?;
    if parse_semver_number(components.next()).is_err()
        || parse_semver_number(components.next()).is_err()
        || components.next().is_some()
    {
        bail!("protocol version is not semantic versioning");
    }
    Ok(major)
}

fn validate_identifiers(value: &str, reject_numeric_leading_zero: bool) -> Result<()> {
    if value.is_empty() {
        bail!("semantic version identifier must not be empty");
    }
    for identifier in value.split('.') {
        if identifier.is_empty()
            || identifier
                .bytes()
                .any(|byte| !byte.is_ascii_alphanumeric() && byte != b'-')
            || (reject_numeric_leading_zero
                && identifier.len() > 1
                && identifier.bytes().all(|byte| byte.is_ascii_digit())
                && identifier.starts_with('0'))
        {
            bail!("invalid semantic version identifier");
        }
    }
    Ok(())
}

fn parse_semver_number(value: Option<&str>) -> Result<u64> {
    let value = value.ok_or_else(|| anyhow::anyhow!("missing semantic version component"))?;
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        bail!("invalid semantic version component");
    }
    value
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid semantic version component"))
}
