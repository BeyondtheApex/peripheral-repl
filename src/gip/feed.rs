use super::policy::{MAX_MESSAGE, MessageClass};
use crate::{bindings, error::Fault, queue::Queue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    #[serde(default)]
    pub interface_guids: Vec<String>,
    pub wait_ms: Option<u64>,
}
impl Filter {
    pub fn validate(&self) -> Result<(), Fault> {
        if self.interface_guids.len() > 8 || self.wait_ms.unwrap_or(500) > 10000 {
            return Err(Fault::new(
                "invalid_argument",
                "gip.list",
                "maximum 8 interface GUIDs; wait_ms 0..10000",
            ));
        }
        if !self.interface_guids.is_empty()
            && (self.vendor_id.is_none() || self.product_id.is_none())
        {
            return Err(Fault::new(
                "invalid_argument",
                "gip.list",
                "interface registration requires vendor_id and product_id",
            ));
        }
        for guid in &self.interface_guids {
            uuid::Uuid::parse_str(guid)
                .map_err(|e| Fault::new("invalid_argument", "gip.list", e))?;
        }
        Ok(())
    }
    pub fn matches(&self, info: &Value) -> bool {
        self.vendor_id.is_none_or(|v| info["vendor_id"] == v)
            && self.product_id.is_none_or(|v| info["product_id"] == v)
    }
}
/// No protocol parsing or cached telemetry. Every event is a fresh observation.
pub struct Feed {
    pub connected: AtomicBool,
    pub resumed: AtomicBool,
    pub received: AtomicU64,
    pub last_message_unix_ms: AtomicU64,
    subscribers: Mutex<Vec<(Weak<Queue>, MessageFilter)>>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MessageFilter {
    pub message_class: Option<MessageClass>,
    pub message_id: Option<u8>,
}
impl MessageFilter {
    pub fn matches(&self, value: &Value) -> bool {
        value["kind"] != "message"
            || (self.message_id.is_none_or(|id| value["message_id"] == id)
                && self.message_class.is_none_or(|class| {
                    value["message_class"] == serde_json::to_value(class).unwrap()
                }))
    }
}
impl Default for Feed {
    fn default() -> Self {
        Self {
            connected: AtomicBool::new(true),
            resumed: AtomicBool::new(false),
            received: AtomicU64::new(0),
            last_message_unix_ms: AtomicU64::new(0),
            subscribers: Mutex::new(vec![]),
        }
    }
}
impl Feed {
    pub fn subscribe(&self, filter: MessageFilter) -> Result<Arc<Queue>, Fault> {
        let mut queues = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        queues.retain(|(q, _)| q.upgrade().is_some_and(|q| !q.is_closed()));
        if !self.connected.load(Ordering::Acquire) {
            return Err(Fault::new(
                "disconnected",
                "gip.subscribe",
                "device disconnected",
            ));
        }
        if queues.len() >= 32 {
            return Err(Fault::new(
                "resource_limit",
                "gip.subscribe",
                "maximum 32 subscriptions per device",
            ));
        }
        let queue = Queue::new(128);
        queues.push((Arc::downgrade(&queue), filter));
        Ok(queue)
    }
    pub fn emit(&self, value: Value) {
        let mut queues = self.subscribers.lock().unwrap_or_else(|e| e.into_inner());
        queues.retain(|(q, filter)| {
            if let Some(q) = q.upgrade().filter(|q| !q.is_closed()) {
                if filter.matches(&value) {
                    q.push(value.clone());
                }
                true
            } else {
                false
            }
        });
    }
    pub fn message(&self, timestamp: u64, class: i32, id: u8, sequence: u8, payload: &[u8]) {
        if !self.connected.load(Ordering::Acquire) {
            return;
        }
        let Ok(class) = MessageClass::from_native(class) else {
            self.emit(json!({"kind":"invalid_frame","message":"unknown GIP class"}));
            return;
        };
        if payload.len() > MAX_MESSAGE {
            self.emit(json!({"kind":"oversized_event","actual_length":payload.len()}));
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.received.fetch_add(1, Ordering::Relaxed);
        self.last_message_unix_ms.store(now, Ordering::Release);
        if self
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .all(|(q, _)| q.upgrade().is_none_or(|q| q.is_closed()))
        {
            return;
        }
        self.emit(json!({"kind":"message","message_class":class,"message_id":id,"sequence":sequence,
            "timestamp":timestamp,"received_unix_ms":now,"received_monotonic_ms":super::diagnostics::monotonic_ms(),"payload_hex":bindings::hex(payload),"actual_length":payload.len()}));
    }
    pub fn disconnect(&self) {
        if self.connected.swap(false, Ordering::AcqRel) {
            self.emit(json!({"kind":"disconnected"}));
            for q in self
                .subscribers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter_map(|(q, _)| q.upgrade())
            {
                q.close();
            }
        }
    }
    pub fn inspect(&self) -> Value {
        json!({"connected":self.connected.load(Ordering::Acquire),"input_resumed":self.resumed.load(Ordering::Acquire),
            "received_messages":self.received.load(Ordering::Relaxed),"last_message_unix_ms":self.last_message_unix_ms.load(Ordering::Acquire)})
    }
}
