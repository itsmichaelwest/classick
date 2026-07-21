use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactSource {
    Reported,
    Decoded,
    Inferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactConfidence {
    Certain,
    Heuristic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact<T> {
    pub value: T,
    pub source: FactSource,
    pub confidence: FactConfidence,
}

impl<T> Fact<T> {
    pub fn reported(value: T) -> Self {
        Self {
            value,
            source: FactSource::Reported,
            confidence: FactConfidence::Certain,
        }
    }

    pub fn decoded(value: T) -> Self {
        Self {
            value,
            source: FactSource::Decoded,
            confidence: FactConfidence::Certain,
        }
    }

    pub fn inferred(value: T) -> Self {
        Self {
            value,
            source: FactSource::Inferred,
            confidence: FactConfidence::Heuristic,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpodFamily {
    Ipod,
    Classic,
    Nano,
    Mini,
    Shuffle,
    Photo,
    Video,
    Touch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpodColour {
    Silver,
    Black,
    White,
    Blue,
    Green,
    Pink,
    Red,
    Yellow,
    Purple,
    Orange,
    Gold,
    StainlessSteel,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HardwareFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<Fact<IpodFamily>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<Fact<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_code: Option<Fact<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<Fact<IpodColour>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<Fact<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_bytes: Option<Fact<u64>>,
}
