//! Bounded metadata only: no raw controller input or authentication tokens.
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Mutex};

static EVENTS: Mutex<VecDeque<Value>> = Mutex::new(VecDeque::new());
pub(super) fn monotonic_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}
#[cfg(windows)]
pub(super) fn record(stage: &str, detail: Value) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut events = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    if events.len() == 64 {
        events.pop_front();
    }
    events.push_back(json!({"stage":stage,"detail":detail,"unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64}));
}
pub(super) fn snapshot() -> Value {
    json!({"events":*EVENTS.lock().unwrap_or_else(|e|e.into_inner()),"scope":"backend_process","limit":64})
}
