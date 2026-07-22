use crate::device::DeviceId;
use crate::portable::profile::{
    validate_profile_components, MutationId, ProfileComponent, SelectionValue, SettingsValue,
    SubscriptionsValue,
};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConfigDelivery {
    PendingDevice {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_failure: Option<String>,
    },
    DeviceCommitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveredComponent<T> {
    pub revision: u64,
    pub mutation_id: MutationId,
    pub value: T,
    pub delivery: ConfigDelivery,
}

impl<T: Clone> DeliveredComponent<T> {
    fn revised(&self) -> ProfileComponent<T> {
        ProfileComponent {
            revision: self.revision,
            mutation_id: self.mutation_id.clone(),
            value: self.value.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceConfigSnapshot {
    pub device_id: DeviceId,
    pub selection: DeliveredComponent<SelectionValue>,
    pub settings: DeliveredComponent<SettingsValue>,
    pub subscriptions: DeliveredComponent<SubscriptionsValue>,
}

impl DeviceConfigSnapshot {
    pub(super) fn validate(&self) -> Result<()> {
        validate_profile_components(
            &self.selection.revised(),
            &self.settings.revised(),
            &self.subscriptions.revised(),
        )?;
        for delivery in [
            &self.selection.delivery,
            &self.settings.delivery,
            &self.subscriptions.delivery,
        ] {
            if matches!(delivery, ConfigDelivery::PendingDevice { last_failure: Some(message) } if message.is_empty())
            {
                bail!("pending device delivery failure requires a message");
            }
        }
        Ok(())
    }
}
