mod hello;
mod routing;

use anyhow::{bail, Context, Result};
pub use hello::{
    validate_peer_hello, CapabilityName, EndpointRole, WireHello, WIRE_PROTOCOL_VERSION,
};
pub use routing::{PromptId, RequestId, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireMessage {
    Hello(WireHello),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmittedStream {
    DesktopReceivingDaemonEvents,
    DaemonReceivingDesktopCommands,
    DaemonReceivingWorkerEvents,
    WorkerReceivingDaemonCommands,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedWireMessage {
    Known(WireMessage),
    IgnoredUnknownEvent { message_type: String },
}

pub fn decode_initial_hello(json: &str) -> Result<WireHello> {
    let value: Value = serde_json::from_str(json).context("decode initial wire message")?;
    let message_type = message_type(&value)?;
    if message_type != "hello" {
        bail!("first wire message must be hello, not {message_type}");
    }
    decode_hello(value)
}

pub fn decode_admitted_message(json: &str, stream: AdmittedStream) -> Result<DecodedWireMessage> {
    let value: Value = serde_json::from_str(json).context("decode admitted wire message")?;
    let message_type = message_type(&value)?;
    if message_type == "hello" {
        bail!("hello is only valid as the first wire message");
    }
    if stream == AdmittedStream::DesktopReceivingDaemonEvents {
        return Ok(DecodedWireMessage::IgnoredUnknownEvent {
            message_type: message_type.to_owned(),
        });
    }
    bail!("unknown {message_type} message is not valid on {stream:?}")
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
