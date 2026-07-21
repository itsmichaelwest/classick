use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

const REQUIRED_SHAPE: &str =
    "device ID must be exactly 16 hexadecimal characters (optional 0x prefix)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdError;

impl fmt::Display for DeviceIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REQUIRED_SHAPE)
    }
}

impl std::error::Error for DeviceIdError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn parse(value: &str) -> Result<Self, DeviceIdError> {
        let hexadecimal = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .unwrap_or(value);

        if hexadecimal.len() != 16 || !hexadecimal.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(DeviceIdError);
        }

        Ok(Self(hexadecimal.to_ascii_uppercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for DeviceId {
    type Err = DeviceIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for DeviceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DeviceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}
