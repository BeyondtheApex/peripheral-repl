//! Strict offline replay input validation; never truncates IDs or payloads.
use super::policy::{MAX_MESSAGE, MessageClass};
use crate::{bindings, error::Fault};
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Options {
    #[serde(default)]
    pub vendor_id: u16,
    #[serde(default)]
    pub product_id: u16,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    op: String,
    message_class: Option<MessageClass>,
    message_id: Option<u8>,
    payload_hex: Option<String>,
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    delay_ms: u64,
    error: Option<String>,
    result: Option<Value>,
    #[serde(default)]
    disconnect: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    message_class: MessageClass,
    message_id: u8,
    payload_hex: String,
    #[serde(default)]
    sequence: u8,
    #[serde(default)]
    timestamp: u64,
}
fn payload(text: &str) -> Result<(), Fault> {
    let bytes = bindings::unhex(text).map_err(|e| Fault::new("invalid_argument", "mock.gip", e))?;
    if bytes.len() > MAX_MESSAGE || bindings::hex(&bytes) != text {
        return Err(Fault::new(
            "invalid_argument",
            "mock.gip",
            "payload must be canonical lowercase hex, <=4096 bytes",
        ));
    }
    Ok(())
}
pub(super) fn validate(steps: &[Value]) -> Result<(), Fault> {
    if steps.len() > 256 || serde_json::to_vec(steps).map_or(usize::MAX, |s| s.len()) > 262144 {
        return Err(Fault::new(
            "resource_limit",
            "mock.gip",
            "replay exceeds 256 steps / 256 KiB",
        ));
    }
    for value in steps {
        let s: Step = serde_json::from_value(value.clone())
            .map_err(|e| Fault::new("invalid_argument", "mock.gip", e))?;
        if s.delay_ms > 5000
            || s.error.as_ref().is_some_and(|s| s.len() > 64)
            || s.messages.len() > 256
        {
            return Err(Fault::new(
                "invalid_argument",
                "mock.gip",
                "delay<=5000, error<=64 characters, messages<=256 required",
            ));
        }
        match s.op.as_str() {
            "send" => {
                if s.message_class.is_none() || s.message_id.is_none() || s.payload_hex.is_none() {
                    return Err(Fault::new(
                        "invalid_argument",
                        "mock.gip",
                        "send requires class, ID and payload",
                    ));
                }
                payload(s.payload_hex.as_ref().unwrap())?;
            }
            "battery_info" => {
                if s.message_class.is_some() || s.message_id.is_some() || s.payload_hex.is_some() {
                    return Err(Fault::new(
                        "invalid_argument",
                        "mock.gip",
                        "battery_info has no request packet",
                    ));
                }
            }
            _ => {
                return Err(Fault::new(
                    "invalid_argument",
                    "mock.gip",
                    "unknown operation",
                ));
            }
        }
        for m in s.messages {
            payload(&m.payload_hex)?;
            let _ = (m.message_class, m.message_id, m.sequence, m.timestamp);
        }
        let _ = (s.result, s.disconnect);
    }
    Ok(())
}
