use super::{
    feed::{Feed, MessageFilter},
    native,
    policy::{MAX_MESSAGE, MessageClass},
};
use crate::{
    bindings::{Context, Policy, Resource, Shared},
    error::Fault,
    queue::Queue,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

pub enum Backend {
    Native(Arc<native::NativeDevice>),
    Mock(Mutex<VecDeque<Value>>),
}
pub struct Core {
    pub info: Value,
    pub feed: Arc<Feed>,
    pub input: Arc<Queue>,
    pub closed: AtomicBool,
    pub pending: AtomicUsize,
    pub gate: tokio::sync::Mutex<()>,
    native_slot: Arc<tokio::sync::Semaphore>,
    backend: Backend,
    policy: Arc<Policy>,
    shared: Arc<Shared>,
    owner: String,
    lease: AtomicBool,
    subscribers: Mutex<Vec<Weak<Queue>>>,
    replay_charge: Mutex<Option<crate::bindings::Charge>>,
}
impl Core {
    pub fn new(
        ctx: &Context,
        info: Value,
        backend: Backend,
        feed: Arc<Feed>,
        filter: MessageFilter,
    ) -> Result<Arc<Self>, Fault> {
        let path = info["path"]
            .as_str()
            .ok_or_else(|| Fault::new("invalid_argument", "gip.open", "path required"))?
            .to_owned();
        let charge = if let Backend::Mock(steps) = &backend {
            let size = serde_json::to_vec(&*steps.lock().unwrap())
                .map_err(|e| Fault::new("invalid_argument", "mock.gip", e))?
                .len();
            Some(
                crate::bindings::Charge::new(ctx.budget.clone(), size)
                    .map_err(|e| crate::error::detail(&e))?,
            )
        } else {
            None
        };
        let mut leases = ctx.shared.leases.lock().unwrap();
        if let Some(owner) = leases.get(&path) {
            return Err(Fault::new(
                "busy",
                "gip.open",
                format!("device leased by {owner}"),
            ));
        }
        let input = feed.subscribe(filter)?;
        leases.insert(path, ctx.name.clone());
        drop(leases);
        Ok(Arc::new(Self {
            info,
            feed,
            input,
            closed: AtomicBool::new(false),
            pending: AtomicUsize::new(0),
            gate: tokio::sync::Mutex::new(()),
            native_slot: Arc::new(tokio::sync::Semaphore::new(1)),
            backend,
            policy: ctx.policy.clone(),
            shared: ctx.shared.clone(),
            owner: ctx.name.clone(),
            lease: AtomicBool::new(true),
            subscribers: Mutex::new(vec![]),
            replay_charge: Mutex::new(charge),
        }))
    }
    pub fn ensure_open(&self) -> Result<(), Fault> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Fault::new("closed", "gip", "channel closed"));
        }
        if !self.feed.connected.load(Ordering::Acquire) {
            return Err(Fault::new(
                "disconnected",
                "gip",
                "device disconnected; rediscover before opening again",
            ));
        }
        Ok(())
    }
    pub fn subscribe(&self, filter: MessageFilter) -> Result<Arc<Queue>, Fault> {
        let mut queues = self.subscribers.lock().unwrap();
        self.ensure_open()?;
        let q = self.feed.subscribe(filter)?;
        queues.retain(|q| q.upgrade().is_some_and(|q| !q.is_closed()));
        queues.push(Arc::downgrade(&q));
        Ok(q)
    }
    fn release_lease(&self) {
        if self.closed.load(Ordering::Acquire)
            && self.pending.load(Ordering::Acquire) == 0
            && self.lease.swap(false, Ordering::AcqRel)
        {
            let mut leases = self.shared.leases.lock().unwrap();
            let path = self.info["path"].as_str().unwrap();
            if leases.get(path) == Some(&self.owner) {
                leases.remove(path);
            }
            drop(leases);
            if let Backend::Mock(steps) = &self.backend {
                steps.lock().unwrap().clear();
            }
            self.replay_charge.lock().unwrap().take();
        }
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.input.close();
        for q in self
            .subscribers
            .lock()
            .unwrap()
            .iter()
            .filter_map(Weak::upgrade)
        {
            q.close();
        }
        self.release_lease();
    }
    pub fn inspect(&self) -> Value {
        json!({"device":self.info,"connection":self.feed.inspect(),"closed":self.closed.load(Ordering::Acquire),"pending_native_operations":self.pending.load(Ordering::Acquire),"capabilities":{"send":"policy","receive":true,"subscriptions":true,"system_battery":true,"native_cancellation":false,"firmware_update":false}})
    }
    pub fn authorize(&self, class: MessageClass, id: u8, payload: &[u8]) -> Result<(), Fault> {
        if payload.len() > MAX_MESSAGE {
            return Err(Fault::new(
                "invalid_argument",
                "gip.send",
                "payload exceeds 4096 bytes",
            ));
        }
        let vendor = self.info["vendor_id"].as_u64().unwrap_or(0) as u16;
        let product = self.info["product_id"].as_u64().unwrap_or(0) as u16;
        if !self
            .policy
            .gip_write_rules
            .iter()
            .any(|r| r.allows(vendor, product, class, id, payload))
        {
            return Err(Fault::new(
                "policy_denied",
                "gip.send",
                "exact device, class, message ID and payload grant required",
            ));
        }
        Ok(())
    }
    pub async fn send(
        self: &Arc<Self>,
        class: MessageClass,
        id: u8,
        payload: Vec<u8>,
        timeout: u64,
    ) -> Result<Value, Fault> {
        self.authorize(class, id, &payload)?;
        self.operation(Operation::Send(class, id, payload), timeout)
            .await
    }
    pub async fn battery_info(self: &Arc<Self>, timeout: u64) -> Result<Value, Fault> {
        self.operation(Operation::Battery, timeout).await
    }
    async fn operation(
        self: &Arc<Self>,
        operation: Operation,
        timeout: u64,
    ) -> Result<Value, Fault> {
        if !(1..=120000).contains(&timeout) {
            return Err(Fault::new(
                "invalid_argument",
                "gip",
                "timeout_ms 1..120000 required",
            ));
        }
        self.ensure_open()?;
        let permit =
            self.native_slot.clone().try_acquire_owned().map_err(|_| {
                Fault::new("busy", "gip", "previous native operation still pending")
            })?;
        self.pending.fetch_add(1, Ordering::AcqRel);
        struct Completion(Arc<Core>);
        impl Drop for Completion {
            fn drop(&mut self) {
                self.0.pending.fetch_sub(1, Ordering::AcqRel);
                self.0.release_lease();
            }
        }
        let completion = Completion(self.clone());
        let core = self.clone();
        let work = tokio::task::spawn_blocking(move || {
            let _completion = completion;
            let _permit = permit;
            core.ensure_open()?;
            match &core.backend {
                Backend::Native(d) => match operation {
                    Operation::Send(class, id, payload) => {
                        d.send(class, id, &payload)?;
                        Ok(json!({"sent":payload.len()}))
                    }
                    Operation::Battery => d.battery_info(),
                },
                Backend::Mock(steps) => {
                    let step = steps
                        .lock()
                        .unwrap()
                        .pop_front()
                        .ok_or_else(|| Fault::new("mock_exhausted", "gip", "no replay step"))?;
                    match operation {
                        Operation::Send(class, id, payload) => {
                            if step["op"] != "send"
                                || step["message_class"] != serde_json::to_value(class).unwrap()
                                || step["message_id"] != id
                                || step["payload_hex"] != crate::bindings::hex(&payload)
                            {
                                return Err(Fault::new(
                                    "mock_mismatch",
                                    "gip.send",
                                    "replay request differs",
                                ));
                            }
                        }
                        Operation::Battery => {
                            if step["op"] != "battery_info" {
                                return Err(Fault::new(
                                    "mock_mismatch",
                                    "gip.battery_info",
                                    "replay operation differs",
                                ));
                            }
                        }
                    }
                    if let Some(ms) = step["delay_ms"].as_u64() {
                        std::thread::sleep(Duration::from_millis(ms.min(5000)));
                    }
                    if let Some(code) = step["error"].as_str() {
                        return Err(Fault::new(code, "gip.mock", "replayed failure"));
                    }
                    for m in step["messages"].as_array().into_iter().flatten() {
                        let bytes = crate::bindings::unhex(m["payload_hex"].as_str().unwrap_or(""))
                            .map_err(|e| Fault::new("invalid_argument", "gip.mock", e))?;
                        let class: MessageClass =
                            serde_json::from_value(m["message_class"].clone())
                                .map_err(|e| Fault::new("invalid_argument", "gip.mock", e))?;
                        core.feed.message(
                            m["timestamp"].as_u64().unwrap_or(0),
                            class.native(),
                            m["message_id"].as_u64().unwrap_or(0) as u8,
                            m["sequence"].as_u64().unwrap_or(0) as u8,
                            &bytes,
                        );
                    }
                    if step["disconnect"] == true {
                        core.feed.disconnect();
                    }
                    Ok(step.get("result").cloned().unwrap_or(json!({"sent":true})))
                }
            }
        });
        tokio::time::timeout(Duration::from_millis(timeout), work)
            .await
            .map_err(|_| {
                Fault::new(
                    "timeout",
                    "gip",
                    "native completion may still be pending; no automatic retry",
                )
            })?
            .map_err(|e| Fault::new("backend", "gip", e))?
    }
}
enum Operation {
    Send(MessageClass, u8, Vec<u8>),
    Battery,
}
impl Resource for Core {
    fn close(&self) {
        Core::close(self)
    }
    fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) && self.pending.load(Ordering::Acquire) == 0
    }
}
impl Drop for Core {
    fn drop(&mut self) {
        self.close();
    }
}
