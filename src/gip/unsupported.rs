use super::{
    MessageClass,
    feed::{Feed, Filter},
};
use crate::error::Fault;
use serde_json::Value;
use std::sync::{Arc, Mutex};
pub struct NativeDevice {
    pub info: Mutex<Value>,
    pub feed: Arc<Feed>,
}
fn error() -> Fault {
    Fault::new("unsupported", "gip", "GIP transport requires Windows")
}
impl NativeDevice {
    pub fn send(&self, _: MessageClass, _: u8, _: &[u8]) -> Result<(), Fault> {
        Err(error())
    }
    pub fn battery_info(&self) -> Result<Value, Fault> {
        Err(error())
    }
}
pub fn discover(_: &Filter) -> Result<(), Fault> {
    Err(error())
}
pub fn snapshot(_: &Filter) -> Result<Vec<Value>, Fault> {
    Err(error())
}
pub fn open(_: &Value) -> Result<Arc<NativeDevice>, Fault> {
    Err(error())
}
