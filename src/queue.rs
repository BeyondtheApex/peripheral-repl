use crate::error::Fault;
use mlua::{LuaSerdeExt, UserData, UserDataMethods};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

/// Per-observer bounded queue. Overflow is an explicit event, never silent loss.
pub struct Queue {
    values: Mutex<VecDeque<Value>>,
    capacity: usize,
    dropped: Mutex<u64>,
    closed: AtomicBool,
    notify: Notify,
    pub cancellation: tokio_util::sync::CancellationToken,
}
impl Queue {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            values: Mutex::new(VecDeque::new()),
            capacity: capacity.clamp(1, 128),
            dropped: Mutex::new(0),
            closed: AtomicBool::new(false),
            notify: Notify::new(),
            cancellation: tokio_util::sync::CancellationToken::new(),
        })
    }
    pub fn push(&self, value: Value) {
        let value = if value.to_string().len() > 12288 {
            json!({"kind":"oversized_event","message":"event exceeds 12 KiB"})
        } else {
            value
        };
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let mut values = self.values.lock().unwrap();
        if values.len() == self.capacity {
            values.pop_front();
            *self.dropped.lock().unwrap() += 1;
        }
        values.push_back(value);
        drop(values);
        self.notify.notify_one();
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.cancellation.cancel();
        self.notify.notify_waiters();
    }
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    pub fn info(&self) -> Value {
        json!({"queued":self.values.lock().unwrap().len(),"capacity":self.capacity,"closed":self.is_closed(),"dropped":*self.dropped.lock().unwrap()})
    }
    fn pop(&self) -> Option<Value> {
        let mut values = self.values.lock().unwrap();
        let mut dropped = self.dropped.lock().unwrap();
        if *dropped > 0 {
            let count = std::mem::take(&mut *dropped);
            return Some(json!({"kind":"overflow","dropped":count}));
        }
        values.pop_front()
    }
    pub async fn next(&self, timeout_ms: u64) -> Result<Value, Fault> {
        if timeout_ms > 120000 {
            return Err(Fault::new(
                "invalid_argument",
                "stream.next",
                "timeout exceeds 120000 ms",
            ));
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(value) = self.pop() {
                return Ok(value);
            }
            if self.is_closed() {
                return Err(Fault::new("closed", "stream.next", "subscription closed"));
            }
            if timeout_ms == 0 {
                return Ok(json!({"kind":"timeout"}));
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(json!({"kind":"timeout"}));
            }
        }
    }
}

#[derive(Clone)]
pub struct Subscription(pub Arc<Queue>);
impl UserData for Subscription {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_async_method("next", |lua, this, timeout: Option<u64>| async move {
            lua.to_value(
                &this
                    .0
                    .next(timeout.unwrap_or(30000))
                    .await
                    .map_err(Fault::lua)?,
            )
        });
        m.add_method("close", |_, this, ()| {
            this.0.close();
            Ok(())
        });
        m.add_method("inspect", |lua, this, ()| lua.to_value(&this.0.info()));
    }
}
