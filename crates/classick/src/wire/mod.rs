mod command;
mod config;
mod event;
mod hello;
mod inventory;
mod routing;

use anyhow::{bail, Context, Result};
pub use command::WireCommand;
pub use config::{ConfigDelivery, DeliveredComponent, DeviceConfigSnapshot};
pub use event::{
    ActionPlanSummary, ArtworkSummary, ConfigComponent, ConfigFailureStage, SkippedForSpace,
    StopReason, TrackResult, WireEvent,
};
pub use hello::{
    validate_peer_hello, CapabilityName, EndpointRole, WireHello, WIRE_PROTOCOL_VERSION,
};
pub use inventory::{
    DeviceInventorySnapshot, DevicePhase, IdentifiedDeviceSnapshot, ProfileStatus,
    StorageFreshness, StorageSnapshot, UnidentifiedDeviceSnapshot,
};
pub use routing::{PromptId, RequestId, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

macro_rules! message_kinds {
    ($(($variant:ident, $wire:literal, $class:ident)),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub(crate) enum MessageKind {
            $($variant),+
        }

        impl MessageKind {
            const ALL: &'static [(&'static str, MessageClass)] = &[
                $(($wire, MessageClass::$class)),+
            ];

            fn class(self) -> MessageClass {
                match self {
                    $(Self::$variant => MessageClass::$class),+
                }
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageClass {
    Hello,
    Command,
    Event,
}

message_kinds!(
    (Hello, "hello", Hello),
    (GetInventory, "get_inventory", Command),
    (SubscribeInventory, "subscribe_inventory", Command),
    (UnsubscribeInventory, "unsubscribe_inventory", Command),
    (AdoptDevice, "adopt_device", Command),
    (ForgetDevice, "forget_device", Command),
    (GetDeviceConfig, "get_device_config", Command),
    (SetSelection, "set_selection", Command),
    (SetSettings, "set_settings", Command),
    (SetSubscriptions, "set_subscriptions", Command),
    (ApplyReview, "apply_review", Command),
    (DryRunReview, "dry_run_review", Command),
    (QuitReview, "quit_review", Command),
    (PromptDecision, "prompt_decision", Command),
    (FormDecision, "form_decision", Command),
    (CancelSync, "cancel_sync", Command),
    (PauseSync, "pause_sync", Command),
    (DeviceInventory, "device_inventory", Event),
    (
        InventorySubscriptionChanged,
        "inventory_subscription_changed",
        Event
    ),
    (DeviceConfig, "device_config", Event),
    (ConfigMutationFailed, "config_mutation_failed", Event),
    (DeviceForgotten, "device_forgotten", Event),
    (RunHeader, "run_header", Event),
    (SyncSummary, "sync_summary", Event),
    (ReviewRequested, "review_requested", Event),
    (Prompt, "prompt", Event),
    (Form, "form", Event),
    (TrackStart, "track_start", Event),
    (TrackDone, "track_done", Event),
    (Finalizing, "finalizing", Event),
    (SyncCancelled, "sync_cancelled", Event),
    (SyncPaused, "sync_paused", Event),
    (SyncLog, "sync_log", Event),
    (SyncError, "sync_error", Event),
    (SyncFinished, "sync_finished", Event),
    (CommandFailed, "command_failed", Event),
);

pub fn known_message_types() -> impl Iterator<Item = &'static str> {
    MessageKind::ALL
        .iter()
        .map(|(message_type, _)| *message_type)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireMessage {
    Hello(WireHello),
    Command(WireCommand),
    Event(WireEvent),
}

impl Serialize for WireMessage {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Hello(hello) => {
                #[derive(Serialize)]
                #[serde(tag = "type", rename_all = "snake_case")]
                enum HelloEnvelope<'a> {
                    Hello(&'a WireHello),
                }
                HelloEnvelope::Hello(hello).serialize(serializer)
            }
            Self::Command(command) => {
                command.validate().map_err(serde::ser::Error::custom)?;
                command.serialize(serializer)
            }
            Self::Event(event) => {
                event.validate().map_err(serde::ser::Error::custom)?;
                event.serialize(serializer)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmittedStream {
    DesktopReceivingDaemonEvents,
    DaemonReceivingDesktopCommands,
    DaemonReceivingWorkerEvents(OwnedSessionRoute),
    WorkerReceivingDaemonCommands(WorkerCommandAdmission),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedSessionRoute {
    device_id: crate::device::DeviceId,
    session_id: SessionId,
}

impl OwnedSessionRoute {
    pub fn new(device_id: crate::device::DeviceId, session_id: SessionId) -> Self {
        Self {
            device_id,
            session_id,
        }
    }

    fn matches(&self, device_id: &crate::device::DeviceId, session_id: SessionId) -> bool {
        self.device_id == *device_id && self.session_id == session_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingWorkerInteraction {
    None,
    Review,
    Prompt {
        prompt_id: PromptId,
        option_count: u32,
    },
    Form {
        prompt_id: PromptId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCommandAdmission {
    route: OwnedSessionRoute,
    pending_interaction: PendingWorkerInteraction,
}

impl WorkerCommandAdmission {
    pub fn new(route: OwnedSessionRoute, pending_interaction: PendingWorkerInteraction) -> Self {
        Self {
            route,
            pending_interaction,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedWireMessage {
    Known(WireMessage),
    IgnoredUnknownEvent { message_type: String },
}

pub fn decode_initial_hello(json: &str) -> Result<WireHello> {
    let value: Value = serde_json::from_str(json).context("decode initial wire message")?;
    let message_type = message_type(&value)?.to_owned();
    if message_type != "hello" {
        bail!("first wire message must be hello, not {message_type}");
    }
    decode_hello(value)
}

pub fn decode_admitted_message(json: &str, stream: &AdmittedStream) -> Result<DecodedWireMessage> {
    let value: Value = serde_json::from_str(json).context("decode admitted wire message")?;
    let message_type = message_type(&value)?.to_owned();
    let Some(kind) = parse_message_kind(&message_type) else {
        if matches!(stream, AdmittedStream::DesktopReceivingDaemonEvents) {
            return Ok(DecodedWireMessage::IgnoredUnknownEvent { message_type });
        }
        bail!("unknown {message_type} message");
    };
    if kind.class() == MessageClass::Command
        && value
            .as_object()
            .is_some_and(|object| object.contains_key("observation_id"))
    {
        bail!("observation ID is not accepted by wire commands");
    }
    if kind == MessageKind::Hello {
        bail!("hello is only valid as the first wire message");
    }
    match stream {
        AdmittedStream::DesktopReceivingDaemonEvents => {
            if kind.class() == MessageClass::Command {
                bail!("{message_type} command is not valid on a desktop event stream");
            }
            decode_event(value, kind).map(DecodedWireMessage::Known)
        }
        AdmittedStream::DaemonReceivingDesktopCommands => {
            if kind.class() != MessageClass::Command {
                bail!("{message_type} event is not valid on a desktop command stream");
            }
            decode_command(value, kind).map(DecodedWireMessage::Known)
        }
        AdmittedStream::WorkerReceivingDaemonCommands(admission) => {
            if kind.class() != MessageClass::Command {
                bail!("{message_type} event is not valid on a worker command stream");
            }
            let command: WireCommand =
                serde_json::from_value(value).context("decode worker command")?;
            if command.kind() != kind {
                bail!("wire command discriminator mismatch");
            }
            validate_worker_command(&command, admission)?;
            Ok(DecodedWireMessage::Known(WireMessage::Command(command)))
        }
        AdmittedStream::DaemonReceivingWorkerEvents(expected_route) => {
            if kind.class() != MessageClass::Event {
                bail!("{message_type} command is not valid on a worker event stream");
            }
            let event: WireEvent = serde_json::from_value(value).context("decode worker event")?;
            if event.kind() != kind {
                bail!("wire event discriminator mismatch");
            }
            if !event.allowed_from_worker() {
                bail!("{message_type} is not valid worker output");
            }
            event.validate()?;
            let Some((device_id, session_id)) = event.route() else {
                bail!("{message_type} worker event has no owned-session route");
            };
            if !expected_route.matches(device_id, session_id) {
                bail!("{message_type} does not match the owned worker session");
            }
            Ok(DecodedWireMessage::Known(WireMessage::Event(event)))
        }
    }
}

fn message_type(value: &Value) -> Result<&str> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("wire message must be a JSON object"))?;
    object
        .get("type")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("wire message requires a non-empty string type"))
}

fn decode_hello(value: Value) -> Result<WireHello> {
    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum InitialMessage {
        Hello(WireHello),
    }

    match serde_json::from_value(value).context("decode hello")? {
        InitialMessage::Hello(hello) => Ok(hello),
    }
}

fn decode_command(value: Value, kind: MessageKind) -> Result<WireMessage> {
    let command: WireCommand = serde_json::from_value(value).context("decode wire command")?;
    if command.kind() != kind {
        bail!("wire command discriminator mismatch");
    }
    command.validate()?;
    Ok(WireMessage::Command(command))
}

fn decode_event(value: Value, kind: MessageKind) -> Result<WireMessage> {
    let event: WireEvent = serde_json::from_value(value).context("decode wire event")?;
    if event.kind() != kind {
        bail!("wire event discriminator mismatch");
    }
    event.validate()?;
    Ok(WireMessage::Event(event))
}

fn parse_message_kind(message_type: &str) -> Option<MessageKind> {
    serde_json::from_value(Value::String(message_type.to_owned())).ok()
}

fn validate_worker_command(
    command: &WireCommand,
    admission: &WorkerCommandAdmission,
) -> Result<()> {
    command.validate()?;
    let Some((device_id, session_id)) = command.session_route() else {
        bail!("non-session command is not valid on a worker command stream");
    };
    if !admission.route.matches(device_id, session_id) {
        bail!("command does not match the owned worker session");
    }
    match (command, &admission.pending_interaction) {
        (WireCommand::ApplyReview { .. }, PendingWorkerInteraction::Review)
        | (WireCommand::DryRunReview { .. }, PendingWorkerInteraction::Review)
        | (WireCommand::QuitReview { .. }, PendingWorkerInteraction::Review)
        | (WireCommand::CancelSync { .. }, _)
        | (WireCommand::PauseSync { .. }, _) => Ok(()),
        (
            WireCommand::PromptDecision {
                prompt_id, choice, ..
            },
            PendingWorkerInteraction::Prompt {
                prompt_id: expected,
                option_count,
            },
        ) if prompt_id == expected && *choice < *option_count => Ok(()),
        (
            WireCommand::FormDecision { prompt_id, .. },
            PendingWorkerInteraction::Form {
                prompt_id: expected,
            },
        ) if prompt_id == expected => Ok(()),
        _ => bail!("command does not match the worker's pending interaction"),
    }
}
