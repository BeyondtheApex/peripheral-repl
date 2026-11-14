use crate::{bindings, error::Fault};
use serde::{Deserialize, Serialize};

pub const MAX_MESSAGE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageClass {
    Command,
    LowLatency,
    StandardLatency,
}
impl MessageClass {
    pub fn from_native(value: i32) -> Result<Self, Fault> {
        match value {
            0 => Ok(Self::Command),
            1 => Ok(Self::LowLatency),
            2 => Ok(Self::StandardLatency),
            _ => Err(Fault::new(
                "invalid_frame",
                "gip.receive",
                "unknown message class",
            )),
        }
    }
    pub fn native(self) -> i32 {
        match self {
            Self::Command => 0,
            Self::LowLatency => 1,
            Self::StandardLatency => 2,
        }
    }
}

/// Exact payload grants; neither message headers nor permissions are inferred.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteRule {
    pub vendor_id: u16,
    pub product_id: u16,
    pub message_class: MessageClass,
    pub message_id: u8,
    pub packets: Vec<String>,
}
impl WriteRule {
    pub fn allows(
        &self,
        vendor: u16,
        product: u16,
        class: MessageClass,
        id: u8,
        payload: &[u8],
    ) -> bool {
        self.vendor_id == vendor
            && self.product_id == product
            && self.message_class == class
            && self.message_id == id
            && self.packets.iter().any(|p| p == &bindings::hex(payload))
    }
}
pub fn validate_rules(rules: &[WriteRule]) -> anyhow::Result<()> {
    anyhow::ensure!(rules.len() <= 64, "maximum 64 GIP write rules");
    for rule in rules {
        anyhow::ensure!(
            !rule.packets.is_empty() && rule.packets.len() <= 256,
            "GIP rule requires 1..256 exact payloads"
        );
        for packet in &rule.packets {
            let bytes = bindings::unhex(packet).map_err(|e| anyhow::anyhow!(e.to_string()))?;
            anyhow::ensure!(
                bytes.len() <= MAX_MESSAGE && bindings::hex(&bytes) == *packet,
                "GIP payloads must be canonical lowercase hex, at most 4096 bytes"
            );
        }
    }
    Ok(())
}
