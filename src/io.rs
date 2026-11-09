use crate::{
    bindings::{self, Buffer, Context, Resource, Shared, unhex},
    error::Fault,
    queue::{Queue, Subscription},
    scheduler,
};
use mlua::{
    AnyUserData, Function, Lua, LuaSerdeExt, MultiValue, Table, UserData, UserDataMethods,
    Value as LV,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

enum Command {
    Write(
        Vec<u8>,
        tokio::sync::oneshot::Sender<Result<usize, Fault>>,
        tokio_util::sync::CancellationToken,
    ),
    Descriptor(tokio::sync::oneshot::Sender<Result<Vec<u8>, Fault>>),
    Control {
        op: String,
        wire: Vec<u8>,
        reply: tokio::sync::oneshot::Sender<Result<Vec<u8>, Fault>>,
        cancel: tokio_util::sync::CancellationToken,
    },
}
pub struct ChannelCore {
    tx: mpsc::SyncSender<Command>,
    closed: AtomicBool,
    finished: AtomicBool,
    subscribers: Mutex<Vec<std::sync::Weak<Queue>>>,
    pub input: Arc<Queue>,
    gate: tokio::sync::Mutex<()>,
    info: Value,
    path: String,
    owner: String,
    shared: Arc<Shared>,
    generation: String,
    numbered: bool,
}
impl ChannelCore {
    async fn control(&self, op: &str, wire: Vec<u8>) -> Result<Vec<u8>, Fault> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Fault::new("closed", op, "channel closed"));
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .try_send(Command::Control {
                op: op.into(),
                wire,
                reply: tx,
                cancel: scheduler::current_cancel(),
            })
            .map_err(|e| Fault::new("busy", op, e))?;
        tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .map_err(|_| Fault::new("timeout", op, "control deadline exceeded"))?
            .map_err(|_| Fault::new("disconnected", op, "worker stopped"))?
    }
    fn trace(&self, op: &str, data: Value) {
        self.shared.emit(
            &self.owner,
            "io",
            json!({"path":self.path,"generation":self.generation,"op":op,"detail":data}),
        );
    }
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.input.close();
        for q in self
            .subscribers
            .lock()
            .unwrap()
            .iter()
            .filter_map(std::sync::Weak::upgrade)
        {
            q.close();
        }
    }
    fn fanout(&self, event: Value) {
        self.input.push(event.clone());
        let mut queues = self.subscribers.lock().unwrap();
        queues.retain(|q| q.strong_count() > 0);
        for q in queues.iter().filter_map(std::sync::Weak::upgrade) {
            q.push(event.clone());
        }
    }
    fn subscribe(&self) -> Result<Arc<Queue>, Fault> {
        let mut observers = self.subscribers.lock().unwrap();
        observers.retain(|q| q.upgrade().is_some_and(|q| !q.is_closed()));
        if observers.len() >= 32 {
            return Err(Fault::new(
                "resource_limit",
                "hid.subscribe",
                "maximum 32 subscriptions per connection",
            ));
        }
        let queue = Queue::new(64);
        observers.push(Arc::downgrade(&queue));
        Ok(queue)
    }
    async fn write(&self, wire: Vec<u8>) -> Result<usize, Fault> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Fault::new("closed", "hid.write", "channel closed"));
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .try_send(Command::Write(wire, tx, crate::scheduler::current_cancel()))
            .map_err(|e| Fault::new("busy", "hid.write", e))?;
        tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .map_err(|_| {
                Fault::new(
                    "stalled",
                    "hid.write",
                    "native write has not finished; channel retained",
                )
            })?
            .map_err(|_| Fault::new("disconnected", "hid.write", "worker stopped"))?
    }
}
#[derive(Clone)]
pub struct Channel {
    core: Arc<ChannelCore>,
    budget: Arc<crate::bindings::Budget>,
    transaction: Option<Arc<AtomicBool>>,
}
impl Channel {
    async fn lock(&self) -> mlua::Result<Option<tokio::sync::MutexGuard<'_, ()>>> {
        if let Some(active) = &self.transaction {
            if !active.load(Ordering::Acquire) {
                return Err(
                    Fault::new("closed", "hid.transaction", "transaction handle expired").lua(),
                );
            }
            Ok(None)
        } else {
            Ok(Some(self.core.gate.lock().await))
        }
    }
}
impl Resource for Channel {
    fn close(&self) {
        self.core.close();
    }
    fn closed(&self) -> bool {
        self.core.finished.load(Ordering::Acquire)
    }
}
impl Drop for Channel {
    fn drop(&mut self) { /* Context owns the connection through stop/unload. */
    }
}
fn packet(
    lua: &Lua,
    value: Value,
    budget: &Arc<crate::bindings::Budget>,
    length: usize,
) -> mlua::Result<Table> {
    let result = lua.create_table()?;
    if value["kind"] == "timeout" {
        result.set("status", "timeout")?;
        result.set("actual_length", 0)?;
        result.set("wire", Buffer::new(vec![], budget)?)?;
        return Ok(result);
    }
    if value["kind"] == "overflow" {
        return Err(Fault::new(
            "overflow",
            "hid.read",
            format!("{} input reports dropped", value["dropped"]),
        )
        .lua());
    }
    if value["kind"] == "disconnected" {
        return Err(Fault::new(
            "disconnected",
            "hid.read",
            value["message"].as_str().unwrap_or("device disconnected"),
        )
        .lua());
    }
    let bytes = unhex(value["wire_hex"].as_str().unwrap_or(""))?;
    if bytes.len() > length {
        return Err(Fault::new(
            "short_buffer",
            "hid.read",
            "input report exceeds caller's buffer; report was not truncated",
        )
        .lua());
    }
    let numbered = value["numbered"].as_bool().unwrap_or(true);
    result.set("status", "ok")?;
    result.set("actual_length", bytes.len())?;
    result.set(
        "report_id",
        if numbered {
            bytes.first().copied().unwrap_or(0)
        } else {
            0
        },
    )?;
    result.set(
        "payload",
        Buffer::new(
            if numbered {
                bytes.get(1..).unwrap_or_default()
            } else {
                &bytes
            }
            .to_vec(),
            budget,
        )?,
    )?;
    result.set("wire", Buffer::new(bytes, budget)?)?;
    Ok(result)
}
impl UserData for Channel {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("inspect",|lua,c,()|lua.to_value(&json!({"device":c.core.info,"closed":c.core.closed.load(Ordering::Acquire),"closing":!c.core.finished.load(Ordering::Acquire)&&c.core.closed.load(Ordering::Acquire),"generation":c.core.generation,"capabilities":{"read_input":true,"get_input_report":true,"write_output":"policy","get_feature":"policy","set_feature":"policy","send_output_report":"policy","transactions":true,"native_process_isolation":c.core.info["backend"]!="mock","subscriptions":true,"native_write_cancellation":false,"numbered_reports":c.core.numbered}})));
        m.add_async_method(
            "transaction",
            |lua, c, (callback, timeout): (Function, Option<u64>)| async move {
                if c.transaction.is_some() {
                    return Err(Fault::new(
                        "invalid_argument",
                        "hid.transaction",
                        "nested transactions are not supported",
                    )
                    .lua());
                }
                let timeout = timeout.unwrap_or(10000);
                if timeout == 0 || timeout > 120000 {
                    return Err(Fault::new(
                        "invalid_argument",
                        "hid.transaction",
                        "timeout 1..120000 required",
                    )
                    .lua());
                }
                tokio::time::timeout(Duration::from_millis(timeout), async {
                    let _guard = c.lock().await?;
                    let active = Arc::new(AtomicBool::new(true));
                    struct Expire(Arc<AtomicBool>);
                    impl Drop for Expire {
                        fn drop(&mut self) {
                            self.0.store(false, Ordering::Release);
                        }
                    }
                    let _expire = Expire(active.clone());
                    let tx = lua.create_userdata(Channel {
                        core: c.core.clone(),
                        budget: c.budget.clone(),
                        transaction: Some(active),
                    })?;
                    scheduler::execute(
                        &lua,
                        callback,
                        MultiValue::from_vec(vec![LV::UserData(tx)]),
                        timeout,
                        500,
                        scheduler::current_cancel(),
                    )
                    .await
                })
                .await
                .map_err(|_| {
                    Fault::new(
                        "timeout",
                        "hid.transaction",
                        "transaction deadline exceeded; completed side effects are not rolled back",
                    )
                    .lua()
                })?
            },
        );
        m.add_async_method("close", |_, c, ()| async move {
            c.core.close();
            let deadline = tokio::time::Instant::now() + Duration::from_millis(2200);
            while !c.core.finished.load(Ordering::Acquire) && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            if !c.core.finished.load(Ordering::Acquire) {
                return Err(Fault::new(
                    "stalled",
                    "hid.close",
                    "native worker still closing; lease retained",
                )
                .lua());
            }
            Ok(())
        });
        m.add_async_method("descriptor", |_, c, ()| async move {
            let _guard = c.lock().await?;
            if c.core.closed.load(Ordering::Acquire) {
                return Err(Fault::new("closed", "hid.descriptor", "channel closed").lua());
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            c.core
                .tx
                .try_send(Command::Descriptor(tx))
                .map_err(|e| Fault::new("busy", "hid.descriptor", e).lua())?;
            let bytes = tokio::time::timeout(Duration::from_secs(2), rx)
                .await
                .map_err(|_| Fault::new("timeout", "hid.descriptor", "deadline exceeded").lua())?
                .map_err(|_| Fault::new("disconnected", "hid.descriptor", "worker ended").lua())?
                .map_err(Fault::lua)?;
            Buffer::new(bytes, &c.budget)
        });
        m.add_async_method(
            "write_output",
            |_, c, (id, payload): (u8, AnyUserData)| async move {
                let payload = payload.borrow::<Buffer>()?.bytes().to_vec();
                if payload.len() > 1023 {
                    return Err(Fault::new(
                        "invalid_argument",
                        "hid.write",
                        "report exceeds 1024 bytes",
                    )
                    .lua());
                }
                let _guard = c.lock().await?;
                let mut wire = vec![id];
                wire.extend_from_slice(&payload);
                c.core.write(wire).await.map_err(Fault::lua)
            },
        );
        for name in ["get_input_report", "get_feature"] {
            m.add_async_method(
                name,
                move |lua, c, (id, length, seed): (u8, usize, Option<AnyUserData>)| async move {
                    if !(1..=1024).contains(&length) || (!c.core.numbered && id != 0) {
                        return Err(Fault::new(
                            "invalid_argument",
                            name,
                            "length 1..1024 required; unnumbered reports require ID 0",
                        )
                        .lua());
                    }
                    let mut wire = vec![0; length];
                    wire[0] = id;
                    if let Some(seed) = seed {
                        let seed = seed.borrow::<Buffer>()?;
                        if seed.bytes().len() != length - 1 {
                            return Err(Fault::new(
                                "invalid_argument",
                                name,
                                "seed must contain exactly length-1 payload bytes",
                            )
                            .lua());
                        }
                        wire[1..].copy_from_slice(seed.bytes());
                    }
                    let _guard = c.lock().await?;
                    let mut bytes = c.core.control(name, wire).await.map_err(Fault::lua)?;
                    if !c.core.numbered {
                        bytes.remove(0);
                    }
                    packet(
                        &lua,
                        json!({"wire_hex":bindings::hex(&bytes),"numbered":c.core.numbered}),
                        &c.budget,
                        length,
                    )
                },
            );
        }
        for name in ["set_feature", "send_output_report"] {
            m.add_async_method(
                name,
                move |_, c, (id, payload): (u8, AnyUserData)| async move {
                    let mut wire = vec![id];
                    wire.extend_from_slice(payload.borrow::<Buffer>()?.bytes());
                    if wire.len() > 1024 || (!c.core.numbered && id != 0) {
                        return Err(Fault::new(
                            "invalid_argument",
                            name,
                            "invalid report ID or length",
                        )
                        .lua());
                    }
                    let _guard = c.lock().await?;
                    Ok(c.core.control(name, wire).await.map_err(Fault::lua)?.len())
                },
            );
        }
        m.add_async_method(
            "read_input",
            |lua, c, (length, timeout): (usize, u64)| async move {
                if !(1..=1024).contains(&length) || timeout > 120000 {
                    return Err(Fault::new(
                        "invalid_argument",
                        "hid.read",
                        "length 1..1024 and timeout 0..120000 ms required",
                    )
                    .lua());
                }
                let _guard = c.lock().await?;
                packet(
                    &lua,
                    c.core.input.next(timeout).await.map_err(Fault::lua)?,
                    &c.budget,
                    length,
                )
            },
        );
        m.add_method("subscribe", |_, c, ()| {
            if c.core.closed.load(Ordering::Acquire) {
                return Err(Fault::new("closed", "hid.subscribe", "channel closed").lua());
            }
            Ok(Subscription(c.core.subscribe().map_err(Fault::lua)?))
        });
        m.add_async_method("request",|lua,c,(id,payload,matcher,timeout):(u8,AnyUserData,Function,Option<u64>)|async move{
            let data=payload.borrow::<Buffer>()?.bytes().to_vec();if data.len()>1023{return Err(Fault::new("invalid_argument","hid.request","report exceeds 1024 bytes").lua());}
            let wait=timeout.unwrap_or(1000).clamp(1,120000);let deadline=Instant::now()+Duration::from_millis(wait);
            tokio::time::timeout(Duration::from_millis(wait), async {
            let _guard=c.lock().await?;
            let queue=c.core.subscribe().map_err(Fault::lua)?; // Matcher receiver exists before transmission.
            struct Cleanup(Arc<Queue>);impl Drop for Cleanup{fn drop(&mut self){self.0.close();}}
            let _cleanup=Cleanup(queue.clone());
            let mut wire=vec![id];wire.extend_from_slice(&data);c.core.write(wire).await.map_err(Fault::lua)?;
            loop{
                let remaining=deadline.saturating_duration_since(Instant::now()).as_millis() as u64;
                if remaining==0{return Err(Fault::new("timeout","hid.request","no matching response; no automatic retry").lua());}
                let value=queue.next(remaining).await.map_err(Fault::lua)?;
                if value["kind"]=="timeout"{return Err(Fault::new("timeout","hid.request","no matching response").lua());}
                let reply=packet(&lua,value,&c.budget,1024)?;
                let values=scheduler::execute(&lua,matcher.clone(),MultiValue::from_vec(vec![LV::Table(reply.clone())]),remaining.min(500),100,tokio_util::sync::CancellationToken::new()).await?;
                if values.front().is_some_and(|v|!matches!(v,LV::Nil|LV::Boolean(false))){return Ok(reply);}
            }
            }).await.map_err(|_|Fault::new("timeout","hid.request","transaction deadline exceeded; native completion may still be pending").lua())?
        });
    }
}
struct Step {
    op: String,
    error: Option<String>,
    request: Vec<u8>,
    response: Option<Vec<u8>>,
    delay: Duration,
    disconnect: bool,
}
struct Lease {
    shared: Arc<Shared>,
    path: String,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.shared.leases.lock().unwrap().remove(&self.path);
    }
}
enum Backend {
    Hid {
        handle: crate::native_hid::NativeHid,
        _lease: Lease,
    },
    Mock {
        steps: VecDeque<Step>,
        pending: Vec<(Instant, Option<Vec<u8>>)>,
        _allocation: crate::bindings::Charge,
    },
}
fn worker(core: Arc<ChannelCore>, mut backend: Backend, rx: mpsc::Receiver<Command>) {
    let mut last_write = None::<Instant>;
    while !core.closed.load(Ordering::Acquire) {
        while let Ok(command) = rx.try_recv() {
            match command {
                Command::Control {
                    op,
                    wire,
                    reply,
                    cancel,
                } => {
                    if reply.is_closed() || cancel.is_cancelled() {
                        continue;
                    }
                    let read = op == "get_input_report" || op == "get_feature";
                    let result = match &mut backend {
                        Backend::Hid { handle, .. } => {
                            if op != "get_input_report"
                                && !core.shared.policy.allows_report(&core.info, &op, &wire)
                            {
                                Err(Fault::new(
                                    "permission_denied",
                                    &op,
                                    "no exact device/operation/report policy matches",
                                ))
                            } else if op != "get_input_report"
                                && last_write
                                    .is_some_and(|t| t.elapsed() < Duration::from_millis(100))
                            {
                                Err(Fault::new(
                                    "rate_limit",
                                    &op,
                                    "minimum 100ms between report commands",
                                ))
                            } else {
                                if op != "get_input_report" {
                                    last_write = Some(Instant::now());
                                }
                                if read {
                                    let mut bytes = wire.clone();
                                    let result = if op == "get_feature" {
                                        handle.get_feature_report(&mut bytes)
                                    } else {
                                        handle.get_input_report(&mut bytes)
                                    };
                                    result.map(|n| {
                                        bytes.truncate(n);
                                        bytes
                                    })
                                } else {
                                    handle.report(&op, &wire).and_then(|n| {
                                        if n == wire.len() {
                                            Ok(wire.clone())
                                        } else {
                                            Err(Fault::new("short_write", &op, "incomplete report"))
                                        }
                                    })
                                }
                            }
                        }
                        Backend::Mock { steps, .. } => {
                            if op.contains("feature") && steps.front().is_none_or(|s| s.op != op) {
                                Err(Fault::new(
                                    "permission_denied",
                                    &op,
                                    "Feature operation requires an explicit replay step",
                                ))
                            } else {
                                (|| {
                                    let step = steps.front().ok_or_else(|| {
                                        Fault::new("replay_exhausted", &op, "replay exhausted")
                                    })?;
                                    if step.op != op || step.request != wire {
                                        return Err(Fault::new(
                                            "replay_mismatch",
                                            &op,
                                            "operation or request mismatch",
                                        ));
                                    }
                                    let step = steps.pop_front().unwrap();
                                    let deadline = Instant::now() + step.delay;
                                    while Instant::now() < deadline {
                                        if cancel.is_cancelled()
                                            || reply.is_closed()
                                            || core.closed.load(Ordering::Acquire)
                                        {
                                            return Err(Fault::new(
                                                "cancelled",
                                                &op,
                                                "replay cancelled",
                                            ));
                                        }
                                        std::thread::sleep(Duration::from_millis(1));
                                    }
                                    if step.disconnect {
                                        core.close();
                                        return Err(Fault::new(
                                            "disconnected",
                                            &op,
                                            "replay disconnected",
                                        ));
                                    }
                                    if let Some(code) = step.error {
                                        return Err(Fault::new(
                                            &code,
                                            &op,
                                            "injected replay error",
                                        ));
                                    }
                                    Ok(if read {
                                        step.response.unwrap_or_default()
                                    } else {
                                        wire.clone()
                                    })
                                })()
                            }
                        }
                    }
                    .and_then(|bytes| {
                        if read
                            && (bytes.is_empty() || bytes.len() > wire.len() || bytes[0] != wire[0])
                        {
                            return Err(Fault::new(
                                "invalid_frame",
                                &op,
                                "invalid report length or ID",
                            ));
                        }
                        Ok(bytes)
                    });
                    core.trace(&op,json!({"request_hex":bindings::hex(&wire),"wire_hex":result.as_ref().ok().map(|v|bindings::hex(v)),"error":result.as_ref().err()}));
                    if result.as_ref().is_err_and(|e| e.code == "disconnected") {
                        core.close();
                    }
                    let _ = reply.send(result);
                }
                Command::Descriptor(reply) => {
                    let result = match &backend {
                        Backend::Hid { handle: h, .. } => {
                            let mut bytes = vec![0; 4096];
                            h.get_report_descriptor(&mut bytes)
                                .map(|n| {
                                    bytes.truncate(n);
                                    bytes
                                })
                                .map_err(|e| Fault::new("backend", "hid.descriptor", e))
                        }
                        _ => Err(Fault::new(
                            "unsupported",
                            "hid.descriptor",
                            "mock has no HID descriptor",
                        )),
                    };
                    if let Ok(bytes) = &result {
                        core.trace(
                            "descriptor",
                            json!({"hex":bindings::hex(bytes),"actual_length":bytes.len()}),
                        );
                    }
                    let _ = reply.send(result);
                }
                Command::Write(wire, reply, cancel) => {
                    if reply.is_closed() || cancel.is_cancelled() {
                        continue;
                    }
                    let result = match &mut backend {
                        Backend::Mock { steps, pending, .. } => match steps.front() {
                            None => Err(Fault::new(
                                "replay_exhausted",
                                "mock.write",
                                "replay exhausted",
                            )),
                            Some(step) if step.op != "write_output" || step.request != wire => {
                                Err(Fault::new(
                                    "replay_mismatch",
                                    "mock.write",
                                    "replay request mismatch",
                                ))
                            }
                            Some(step) if step.error.is_some() => {
                                let step = steps.pop_front().unwrap();
                                Err(Fault::new(
                                    &step.error.unwrap(),
                                    "mock.write",
                                    "injected replay error",
                                ))
                            }
                            Some(_) => {
                                let step = steps.pop_front().unwrap();
                                if step.disconnect || step.response.is_some() {
                                    pending.push((
                                        Instant::now() + step.delay,
                                        if step.disconnect { None } else { step.response },
                                    ));
                                }
                                Ok(wire.len())
                            }
                        },
                        Backend::Hid { handle: h, .. } => {
                            if !core.shared.policy.allows(&core.info, &wire) {
                                Err(Fault::new(
                                    "permission_denied",
                                    "hid.write",
                                    "policy denied: output report is not an approved query",
                                ))
                            } else if last_write
                                .is_some_and(|t| t.elapsed() < Duration::from_millis(100))
                            {
                                Err(Fault::new(
                                    "rate_limit",
                                    "hid.write",
                                    "minimum 100ms between reports",
                                ))
                            } else {
                                last_write = Some(Instant::now());
                                h.write(&wire)
                                    .map_err(|e| Fault::new("backend", "hid.write", e))
                                    .and_then(|n| {
                                        if n == wire.len() {
                                            Ok(n)
                                        } else {
                                            Err(Fault::new(
                                                "short_write",
                                                "hid.write",
                                                format!("{n}/{}", wire.len()),
                                            ))
                                        }
                                    })
                            }
                        }
                    };
                    let op = if result
                        .as_ref()
                        .err()
                        .is_some_and(|e| e.code == "permission_denied")
                    {
                        "denied"
                    } else {
                        "write_output"
                    };
                    core.trace(op,json!({"wire_hex":bindings::hex(&wire),"report_id":wire[0],"actual_length":result.as_ref().ok(),"error":result.as_ref().err()}));
                    let _ = reply.send(result);
                }
            }
        }
        let incoming = match &mut backend {
            Backend::Hid { handle: h, .. } => {
                let mut bytes = vec![0; 1024];
                match h.read_timeout(&mut bytes, 10) {
                    Ok(0) => None,
                    Ok(n) => {
                        bytes.truncate(n);
                        Some(Ok(bytes))
                    }
                    Err(e) => Some(Err(e.to_string())),
                }
            }
            Backend::Mock { pending, .. } => {
                if let Some(i) = pending
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.0 <= Instant::now())
                    .min_by_key(|(_, p)| p.0)
                    .map(|(i, _)| i)
                {
                    Some(
                        pending
                            .remove(i)
                            .1
                            .ok_or_else(|| "replay device disconnected".into()),
                    )
                } else {
                    std::thread::sleep(Duration::from_millis(1));
                    None
                }
            }
        };
        if let Some(result) = incoming {
            match result {
                Ok(bytes) => {
                    let event = json!({"kind":"input","wire_hex":bindings::hex(&bytes),"actual_length":bytes.len(),"numbered":core.numbered,"generation":core.generation});
                    core.trace("read_input",json!({"wire_hex":bindings::hex(&bytes),"actual_length":bytes.len(),"status":"ok"}));
                    core.fanout(event);
                }
                Err(message) => {
                    core.fanout(json!({"kind":"disconnected","message":message}));
                    core.trace("disconnected", json!({"message":message}));
                    core.close();
                }
            }
        }
    }
    drop(backend);
    core.finished.store(true, Ordering::Release);
    core.close();
    core.trace("closed", json!({}));
}
fn open_channel(
    ctx: &Context,
    path: String,
    info: Value,
    backend: Backend,
    numbered: bool,
) -> mlua::Result<Channel> {
    let (tx, rx) = mpsc::sync_channel(16);
    let core = Arc::new(ChannelCore {
        tx,
        closed: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        subscribers: Mutex::new(vec![]),
        input: Queue::new(128),
        gate: tokio::sync::Mutex::new(()),
        info,
        path: path.clone(),
        owner: ctx.name.clone(),
        shared: ctx.shared.clone(),
        generation: uuid::Uuid::new_v4().to_string(),
        numbered,
    });
    let channel = Channel {
        core: core.clone(),
        budget: ctx.budget.clone(),
        transaction: None,
    };
    if let Err(e) = ctx.manage(Rc::new(channel.clone())) {
        channel.core.finished.store(true, Ordering::Release);
        return Err(e);
    }
    std::thread::Builder::new()
        .name("hid-io".into())
        .spawn(move || worker(core, backend, rx))
        .map_err(|e| {
            channel.core.close();
            channel.core.finished.store(true, Ordering::Release);
            mlua::Error::external(e)
        })?;
    channel.core.trace("open", json!({}));
    Ok(channel)
}
struct HidWatch(Arc<Queue>);
impl Resource for HidWatch {
    fn close(&self) {
        self.0.close();
    }
    fn closed(&self) -> bool {
        self.0.is_closed()
    }
}
fn resolve(reference: &Value, devices: &[Value]) -> Result<Value, Fault> {
    let candidates: Vec<_> = devices
        .iter()
        .filter(|d| {
            [
                "vendor_id",
                "product_id",
                "interface",
                "usage_page",
                "usage",
            ]
            .iter()
            .all(|k| reference.get(k).is_some_and(|v| d[*k] == *v))
        })
        .collect();
    if let Some(exact) = candidates.iter().find(|d| {
        d["path"] == reference["path"]
            && reference["container_id"]
                .as_str()
                .zip(d["container_id"].as_str())
                .is_none_or(|(old, new)| old == new)
    }) {
        return Ok((*exact).clone());
    }
    if let Some(container) = reference["container_id"].as_str() {
        let found: Vec<_> = candidates
            .iter()
            .filter(|d| d["container_id"] == container)
            .collect();
        match found.as_slice() {
            [d] => return Ok((**d).clone()),
            [] => {}
            _ => {
                return Err(Fault::new(
                    "ambiguous",
                    "device.resolve",
                    "multiple collections match OS container identity",
                ));
            }
        }
    }
    let serial = reference["serial"].as_str().unwrap_or("");
    if serial.is_empty() || serial.chars().all(|c| c == '0' || c.is_whitespace()) {
        return Err(Fault::new(
            "identity_required",
            "device.resolve",
            "path changed and no trustworthy serial; select a new reference explicitly",
        ));
    }
    let found: Vec<_> = candidates
        .into_iter()
        .filter(|d| d["serial"] == serial)
        .collect();
    match found.as_slice() {
        [d] => Ok((*d).clone()),
        [] => Err(Fault::new("not_found", "device.resolve", "device absent")),
        _ => Err(Fault::new(
            "ambiguous",
            "device.resolve",
            "multiple matching serials; select exact path",
        )),
    }
}
pub fn install(lua: &Lua, ctx: &Context) -> mlua::Result<()> {
    // Materialize async userdata methods while mlua's coroutine helper is available.
    let (tx, _) = mpsc::sync_channel(1);
    lua.create_userdata(Channel {
        core: Arc::new(ChannelCore {
            tx,
            closed: AtomicBool::new(true),
            finished: AtomicBool::new(true),
            subscribers: Mutex::new(vec![]),
            input: Queue::new(1),
            gate: tokio::sync::Mutex::new(()),
            info: json!({}),
            path: String::new(),
            owner: ctx.name.clone(),
            shared: ctx.shared.clone(),
            generation: String::new(),
            numbered: true,
        }),
        budget: ctx.budget.clone(),
        transaction: None,
    })?;
    lua.create_userdata(Subscription(Queue::new(1)))?;

    let device = lua.create_table()?;
    device.set(
        "list",
        lua.create_async_function(|lua, filter: Option<Table>| async move {
            let filter = filter
                .map(|t| bindings::to_json(LV::Table(t), 0))
                .transpose()?;
            if filter.as_ref().is_some_and(|f| f["backend"] == "ble") {
                return lua.to_value(
                    &crate::bluetooth::list(filter.unwrap())
                        .await
                        .map_err(Fault::lua)?,
                );
            }
            let result = tokio::task::spawn_blocking(move || bindings::enumerate(filter.as_ref()))
                .await
                .map_err(mlua::Error::external)?
                .map_err(mlua::Error::external)?;
            lua.to_value(&result)
        })?,
    )?;
    device.set(
        "resolve",
        lua.create_async_function(|lua, reference: Table| async move {
            let reference = bindings::to_json(LV::Table(reference), 0)?;
            let result = tokio::task::spawn_blocking(move || {
                let filter=json!({"vendor_id":reference["vendor_id"],"product_id":reference["product_id"]});
                let devices = bindings::enumerate(Some(&filter))
                    .map_err(|e| Fault::new("backend", "device.resolve", e))?;
                resolve(&reference, devices.as_array().unwrap())
            })
            .await
            .map_err(mlua::Error::external)?
            .map_err(Fault::lua)?;
            lua.to_value(&result)
        })?,
    )?;
    let watch_context = ctx.clone();
    device.set("watch",lua.create_function(move|_,opts:Option<Table>|{
        let interval=opts.as_ref().map(|o|o.get::<Option<u64>>("interval_ms")).transpose()?.flatten().unwrap_or(1000);
        if !(500..=60000).contains(&interval){return Err(Fault::new("invalid_argument","device.watch","interval must be 500..60000ms").lua());}
        let filter=opts.as_ref().map(|o|o.get::<Option<Table>>("filter")).transpose()?.flatten().map(|t|bindings::to_json(LV::Table(t),0)).transpose()?;
        let q=Queue::new(64);watch_context.manage(Rc::new(HidWatch(q.clone())))?;
        let queue=q.clone();crate::bluetooth::executor().spawn(async move{
            let mut previous=std::collections::BTreeMap::new();let mut first=true;
            loop {
                if queue.is_closed(){break;}
                let f=filter.clone();
                match tokio::task::spawn_blocking(move||bindings::enumerate(f.as_ref())).await {
                    Ok(Ok(devices))=>{
                        let now:std::collections::BTreeMap<_,_>=devices.as_array().unwrap().iter().map(|d|(d["path"].as_str().unwrap().to_owned(),d.clone())).collect();
                        for(path,d)in &now {if !previous.contains_key(path){queue.push(json!({"kind":if first{"present"}else{"added"},"device":d}));}}
                        for(path,d)in &previous {if !now.contains_key(path){queue.push(json!({"kind":"removed","device":d}));}}
                        if first {queue.push(json!({"kind":"ready","interval_ms":interval}));first=false;}
                        previous=now;
                    },
                    error=>queue.push(json!({"kind":"error","message":format!("{error:?}")})),
                }
                tokio::select!{_=queue.cancellation.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(interval))=>{}}
            }
            queue.close();
        });Ok(Subscription(q))
    })?)?;
    device.set(
        "parse_descriptor",
        lua.create_function(|lua, data: AnyUserData| {
            lua.to_value(
                &crate::hid_descriptor::parse(data.borrow::<Buffer>()?.bytes())
                    .map_err(Fault::lua)?,
            )
        })?,
    )?;
    let c = ctx.clone();
    device.set(
        "open",
        lua.create_async_function(move |_, (reference, opts): (Table, Option<Table>)| {
            let c = c.clone();
            async move {
                let path = reference.get::<String>("path")?;
                let numbered = opts
                    .as_ref()
                    .map(|o| o.get::<Option<bool>>("numbered_reports"))
                    .transpose()?
                    .flatten()
                    .unwrap_or(true);
                let shared = c.shared.clone();
                let owner = c.name.clone();
                let p = path.clone();
                let cancelled = crate::scheduler::current_cancel();
                let (handle, info, lease) =
                    tokio::task::spawn_blocking(move || -> Result<_, Fault> {
                        let mut leases = shared.leases.lock().unwrap();
                        if let Some(owner) = leases.get(&p) {
                            return Err(Fault::new(
                                "busy",
                                "device.open",
                                format!("channel leased by instance {owner}"),
                            ));
                        }
                        leases.insert(p.clone(), owner);
                        drop(leases);
                        let lease = Lease {
                            shared: shared.clone(),
                            path: p.clone(),
                        };
                        let list = bindings::enumerate(Some(&json!({"path":p})))
                            .map_err(|e| Fault::new("backend", "device.list", e))?;
                        let info = list
                            .as_array()
                            .and_then(|a| a.first())
                            .cloned()
                            .ok_or_else(|| {
                                Fault::new("not_found", "device.open", "device not present")
                            })?;
                        let handle = crate::native_hid::NativeHid::open(&p)?;
                        if cancelled.is_cancelled() {
                            return Err(Fault::new("cancelled", "device.open", "open cancelled"));
                        }
                        Ok((handle, info, lease))
                    })
                    .await
                    .map_err(mlua::Error::external)?
                    .map_err(Fault::lua)?;
                open_channel(
                    &c,
                    path,
                    info,
                    Backend::Hid {
                        handle,
                        _lease: lease,
                    },
                    numbered,
                )
            }
        })?,
    )?;
    lua.globals().set("device", device)?;
    lua.load(
        r#"
        function device.reconnect(channel, reference)
            local old=channel:inspect().device
            channel:close()
            return device.open(reference or device.resolve(old))
        end
    "#,
    )
    .exec()?;
    let mock = lua.create_table()?;
    let c = ctx.clone();
    mock.set(
        "replay",
        lua.create_function(move |_, steps: Table| {
            let mut queue = VecDeque::new();
            let mut size = 0;
            for step in steps.sequence_values::<Table>() {
                if queue.len() >= 64 {
                    return Err(
                        Fault::new("resource_limit", "mock.replay", "64 steps maximum").lua(),
                    );
                }
                let step = step?;
                let request = unhex(&step.get::<String>("request")?)?;
                let response = step
                    .get::<Option<String>>("response")?
                    .map(|s| unhex(&s))
                    .transpose()?;
                if request.is_empty()
                    || request.len() > 1024
                    || response.as_ref().is_some_and(|b| b.len() > 1024)
                {
                    return Err(Fault::new(
                        "invalid_argument",
                        "mock.replay",
                        "report length 1..1024",
                    )
                    .lua());
                }
                size += request.len() + response.as_ref().map_or(0, Vec::len);
                let op = step
                    .get::<Option<String>>("op")?
                    .unwrap_or_else(|| "write_output".into());
                if ![
                    "write_output",
                    "get_input_report",
                    "get_feature",
                    "set_feature",
                    "send_output_report",
                ]
                .contains(&op.as_str())
                {
                    return Err(
                        Fault::new("invalid_argument", "mock.replay", "unknown operation").lua(),
                    );
                }
                queue.push_back(Step {
                    op,
                    error: step.get::<Option<String>>("error")?,
                    request,
                    response,
                    delay: Duration::from_millis(
                        step.get::<Option<u64>>("delay_ms")?
                            .unwrap_or(0)
                            .min(120000),
                    ),
                    disconnect: step.get::<Option<bool>>("disconnect")?.unwrap_or(false),
                });
            }
            if c.budget.get() + size > 1024 * 1024 {
                return Err(Fault::new(
                    "resource_limit",
                    "mock.replay",
                    "native Buffer budget exceeded",
                )
                .lua());
            }
            open_channel(
                &c,
                format!("mock:{}", uuid::Uuid::new_v4()),
                json!({"backend":"mock"}),
                Backend::Mock {
                    steps: queue,
                    pending: vec![],
                    _allocation: crate::bindings::Charge::new(c.budget.clone(), size)?,
                },
                true,
            )
        })?,
    )?;
    lua.globals().set("mock", mock)?;
    Ok(())
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn reconnect_requires_unambiguous_identity() {
        let original = json!({"vendor_id":1,"product_id":2,"interface":1,"usage_page":65299,"usage":1,"path":"old","serial":"000000","container_id":"container-one"});
        let mut moved = original.clone();
        moved["container_id"] = json!("different-device-same-port");
        assert_eq!(
            resolve(&original, &[moved.clone()]).unwrap_err().code,
            "identity_required"
        );
        moved["container_id"] = original["container_id"].clone();
        moved["path"] = json!("new");
        assert_eq!(resolve(&original, &[moved.clone()]).unwrap()["path"], "new");
        let mut duplicate = moved.clone();
        duplicate["path"] = json!("another");
        assert_eq!(
            resolve(&original, &[moved.clone(), duplicate])
                .unwrap_err()
                .code,
            "ambiguous"
        );
        moved["container_id"] = Value::Null;
        assert_eq!(
            resolve(&original, &[moved.clone()]).unwrap_err().code,
            "identity_required"
        );
        let mut serial = original.clone();
        serial["serial"] = json!("real-serial");
        moved["serial"] = json!("real-serial");
        assert_eq!(resolve(&serial, &[moved]).unwrap()["path"], "new");
    }
}
