use anyhow::{bail, Result};
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

    fn has_valid_provenance(&self) -> bool {
        matches!(
            (self.source, self.confidence),
            (
                FactSource::Reported | FactSource::Decoded,
                FactConfidence::Certain
            ) | (FactSource::Inferred, FactConfidence::Heuristic)
        )
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
    BlackRed,
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

impl HardwareFacts {
    pub fn validate(&self) -> Result<()> {
        for (name, valid) in [
            (
                "family",
                self.family.as_ref().is_none_or(Fact::has_valid_provenance),
            ),
            (
                "colour",
                self.colour.as_ref().is_none_or(Fact::has_valid_provenance),
            ),
            (
                "capacity",
                self.capacity_bytes
                    .as_ref()
                    .is_none_or(Fact::has_valid_provenance),
            ),
        ] {
            if !valid {
                bail!("hardware {name} fact has inconsistent provenance");
            }
        }
        for (name, fact) in [
            ("generation", self.generation.as_ref()),
            ("model code", self.model_code.as_ref()),
            ("firmware", self.firmware.as_ref()),
        ] {
            if let Some(fact) = fact {
                if !fact.has_valid_provenance() || fact.value.is_empty() {
                    bail!("hardware {name} fact is empty or has inconsistent provenance");
                }
            }
        }
        if self
            .capacity_bytes
            .as_ref()
            .is_some_and(|fact| fact.value == 0)
        {
            bail!("hardware capacity fact must be nonzero");
        }
        Ok(())
    }
}
