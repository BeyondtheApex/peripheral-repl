//! BLE GATT transport. No pairing, radio power changes or guessed vendor writes.
use crate::{
    bindings::{self, Buffer, Context, Resource, Shared},
    error::Fault,
    queue::{Queue, Subscription},
};
use btleplug::{
    api::{
        Central, CentralEvent, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
    },
    platform::{Adapter, Manager, Peripheral},
};
use futures::StreamExt;
use mlua::{AnyUserData, Lua, LuaSerdeExt, Table, UserData, UserDataMethods, Value as LV};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

fn io_error(op: &str, e: btleplug::Error) -> Fault {
    let text = e.to_string();
    let code = match &e {
        btleplug::Error::PermissionDenied => "access_denied",
        btleplug::Error::DeviceNotFound => "not_found",
        btleplug::Error::NotConnected => "disconnected",
        btleplug::Error::NotSupported(_) => "unsupported",
        btleplug::Error::TimedOut(_) => "timeout",
        _ => "backend",
    };
    Fault::new(code, op, text)
}
pub fn executor() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(8)
            .enable_all()
            .thread_name("ble-io")
            .build()
            .unwrap()
    })
}
struct Hub {
    adapters: tokio::sync::Mutex<Option<Vec<Adapter>>>,
    devices: Mutex<BTreeMap<String, (usize, Peripheral, Value)>>,
    scan_gate: tokio::sync::Mutex<()>,
    operations: Arc<tokio::sync::Semaphore>,
}
fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        adapters: tokio::sync::Mutex::new(None),
        devices: Mutex::new(BTreeMap::new()),
        scan_gate: tokio::sync::Mutex::new(()),
        operations: Arc::new(tokio::sync::Semaphore::new(16)),
    })
}
async fn adapters() -> Result<Vec<Adapter>, Fault> {
    let mut a = hub().adapters.lock().await;
    if a.is_none() {
        let manager = Manager::new()
            .await
            .map_err(|e| io_error("ble.manager", e))?;
        *a = Some(
            manager
                .adapters()
                .await
                .map_err(|e| io_error("ble.adapters", e))?,
        );
    }
    Ok(a.as_ref().unwrap().clone())
}
fn uuid(s: &str) -> Result<uuid::Uuid, Fault> {
    let s = s.trim();
    let expanded = if s.len() == 4 {
        format!("0000{s}-0000-1000-8000-00805f9b34fb")
    } else if s.len() == 8 {
        format!("{s}-0000-1000-8000-00805f9b34fb")
    } else {
        s.into()
    };
    uuid::Uuid::parse_str(&expanded).map_err(|e| Fault::new("invalid_argument", "ble.uuid", e))
}
async fn discovery<T: Send + 'static>(
    op: &'static str,
    timeout: u64,
    work: impl std::future::Future<Output = Result<T, Fault>> + Send + 'static,
) -> Result<T, Fault> {
    let permit = hub()
        .operations
        .clone()
        .try_acquire_owned()
        .map_err(|_| Fault::new("busy", op, "Bluetooth operation limit reached"))?;
    let handle = executor().spawn(async move {
        let _permit = permit;
        work.await
    });
    tokio::time::timeout(Duration::from_millis(timeout), handle)
        .await
        .map_err(|_| {
            Fault::new(
                "timeout",
                op,
                "caller deadline; backend slot retained until completion",
            )
        })?
        .map_err(|e| Fault::new("backend", op, e))?
}
pub async fn list(filter: Value) -> Result<Value, Fault> {
    let filter = filter.clone();
    discovery("ble.list", 10000, async move{
        let adapters=adapters().await?;
        for(index,adapter)in adapters.iter().enumerate(){
            for peripheral in adapter.peripherals().await.map_err(|e|io_error("ble.list",e))?.into_iter().take(256){
                let properties=peripheral.properties().await.map_err(|e|io_error("ble.properties",e))?;
                let path=format!("ble:{index}:{}",peripheral.id());
                let value=if let Some(p)=properties{json!({"backend":"ble","path":path,"adapter":index,"id":peripheral.id().to_string(),"address":p.address.to_string(),"address_type":format!("{:?}",p.address_type),"name":p.local_name,"rssi":p.rssi,"services":p.services.iter().map(ToString::to_string).collect::<Vec<_>>(),"identity":"OS peripheral identity; random BLE addresses may rotate"})}else{json!({"backend":"ble","path":path,"adapter":index,"id":peripheral.id().to_string()})};
                let mut devices=hub().devices.lock().unwrap();if devices.len()<256||devices.contains_key(&path){devices.insert(path,(index,peripheral,value));}
            }
        }
        let result=hub().devices.lock().unwrap().values().map(|(_,_,v)|v.clone()).filter(|v|["path","name","address","adapter"].iter().all(|k|filter.get(k).is_none_or(|wanted|v[*k]==*wanted))).collect::<Vec<_>>();Ok(json!(result))
    }).await
}
pub fn list_sync(filter: Value) -> anyhow::Result<Value> {
    executor().block_on(list(filter)).map_err(Into::into)
}
pub fn command_sync(command: Value) -> anyhow::Result<Value> {
    let action = command["action"].as_str().unwrap_or("adapters");
    match action {
        "adapters" => executor().block_on(adapter_info()).map_err(Into::into),
        "list" => list_sync(command.get("filter").cloned().unwrap_or(json!({}))),
        "scan" => executor()
            .block_on(scan(
                command["duration_ms"].as_u64().unwrap_or(5000),
                command["adapter"].as_u64().unwrap_or(0) as usize,
                vec![],
            ))
            .map_err(Into::into),
        _ => Err(Fault::new(
            "invalid_argument",
            "bluetooth",
            "action must be adapters/list/scan",
        )
        .into()),
    }
}
async fn adapter_info() -> Result<Value, Fault> {
    discovery("ble.adapters", 10000, async{
    let mut result=vec![];for(index,a)in adapters().await?.iter().enumerate(){result.push(json!({"index":index,"info":a.adapter_info().await.map_err(|e|io_error("ble.adapter",e))?,"state":format!("{:?}",a.adapter_state().await.map_err(|e|io_error("ble.adapter",e))?)}));}Ok(json!(result))
}).await
}
async fn scan(duration: u64, index: usize, services: Vec<uuid::Uuid>) -> Result<Value, Fault> {
    if !(100..=30000).contains(&duration) {
        return Err(Fault::new(
            "invalid_argument",
            "ble.scan",
            "scan duration must be 100..30000 ms",
        ));
    }
    // Detached bounded scan owns cleanup even if the Lua caller is cancelled.
    discovery("ble.scan", duration + 20000, async move {
        let _guard = hub()
            .scan_gate
            .try_lock()
            .map_err(|_| Fault::new("busy", "ble.scan", "another scan is active"))?;
        let adapter =
            adapters().await?.get(index).cloned().ok_or_else(|| {
                Fault::new("not_found", "ble.scan", "Bluetooth adapter not found")
            })?;
        adapter
            .start_scan(ScanFilter { services })
            .await
            .map_err(|e| io_error("ble.scan", e))?;
        tokio::time::sleep(Duration::from_millis(duration)).await;
        let stop = adapter
            .stop_scan()
            .await
            .map_err(|e| io_error("ble.stop_scan", e));
        stop?;
        list(json!({"adapter":index})).await
    })
    .await
}

#[derive(Clone, Deserialize, Serialize)]
pub struct PairRule {
    pub path: String,
    pub operation: String,
    #[serde(default)]
    pub protection: Option<String>,
}
pub fn validate_pair_rules(rules: &[PairRule]) -> anyhow::Result<()> {
    anyhow::ensure!(rules.len() <= 32, "maximum 32 pairing rules");
    for r in rules {
        anyhow::ensure!(
            !r.path.is_empty() && r.path.len() <= 1024,
            "exact pairing path required"
        );
        anyhow::ensure!(
            (r.operation == "pair"
                && r.protection.as_deref().is_some_and(|s| [
                    "none",
                    "encryption",
                    "encryption_and_authentication"
                ]
                .contains(&s)))
                || (r.operation == "unpair" && r.protection.is_none()),
            "pair requires protection; unpair requires no protection"
        );
    }
    Ok(())
}
#[derive(Clone, Deserialize, Serialize)]
pub struct WriteRule {
    pub path: String,
    pub service: String,
    pub characteristic: String,
    #[serde(default)]
    pub descriptor: Option<String>,
    pub mode: String,
    #[serde(default)]
    pub service_handle: Option<u16>,
    #[serde(default)]
    pub characteristic_handle: Option<u16>,
    #[serde(default)]
    pub descriptor_handle: Option<u16>,
    #[serde(default)]
    pub packets: Vec<String>,
}
pub fn validate_rules(rules: &[WriteRule]) -> anyhow::Result<()> {
    anyhow::ensure!(rules.len() <= 32, "maximum 32 BLE write rules");
    for r in rules {
        anyhow::ensure!(
            !r.path.is_empty() && r.path.len() <= 1024,
            "exact BLE path required"
        );
        uuid(&r.service)?;
        uuid(&r.characteristic)?;
        if let Some(d) = &r.descriptor {
            uuid(d)?;
            anyhow::ensure!(
                r.mode == "descriptor" && uuid(d)? != uuid("2902")?,
                "descriptor mode required; CCCD uses subscribe"
            );
        } else {
            anyhow::ensure!(
                ["with_response", "without_response", "reliable"].contains(&r.mode.as_str()),
                "invalid BLE write mode"
            );
        }
        anyhow::ensure!(r.packets.len() <= 256, "maximum 256 packets");
        for p in &r.packets {
            let bytes = bindings::unhex(p)?;
            anyhow::ensure!(
                bytes.len() <= 512 && bindings::hex(&bytes) == *p,
                "BLE packets require canonical hex, at most 512 bytes"
            );
        }
    }
    Ok(())
}
fn allowed(
    policy: &bindings::Policy,
    path: &str,
    service: &str,
    characteristic: &str,
    descriptor: Option<&str>,
    mode: &str,
    bytes: &[u8],
) -> bool {
    policy.ble_write_rules.iter().any(|r| {
        r.path == path
            && uuid(&r.service).ok() == uuid(service).ok()
            && uuid(&r.characteristic).ok() == uuid(characteristic).ok()
            && r.descriptor.as_deref().map(uuid).transpose().ok()
                == descriptor.map(uuid).transpose().ok()
            && r.mode == mode
            && r.packets.contains(&bindings::hex(bytes))
    })
}
#[derive(Clone)]
struct Attribute {
    handle: u16,
    service_handle: u16,
    reference: String,
    service: String,
    uuid: String,
    properties: u8,
    real: Option<Characteristic>,
    value: Vec<u8>,
    descriptors: Vec<Value>,
}
enum Backend {
    #[cfg_attr(windows, allow(dead_code))] // portable fallback used on other platforms
    Real(Peripheral),
    #[cfg(windows)]
    Windows(Arc<crate::windows_gatt::Connection>),
    Mock {
        delay: u64,
        fail: Mutex<Option<String>>,
    },
}
struct Core {
    backend: Backend,
    attributes: Mutex<Vec<Attribute>>,
    gate: Arc<tokio::sync::Mutex<()>>,
    closed: AtomicBool,
    finished: AtomicBool,
    connected: AtomicBool,
    events: Arc<Queue>,
    notifications: Mutex<BTreeMap<String, Vec<Arc<Queue>>>>,
    notification_cancel: Mutex<CancellationToken>,
    allocation: Mutex<Option<crate::bindings::Charge>>,
    subscribed: Mutex<BTreeSet<String>>,
    cancel: CancellationToken,
    path: String,
    generation: String,
    owner: String,
    shared: Arc<Shared>,
    policy: Arc<bindings::Policy>,
    started: Instant,
}
impl Core {
    fn trace(&self, operation: &str, data: Value) {
        self.shared.emit(&self.owner,"gatt",json!({"path":self.path,"generation":self.generation,"operation":operation,"elapsed_ms":self.started.elapsed().as_millis() as u64,"data":data}));
    }
    fn state(&self, kind: &str) {
        let value = json!({"kind":kind,"path":self.path,"generation":self.generation});
        self.events.push(value.clone());
        self.trace("connection", value);
    }
    fn attribute(&self, reference: &str) -> Result<Attribute, Fault> {
        let a = self.attributes.lock().unwrap();
        a.iter().find(|a|a.reference==reference).cloned().ok_or_else(||Fault::new("not_found","gatt.attribute","use a characteristic reference from this connection's services(); UUID alone is not a reference"))
    }
    fn ensure(&self) -> Result<(), Fault> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Fault::new("closed", "gatt", "connection closed"));
        }
        if !self.connected.load(Ordering::Acquire) {
            return Err(Fault::new(
                "disconnected",
                "gatt",
                "device disconnected; reconnect explicitly",
            ));
        }
        Ok(())
    }
    fn deliver(&self, reference: &str, bytes: Vec<u8>) {
        if bytes.len() > 512 {
            self.trace("notification_error", json!({"code":"oversized_value"}));
            return;
        }
        let value = json!({"kind":"notification","reference":reference,"hex":bindings::hex(&bytes),"length":bytes.len(),"generation":self.generation});
        if let Some(queues) = self.notifications.lock().unwrap().get_mut(reference) {
            queues.retain(|q| !q.is_closed());
            for queue in queues {
                queue.push(value.clone());
            }
        }
        self.trace("notification", value);
    }
    fn close(self: &Arc<Self>) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancel.cancel();
        for queues in self.notifications.lock().unwrap().values() {
            for q in queues {
                q.close();
            }
        }
        self.events.close();
        let c = self.clone();
        executor().spawn(async move {
            let _guard = c.gate.lock().await;
            if let Backend::Real(p) = &c.backend {
                let _ = p.disconnect().await;
            }
            #[cfg(windows)]
            if let Backend::Windows(p) = &c.backend {
                p.close();
            }
            c.connected.store(false, Ordering::Release);
            c.attributes.lock().unwrap().clear();
            c.allocation.lock().unwrap().take();
            c.shared.leases.lock().unwrap().remove(&c.path);
            c.finished.store(true, Ordering::Release);
            c.trace("closed", json!({}));
        });
    }
}
#[derive(Clone)]
struct Gatt {
    core: Arc<Core>,
    budget: Arc<crate::bindings::Budget>,
}
impl Resource for Gatt {
    fn close(&self) {
        self.core.close();
    }
    fn closed(&self) -> bool {
        self.core.finished.load(Ordering::Acquire)
    }
}

// Waiting caller can time out/cancel. The bounded worker retains its gate/lease
// until OS completion; a timeout never creates an unlimited replacement worker.
async fn operation<T: Send + 'static>(
    core: Arc<Core>,
    name: &'static str,
    timeout: u64,
    work: impl FnOnce(
        Arc<Core>,
    )
        -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, Fault>> + Send>>
    + Send
    + 'static,
) -> Result<T, Fault> {
    if timeout == 0 || timeout > 120000 {
        return Err(Fault::new(
            "invalid_argument",
            name,
            "timeout must be 1..120000 ms",
        ));
    }
    let permit = hub()
        .operations
        .clone()
        .try_acquire_owned()
        .map_err(|_| Fault::new("busy", name, "16 Bluetooth operations already pending"))?;
    let connection_cancel = core.cancel.clone();
    let cancel = crate::scheduler::current_cancel();
    let _cancel_on_drop = cancel.clone().drop_guard();
    let worker_cancel = cancel.clone();
    let handle = executor().spawn(async move {
        let _permit = permit;
        let _guard = tokio::select! {
            biased;
            _ = worker_cancel.cancelled() => return Err(Fault::new("cancelled", name, "cancelled before execution")),
            guard = core.gate.clone().lock_owned() => guard,
        };
        if worker_cancel.is_cancelled() { return Err(Fault::new("cancelled", name, "cancelled before execution")); }
        if core.closed.load(Ordering::Acquire) {
            return Err(Fault::new("closed", name, "connection closed"));
        }
        if name != "reconnect" {
            core.ensure()?;
        }
        if let Backend::Mock { delay, fail } = &core.backend {
            tokio::time::sleep(Duration::from_millis(*delay)).await;
            if let Some(code) = fail.lock().unwrap().take() {
                return Err(Fault::new(&code, name, "injected GATT failure"));
            }
        }
        if worker_cancel.is_cancelled() { return Err(Fault::new("cancelled", name, "cancelled before execution")); }
        let result = {
            #[cfg(windows)]
            if matches!(core.backend,Backend::Windows(_)) {
                tokio::select!{
                    _=worker_cancel.cancelled()=>Err(Fault::new("cancelled",name,"Windows operation cancelled; connection invalidated")),
                    _=core.cancel.cancelled()=>Err(Fault::new("closed",name,"connection closing")),
                    r=tokio::time::timeout(Duration::from_secs(30),work(core.clone()))=>r.unwrap_or_else(|_|Err(Fault::new("timeout",name,"Windows native deadline; connection invalidated"))),
                }
            } else {work(core.clone()).await}
            #[cfg(not(windows))]
            {work(core.clone()).await}
        };
        #[cfg(windows)]
        if result.as_ref().is_err_and(|e|["cancelled","closed","timeout","disconnected"].contains(&e.code.as_str())) && let Backend::Windows(p)=&core.backend {p.close();core.connected.store(false,Ordering::Release);core.state("disconnected");}
        core.trace(
            name,
            json!({"ok":result.is_ok(),"error":result.as_ref().err()}),
        );
        result
    });
    tokio::select! {
        _=connection_cancel.cancelled()=>Err(Fault::new("cancelled",name,"connection cancelled; native completion may still be pending")),
        r=tokio::time::timeout(Duration::from_millis(timeout),handle)=>r.map_err(|_|Fault::new("timeout",name,"caller deadline reached; native operation retains its bounded slot until completion"))?.map_err(|e|Fault::new("backend",name,e))?,
    }
}
fn attribute_json(a: &Attribute) -> Value {
    json!({"handle":a.handle,"service_handle":a.service_handle,"reference":a.reference,"uuid":a.uuid,"service":a.service,"properties":{"read":a.properties&2!=0,"write":a.properties&8!=0,"write_without_response":a.properties&4!=0,"notify":a.properties&16!=0,"indicate":a.properties&32!=0},"descriptors":a.descriptors})
}

impl UserData for Gatt {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("inspect",|lua,g,()|lua.to_value(&json!({"backend":"ble","path":g.core.path,"generation":g.core.generation,"connected":g.core.connected.load(Ordering::Acquire),"closed":g.core.closed.load(Ordering::Acquire),"native_pending":g.core.gate.try_lock().is_err(),"capabilities":capabilities(&g.core),"mtu":mtu(&g.core)})));
        m.add_method("mtu", |_, g, ()| Ok(mtu(&g.core)));
        m.add_async_method(
            "write_reliable",
            |_, g, (items, opts): (Table, Option<Table>)| async move {
                let timeout = options(opts.as_ref())?;
                let mut values = vec![];
                for item in items.sequence_values::<Table>() {
                    if values.len() >= 16 {
                        return Err(Fault::new(
                            "resource_limit",
                            "gatt.reliable",
                            "maximum 16 writes",
                        )
                        .lua());
                    }
                    let item = item?;
                    let a = g
                        .core
                        .attribute(&item.get::<String>("reference")?)
                        .map_err(Fault::lua)?;
                    let data = item
                        .get::<AnyUserData>("payload")?
                        .borrow::<Buffer>()?
                        .bytes()
                        .to_vec();
                    if data.len() > 512 || a.properties & 8 == 0 {
                        return Err(Fault::new(
                            "invalid_argument",
                            "gatt.reliable",
                            "write-with-response property and at most 512 bytes required",
                        )
                        .lua());
                    }
                    if !matches!(g.core.backend, Backend::Mock { .. })
                        && !allowed_attribute(&g.core, &a, None, "reliable", &data)
                    {
                        return Err(Fault::new(
                            "permission_denied",
                            "gatt.reliable",
                            "no exact reliable-write rule matches",
                        )
                        .lua());
                    }
                    values.push((a.reference, data));
                }
                if values.is_empty() {
                    return Err(Fault::new(
                        "invalid_argument",
                        "gatt.reliable",
                        "empty transaction",
                    )
                    .lua());
                }
                operation(g.core.clone(), "write_reliable", timeout, move |c| {
                    Box::pin(async move {
                        let mut writes = vec![];
                        for (reference, data) in values {
                            writes.push((c.attribute(&reference)?, data));
                        }
                        match &c.backend {
                            #[cfg(windows)]
                            Backend::Windows(p) => {
                                p.reliable(writes.into_iter().map(|(a, b)| (a.handle, b)).collect())
                                    .await
                            }
                            Backend::Mock { .. } => {
                                let mut attrs = c.attributes.lock().unwrap();
                                for (a, data) in writes {
                                    attrs
                                        .iter_mut()
                                        .find(|v| v.reference == a.reference)
                                        .unwrap()
                                        .value = data;
                                }
                                Ok(())
                            }
                            _ => Err(Fault::new(
                                "unsupported",
                                "gatt.reliable",
                                "platform backend has no reliable write support",
                            )),
                        }
                    })
                })
                .await
                .map_err(Fault::lua)
            },
        );
        m.add_method("services", |lua, g, ()| {
            g.core.ensure().map_err(Fault::lua)?;
            lua.to_value(
                &g.core
                    .attributes
                    .lock()
                    .unwrap()
                    .iter()
                    .map(attribute_json)
                    .collect::<Vec<_>>(),
            )
        });
        m.add_method("close", |_, g, ()| {
            g.core.close();
            Ok(())
        });
        m.add_method("events", |_, g, ()| Ok(Subscription(g.core.events.clone())));
        m.add_async_method(
            "read",
            |_, g, (reference, opts): (String, Option<Table>)| async move {
                let timeout = options(opts.as_ref())?;
                let cache = cache_option(opts.as_ref())?;
                let a = g.core.attribute(&reference).map_err(Fault::lua)?;
                if a.properties & 2 == 0 {
                    return Err(Fault::new(
                        "unsupported",
                        "gatt.read",
                        "characteristic is not readable",
                    )
                    .lua());
                }
                let bytes = operation(g.core.clone(), "read", timeout, move |c| {
                    Box::pin(async move {
                        let a = c.attribute(&a.reference)?;
                        match &c.backend {
                            #[cfg(windows)]
                            Backend::Windows(p) => p.read(a.handle, &cache).await,
                            Backend::Real(p) => p
                                .read(a.real.as_ref().unwrap())
                                .await
                                .map_err(|e| io_error("gatt.read", e)),
                            Backend::Mock { .. } => Ok(a.value),
                        }
                    })
                })
                .await
                .map_err(Fault::lua)?;
                if bytes.len() > 512 {
                    return Err(Fault::new(
                        "resource_limit",
                        "gatt.read",
                        "value exceeds 512 bytes",
                    )
                    .lua());
                }
                Buffer::new(bytes, &g.budget)
            },
        );
        m.add_async_method("write",|_,g,(reference,payload,mode,opts):(String,AnyUserData,String,Option<Table>)|async move{
            let timeout=options(opts.as_ref())?;let bytes=payload.borrow::<Buffer>()?.bytes().to_vec();let a=g.core.attribute(&reference).map_err(Fault::lua)?;
            if bytes.len()>512{return Err(Fault::new("invalid_argument","gatt.write","maximum 512 bytes").lua());}
            if mode=="without_response" && mtu(&g.core).is_some_and(|n|bytes.len()>n.saturating_sub(3) as usize){return Err(Fault::new("invalid_argument","gatt.write","value exceeds MTU-3").lua());}
            let flag=match mode.as_str(){"with_response"=>8,"without_response"=>4,_=>return Err(Fault::new("invalid_argument","gatt.write","mode must be with_response or without_response").lua())};
            if a.properties&flag==0{return Err(Fault::new("unsupported","gatt.write","write mode not supported by characteristic").lua());}
            if !matches!(g.core.backend,Backend::Mock{..})&&!allowed_attribute(&g.core,&a,None,&mode,&bytes){return Err(Fault::new("permission_denied","gatt.write","no exact GATT write policy matches").lua());}
            operation(g.core.clone(),"write",timeout,move|c|Box::pin(async move{
                let a = c.attribute(&a.reference)?;
                match &c.backend{
                #[cfg(windows)]
                Backend::Windows(p)=>p.write(a.handle,&bytes,mode=="with_response").await,
                Backend::Real(p)=>p.write(a.real.as_ref().unwrap(),&bytes,if mode=="with_response"{WriteType::WithResponse}else{WriteType::WithoutResponse}).await.map_err(|e|io_error("gatt.write",e)),Backend::Mock{..}=>{if let Some(found)=c.attributes.lock().unwrap().iter_mut().find(|x|x.reference==a.reference){found.value=bytes;}Ok(())}}
            })).await.map_err(Fault::lua)
        });
        m.add_async_method(
            "subscribe",
            |_, g, (reference, opts): (String, Option<Table>)| async move {
                let timeout = options(opts.as_ref())?;
                let a = g.core.attribute(&reference).map_err(Fault::lua)?;
                if a.properties & (16 | 32) == 0 {
                    return Err(Fault::new(
                        "unsupported",
                        "gatt.subscribe",
                        "no notify/indicate property",
                    )
                    .lua());
                }
                let queue = Queue::new(64);
                {
                    let mut observers = g.core.notifications.lock().unwrap();
                    for queues in observers.values_mut() {
                        queues.retain(|q| !q.is_closed());
                    }
                    if observers.values().map(Vec::len).sum::<usize>() >= 32 {
                        return Err(Fault::new(
                            "resource_limit",
                            "gatt.subscribe",
                            "maximum 32 subscriptions per connection",
                        )
                        .lua());
                    }
                    observers
                        .entry(reference.clone())
                        .or_default()
                        .push(queue.clone());
                }
                let result = operation(g.core.clone(), "subscribe", timeout, move |c| {
                    Box::pin(async move {
                        let a = c.attribute(&a.reference)?;
                        let subscribed = c.subscribed.lock().unwrap().contains(&a.reference);
                        if !subscribed {
                            if let Backend::Real(p) = &c.backend {
                                p.subscribe(a.real.as_ref().unwrap())
                                    .await
                                    .map_err(|e| io_error("gatt.subscribe", e))?;
                            }
                            #[cfg(windows)]
                            if let Backend::Windows(p) = &c.backend {
                                let weak = Arc::downgrade(&c);
                                let reference = a.reference.clone();
                                p.subscribe(
                                    a.handle,
                                    Arc::new(move |bytes| {
                                        if let Some(c) = weak.upgrade() {
                                            c.deliver(&reference, bytes);
                                        }
                                    }),
                                )
                                .await?;
                            }
                            c.subscribed.lock().unwrap().insert(a.reference);
                        }
                        Ok(())
                    })
                })
                .await;
                if let Err(e) = result {
                    queue.close();
                    return Err(e.lua());
                }
                Ok(Subscription(queue))
            },
        );
        m.add_async_method("unsubscribe", |_, g, reference: String| async move {
            let a = g.core.attribute(&reference).map_err(Fault::lua)?;
            operation(g.core.clone(), "unsubscribe", 5000, move |c| {
                Box::pin(async move {
                    let a = c.attribute(&a.reference)?;
                    if let Backend::Real(p) = &c.backend {
                        p.unsubscribe(a.real.as_ref().unwrap())
                            .await
                            .map_err(|e| io_error("gatt.unsubscribe", e))?;
                    }
                    #[cfg(windows)]
                    if let Backend::Windows(p) = &c.backend {
                        p.unsubscribe(a.handle).await?;
                    }
                    c.subscribed.lock().unwrap().remove(&a.reference);
                    if let Some(queues) = c.notifications.lock().unwrap().remove(&a.reference) {
                        for q in queues {
                            q.close();
                        }
                    }
                    Ok(())
                })
            })
            .await
            .map_err(Fault::lua)
        });
        m.add_async_method(
            "read_descriptor",
            |_, g, (reference, descriptor, opts): (String, String, Option<Table>)| async move {
                let timeout = options(opts.as_ref())?;
                let cache = cache_option(opts.as_ref())?;
                let a = g.core.attribute(&reference).map_err(Fault::lua)?;
                let selected = select_descriptor(&a, &descriptor).map_err(Fault::lua)?;
                let id = uuid(selected["uuid"].as_str().unwrap()).map_err(Fault::lua)?;
                let bytes = operation(g.core.clone(), "read_descriptor", timeout, move |c| {
                    Box::pin(async move {
                        let a = c.attribute(&a.reference)?;
                        match &c.backend {
                            #[cfg(windows)]
                            Backend::Windows(p) => {
                                p.read_descriptor(
                                    selected["handle"].as_u64().unwrap() as u16,
                                    &cache,
                                )
                                .await
                            }
                            Backend::Real(p) => {
                                let d = a
                                    .real
                                    .as_ref()
                                    .unwrap()
                                    .descriptors
                                    .iter()
                                    .find(|d| d.uuid == id)
                                    .ok_or_else(|| {
                                        Fault::new(
                                            "not_found",
                                            "gatt.descriptor",
                                            "descriptor missing",
                                        )
                                    })?;
                                p.read_descriptor(d)
                                    .await
                                    .map_err(|e| io_error("gatt.read_descriptor", e))
                            }
                            Backend::Mock { .. } => {
                                let d = a
                                    .descriptors
                                    .iter()
                                    .find(|d| d["reference"] == selected["reference"])
                                    .ok_or_else(|| {
                                        Fault::new(
                                            "not_found",
                                            "gatt.descriptor",
                                            "descriptor missing",
                                        )
                                    })?;
                                bindings::unhex(d["hex"].as_str().unwrap_or("")).map_err(|e| {
                                    Fault::new("invalid_argument", "mock.descriptor", e)
                                })
                            }
                        }
                    })
                })
                .await
                .map_err(Fault::lua)?;
                if bytes.len() > 512 {
                    return Err(Fault::new(
                        "resource_limit",
                        "gatt.read",
                        "value exceeds 512 bytes",
                    )
                    .lua());
                }
                Buffer::new(bytes, &g.budget)
            },
        );
        m.add_async_method(
            "write_descriptor",
            |_,
             g,
             (reference, descriptor, payload, opts): (
                String,
                String,
                AnyUserData,
                Option<Table>,
            )| async move {
                let timeout = options(opts.as_ref())?;
                let a = g.core.attribute(&reference).map_err(Fault::lua)?;
                if uuid(&descriptor).ok() == uuid("2902").ok() {
                    return Err(Fault::new(
                        "invalid_argument",
                        "gatt.write_descriptor",
                        "use subscribe/unsubscribe for CCCD",
                    )
                    .lua());
                }
                let selected = select_descriptor(&a, &descriptor).map_err(Fault::lua)?;
                let id = uuid(selected["uuid"].as_str().unwrap()).map_err(Fault::lua)?;
                let bytes = payload.borrow::<Buffer>()?.bytes().to_vec();
                if bytes.len() > 512 {
                    return Err(Fault::new(
                        "invalid_argument",
                        "gatt.write_descriptor",
                        "maximum 512 bytes",
                    )
                    .lua());
                }
                if id == uuid("2902").unwrap() {
                    return Err(Fault::new(
                        "invalid_argument",
                        "gatt.write_descriptor",
                        "use subscribe/unsubscribe for CCCD",
                    )
                    .lua());
                }
                if !matches!(g.core.backend, Backend::Mock { .. })
                    && !allowed_attribute(&g.core, &a, Some(&selected), "descriptor", &bytes)
                {
                    return Err(Fault::new(
                        "permission_denied",
                        "gatt.write_descriptor",
                        "no exact descriptor write policy matches",
                    )
                    .lua());
                }
                operation(g.core.clone(), "write_descriptor", timeout, move |c| {
                    Box::pin(async move {
                        let a = c.attribute(&a.reference)?;
                        match &c.backend {
                            #[cfg(windows)]
                            Backend::Windows(p) => {
                                p.write_descriptor(
                                    selected["handle"].as_u64().unwrap() as u16,
                                    &bytes,
                                )
                                .await
                            }
                            Backend::Real(p) => {
                                let d = a
                                    .real
                                    .as_ref()
                                    .unwrap()
                                    .descriptors
                                    .iter()
                                    .find(|d| d.uuid == id)
                                    .ok_or_else(|| {
                                        Fault::new(
                                            "not_found",
                                            "gatt.descriptor",
                                            "descriptor missing",
                                        )
                                    })?;
                                p.write_descriptor(d, &bytes)
                                    .await
                                    .map_err(|e| io_error("gatt.write_descriptor", e))
                            }
                            Backend::Mock { .. } => {
                                let mut attrs = c.attributes.lock().unwrap();
                                let a = attrs
                                    .iter_mut()
                                    .find(|v| v.reference == reference)
                                    .ok_or_else(|| {
                                        Fault::new(
                                            "not_found",
                                            "gatt.descriptor",
                                            "stale reference",
                                        )
                                    })?;
                                let d = a
                                    .descriptors
                                    .iter_mut()
                                    .find(|d| d["reference"] == selected["reference"])
                                    .ok_or_else(|| {
                                        Fault::new(
                                            "not_found",
                                            "gatt.descriptor",
                                            "descriptor missing",
                                        )
                                    })?;
                                d["hex"] = json!(bindings::hex(&bytes));
                                Ok(())
                            }
                        }
                    })
                })
                .await
                .map_err(Fault::lua)
            },
        );
        m.add_async_method("reconnect", |lua, g, opts: Option<Table>| async move {
            let timeout = options(opts.as_ref())?;
            let cache = cache_option(opts.as_ref())?;
            operation(g.core.clone(), "reconnect", timeout, move |c| {
                Box::pin(async move {
                    c.connected.store(false, Ordering::Release);
                    for queues in c.notifications.lock().unwrap().values() {
                        for q in queues {
                            q.close();
                        }
                    }
                    c.notifications.lock().unwrap().clear();
                    c.subscribed.lock().unwrap().clear();
                    let epoch = uuid::Uuid::new_v4().to_string();
                    match &c.backend {
                        #[cfg(windows)]
                        Backend::Windows(p) => {
                            let weak = Arc::downgrade(&c);
                            let values = p
                                .connect(
                                    &cache,
                                    Arc::new(move || {
                                        if let Some(c) = weak.upgrade() {
                                            c.connected.store(false, Ordering::Release);
                                            c.state("disconnected");
                                        }
                                    }),
                                )
                                .await?;
                            *c.attributes.lock().unwrap() = windows_attributes(values, &epoch);
                        }
                        Backend::Real(p) => {
                            p.connect()
                                .await
                                .map_err(|e| io_error("gatt.reconnect", e))?;
                            p.discover_services()
                                .await
                                .map_err(|e| io_error("gatt.discover", e))?;
                            let attrs = p
                                .characteristics()
                                .into_iter()
                                .enumerate()
                                .map(|(index, a)| Attribute {
                                    handle: 0,
                                    service_handle: 0,
                                    reference: format!("{epoch}:{index}"),
                                    service: a.service_uuid.to_string(),
                                    uuid: a.uuid.to_string(),
                                    properties: a.properties.bits(),
                                    value: vec![],
                                    descriptors: a
                                        .descriptors
                                        .iter()
                                        .map(|d| json!({"uuid":d.uuid.to_string()}))
                                        .collect(),
                                    real: Some(a),
                                })
                                .collect::<Vec<_>>();
                            if attrs.len() > 256 {
                                return Err(Fault::new(
                                    "resource_limit",
                                    "gatt.discover",
                                    "more than 256 characteristics",
                                ));
                            }
                            *c.attributes.lock().unwrap() = attrs;
                            notification_pump(c.clone(), p).await?;
                        }
                        Backend::Mock { .. } => {
                            for (index, a) in c.attributes.lock().unwrap().iter_mut().enumerate() {
                                a.reference = format!("{epoch}:{index}");
                                for (i, d) in a.descriptors.iter_mut().enumerate() {
                                    d["reference"] =
                                        json!(format!("{epoch}:{index}:descriptor:{i}"));
                                }
                            }
                        }
                    }
                    c.connected.store(true, Ordering::Release);
                    c.state("reconnected");
                    Ok(())
                })
            })
            .await
            .map_err(Fault::lua)?;
            lua.to_value(
                &g.core
                    .attributes
                    .lock()
                    .unwrap()
                    .iter()
                    .map(attribute_json)
                    .collect::<Vec<_>>(),
            )
        });
        m.add_method(
            "mock_notify",
            |_, g, (reference, payload): (String, AnyUserData)| {
                if !matches!(g.core.backend, Backend::Mock { .. }) {
                    return Err(Fault::new(
                        "unsupported",
                        "mock_notify",
                        "only mock GATT supports injection",
                    )
                    .lua());
                }
                g.core.ensure().map_err(Fault::lua)?;
                g.core.attribute(&reference).map_err(Fault::lua)?;
                g.core
                    .deliver(&reference, payload.borrow::<Buffer>()?.bytes().to_vec());
                Ok(())
            },
        );
        m.add_method("mock_disconnect", |_, g, ()| {
            if !matches!(g.core.backend, Backend::Mock { .. }) {
                return Err(Fault::new("unsupported", "mock_disconnect", "mock only").lua());
            }
            g.core.connected.store(false, Ordering::Release);
            g.core.state("disconnected");
            Ok(())
        });
        m.add_method("mock_fail", |_, g, code: String| {
            if let Backend::Mock { fail, .. } = &g.core.backend {
                *fail.lock().unwrap() = Some(code);
                Ok(())
            } else {
                Err(Fault::new("unsupported", "mock_fail", "mock only").lua())
            }
        });
    }
}
fn cache_option(opts: Option<&Table>) -> mlua::Result<String> {
    Ok(opts
        .map(|t| t.get::<Option<String>>("cache"))
        .transpose()?
        .flatten()
        .unwrap_or_else(|| "backend_default".into()))
}
fn select_descriptor(a: &Attribute, key: &str) -> Result<Value, Fault> {
    if let Some(d) = a
        .descriptors
        .iter()
        .find(|d| d["reference"].as_str() == Some(key))
    {
        return Ok(d.clone());
    }
    let id = uuid(key)?.to_string();
    let found: Vec<_> = a.descriptors.iter().filter(|d| d["uuid"] == id).collect();
    match found.as_slice() {
        [d] => Ok((*d).clone()),
        [] => Err(Fault::new(
            "not_found",
            "gatt.descriptor",
            "descriptor missing or stale",
        )),
        _ => Err(Fault::new(
            "ambiguous",
            "gatt.descriptor",
            "duplicate UUID: use descriptor reference",
        )),
    }
}
fn allowed_attribute(c: &Core, a: &Attribute, d: Option<&Value>, mode: &str, bytes: &[u8]) -> bool {
    if !allowed(
        &c.policy,
        &c.path,
        &a.service,
        &a.uuid,
        d.and_then(|d| d["uuid"].as_str()),
        mode,
        bytes,
    ) {
        return false;
    }
    let duplicate = c
        .attributes
        .lock()
        .unwrap()
        .iter()
        .filter(|b| b.service == a.service && b.uuid == a.uuid)
        .count()
        > 1;
    let duplicate_d = d.is_some_and(|d| {
        a.descriptors
            .iter()
            .filter(|b| b["uuid"] == d["uuid"])
            .count()
            > 1
    });
    c.policy.ble_write_rules.iter().any(|r| {
        r.path == c.path
            && uuid(&r.service).ok() == uuid(&a.service).ok()
            && uuid(&r.characteristic).ok() == uuid(&a.uuid).ok()
            && r.descriptor.as_deref().map(uuid).transpose().ok()
                == d.and_then(|d| d["uuid"].as_str())
                    .map(uuid)
                    .transpose()
                    .ok()
            && r.mode == mode
            && r.packets.contains(&bindings::hex(bytes))
            && r.service_handle.is_none_or(|h| h == a.service_handle)
            && r.characteristic_handle.is_none_or(|h| h == a.handle)
            && (!duplicate
                || (r.service_handle == Some(a.service_handle)
                    && r.characteristic_handle == Some(a.handle)))
            && r.descriptor_handle
                .is_none_or(|h| d.is_some_and(|d| d["handle"] == h))
            && (!duplicate_d || r.descriptor_handle.is_some())
    })
}
fn capabilities(c: &Core) -> Value {
    let native = match &c.backend {
        #[cfg(windows)]
        Backend::Windows(_) => true,
        Backend::Mock { .. } => true,
        _ => false,
    };
    json!({"cache_mode":if native{"explicit"}else{"backend_default"},"duplicate_attribute_instances":native,"write":"exact allowlist","notifications":true,"reliable_write":native,"mtu_query":native,"mtu_negotiation":"OS-managed","long_write":if native{"with_response_up_to_512"}else{"backend-dependent"}})
}
fn mtu(c: &Core) -> Option<u16> {
    match &c.backend {
        #[cfg(windows)]
        Backend::Windows(p) => p.mtu(),
        Backend::Mock { .. } => Some(23),
        _ => None,
    }
}
#[cfg(windows)]
fn windows_address(v: &Value) -> Result<(u64, bool), Fault> {
    let address = v["address"]
        .as_str()
        .or_else(|| v["id"].as_str())
        .unwrap_or("")
        .replace(':', "");
    u64::from_str_radix(&address, 16)
        .map(|a| {
            (
                a,
                v["address_type"]
                    .as_str()
                    .is_some_and(|s| s.contains("Random")),
            )
        })
        .map_err(|e| Fault::new("invalid_argument", "ble.address", e))
}
#[cfg(windows)]
fn windows_attributes(values: Vec<Value>, epoch: &str) -> Vec<Attribute> {
    values
        .into_iter()
        .map(|v| {
            let handle = v["handle"].as_u64().unwrap() as u16;
            let reference = format!("{epoch}:{handle}");
            let mut descriptors = v["descriptors"].as_array().unwrap().clone();
            for d in &mut descriptors {
                d["reference"] = json!(format!("{reference}:descriptor:{}", d["handle"]));
            }
            Attribute {
                reference,
                handle,
                service_handle: v["service_handle"].as_u64().unwrap() as u16,
                service: v["service"].as_str().unwrap().into(),
                uuid: v["uuid"].as_str().unwrap().into(),
                properties: v["properties"].as_u64().unwrap() as u8,
                real: None,
                value: vec![],
                descriptors,
            }
        })
        .collect()
}
fn options(opts: Option<&Table>) -> mlua::Result<u64> {
    if let Some(opts) = opts {
        if let Some(cache) = opts.get::<Option<String>>("cache")?
            && !["backend_default", "cached", "uncached"].contains(&cache.as_str())
        {
            return Err(Fault::new(
                "invalid_argument",
                "gatt.cache",
                "cache must be backend_default/cached/uncached",
            )
            .lua());
        }
        Ok(opts.get::<Option<u64>>("timeout_ms")?.unwrap_or(10000))
    } else {
        Ok(10000)
    }
}
fn new_core(
    ctx: &Context,
    path: String,
    backend: Backend,
    attributes: Vec<Attribute>,
) -> Arc<Core> {
    Arc::new(Core {
        backend,
        attributes: Mutex::new(attributes),
        gate: Arc::new(tokio::sync::Mutex::new(())),
        closed: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        connected: AtomicBool::new(true),
        events: Queue::new(64),
        notifications: Mutex::new(BTreeMap::new()),
        notification_cancel: Mutex::new(CancellationToken::new()),
        allocation: Mutex::new(None),
        subscribed: Mutex::new(BTreeSet::new()),
        cancel: CancellationToken::new(),
        path,
        generation: uuid::Uuid::new_v4().to_string(),
        owner: ctx.name.clone(),
        shared: ctx.shared.clone(),
        policy: ctx.policy.clone(),
        started: Instant::now(),
    })
}

async fn notification_pump(core: Arc<Core>, peripheral: &Peripheral) -> Result<(), Fault> {
    core.notification_cancel.lock().unwrap().cancel();
    let token = core.cancel.child_token();
    *core.notification_cancel.lock().unwrap() = token.clone();
    let mut stream = peripheral
        .notifications()
        .await
        .map_err(|e| io_error("gatt.notifications", e))?;
    executor().spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                next = stream.next() => {
                    let Some(n) = next else { core.connected.store(false, Ordering::Release); core.state("disconnected"); break; };
                    let refs = core.attributes.lock().unwrap().iter().filter(|a| a.uuid == n.uuid.to_string() && a.service == n.service_uuid.to_string()).map(|a| a.reference.clone()).collect::<Vec<_>>();
                    if refs.len() == 1 { core.deliver(&refs[0], n.value); }
                    else { core.trace("ambiguous_notification", json!({"uuid":n.uuid.to_string()})); }
                }
            }
        }
    });
    Ok(())
}

struct Opening(Option<Arc<Core>>);
impl Drop for Opening {
    fn drop(&mut self) {
        if let Some(core) = self.0.take() {
            core.close();
        }
    }
}

struct Watch {
    queue: Arc<Queue>,
    cancel: CancellationToken,
}
impl Resource for Watch {
    fn close(&self) {
        self.cancel.cancel();
        self.queue.close();
    }
    fn closed(&self) -> bool {
        self.queue.is_closed()
    }
}

pub fn install(lua: &Lua, ctx: &Context) -> mlua::Result<()> {
    let dummy = new_core(
        ctx,
        String::new(),
        Backend::Mock {
            delay: 0,
            fail: Mutex::new(None),
        },
        vec![],
    );
    dummy.closed.store(true, Ordering::Release);
    dummy.finished.store(true, Ordering::Release);
    lua.create_userdata(Gatt {
        core: dummy,
        budget: ctx.budget.clone(),
    })?;

    let ble = lua.create_table()?;
    ble.set(
        "adapters",
        lua.create_async_function(|lua, ()| async move {
            lua.to_value(&adapter_info().await.map_err(Fault::lua)?)
        })?,
    )?;
    ble.set(
        "list",
        lua.create_async_function(|lua, filter: Option<Table>| async move {
            let filter = filter
                .map(|t| bindings::to_json(LV::Table(t), 0))
                .transpose()?
                .unwrap_or(json!({}));
            lua.to_value(&list(filter).await.map_err(Fault::lua)?)
        })?,
    )?;
    ble.set(
        "scan",
        lua.create_async_function(|lua, opts: Option<Table>| async move {
            let duration = opts
                .as_ref()
                .map(|o| o.get::<Option<u64>>("duration_ms"))
                .transpose()?
                .flatten()
                .unwrap_or(5000);
            let index = opts
                .as_ref()
                .map(|o| o.get::<Option<usize>>("adapter"))
                .transpose()?
                .flatten()
                .unwrap_or(0);
            let services = if let Some(t) = opts
                .as_ref()
                .map(|o| o.get::<Option<Table>>("services"))
                .transpose()?
                .flatten()
            {
                t.sequence_values::<String>()
                    .map(|s| uuid(&s?).map_err(Fault::lua))
                    .collect::<mlua::Result<Vec<_>>>()?
            } else {
                vec![]
            };
            lua.to_value(&scan(duration, index, services).await.map_err(Fault::lua)?)
        })?,
    )?;
    let c = ctx.clone();
    ble.set("watch",lua.create_async_function(move|_,opts:Option<Table>|{let c=c.clone();async move{
        let index=opts.as_ref().map(|o|o.get::<Option<usize>>("adapter")).transpose()?.flatten().unwrap_or(0);
        let queue=Queue::new(64);let token=queue.cancellation.clone();c.manage(Rc::new(Watch{queue:queue.clone(),cancel:token.clone()}))?;
        let q=queue.clone();let result=executor().spawn(async move{
            let adapter=adapters().await?.get(index).cloned().ok_or_else(||Fault::new("not_found","ble.watch","adapter not found"))?;
            let mut events=adapter.events().await.map_err(|e|io_error("ble.watch",e))?;
            executor().spawn(async move{
                loop{tokio::select!{_=token.cancelled()=>break,event=events.next()=>{let Some(event)=event else{break};
                    let(kind,id)=match &event{CentralEvent::DeviceDiscovered(id)=>("discovered",Some(id)),CentralEvent::DeviceUpdated(id)=>("updated",Some(id)),CentralEvent::DeviceConnected(id)=>("connected",Some(id)),CentralEvent::DeviceDisconnected(id)=>("disconnected",Some(id)),_=>("adapter_event",None)};
                    q.push(json!({"kind":kind,"adapter":index,"path":id.map(|id|format!("ble:{index}:{id}")),"detail":format!("{event:?}")}));
                }}}
                q.close();
            });Ok::<(),Fault>(())
        }).await.map_err(mlua::Error::external)?;
        if let Err(e)=result{queue.close();return Err(e.lua());}Ok(Subscription(queue))
    }})?)?;
    for name in ["pairing", "pair", "unpair"] {
        let context = ctx.clone();
        ble.set(
            name,
            lua.create_async_function(move |lua, (reference, opts): (Table, Option<Table>)| {
                let context = context.clone();
                async move {
                    let path = reference.get::<String>("path")?;
                    let protection = if name == "pair" {
                        Some(
                            opts.as_ref()
                                .map(|o| o.get::<Option<String>>("protection"))
                                .transpose()?
                                .flatten()
                                .unwrap_or_else(|| "encryption_and_authentication".into()),
                        )
                    } else {
                        None
                    };
                    if name != "pairing"
                        && !context.policy.ble_pair_rules.iter().any(|r| {
                            r.path == path && r.operation == name && r.protection == protection
                        })
                    {
                        return Err(Fault::new(
                            "permission_denied",
                            name,
                            "no exact pairing policy matches",
                        )
                        .lua());
                    }
                    if context.shared.leases.lock().unwrap().contains_key(&path) {
                        return Err(Fault::new(
                            "busy",
                            name,
                            "close the GATT connection before pairing operations",
                        )
                        .lua());
                    }
                    #[cfg(windows)]
                    {
                        let (address, random) = {
                            let h = hub().devices.lock().unwrap();
                            let v = &h
                                .get(&path)
                                .ok_or_else(|| {
                                    Fault::new(
                                        "not_found",
                                        name,
                                        "scan/list first; reference not present",
                                    )
                                    .lua()
                                })?
                                .2;
                            windows_address(v).map_err(Fault::lua)?
                        };
                        struct PairLease {shared:Arc<Shared>,path:String}
                        impl Drop for PairLease{fn drop(&mut self){self.shared.leases.lock().unwrap().remove(&self.path);}}
                        {
                            let mut leases=context.shared.leases.lock().unwrap();
                            if leases.contains_key(&path){return Err(Fault::new("busy",name,"device in use").lua());}
                            leases.insert(path.clone(),context.name.clone());
                        }
                        let lease=PairLease{shared:context.shared.clone(),path};
                        let cancel=crate::scheduler::current_cancel();let _cancel_on_exit=cancel.clone().drop_guard();
                        let result=discovery(name,35000,async move{
                            let _lease=lease;
                            tokio::select!{
                                biased;
                                _=cancel.cancelled()=>Err(Fault::new("cancelled",name,"pairing cancelled")),
                                r=tokio::time::timeout(Duration::from_secs(30),crate::windows_gatt::pairing(address,random,if name=="unpair"{Some("unpair")}else{protection.as_deref()}))=>r.map_err(|_|Fault::new("timeout",name,"Windows pairing deadline"))?,
                            }
                        }).await.map_err(Fault::lua)?;
                        lua.to_value(&result)
                    }
                    #[cfg(not(windows))]
                    {
                        let _ = (lua, protection);
                        Err::<LV, _>(
                            Fault::new(
                                "unsupported",
                                name,
                                "pairing backend is implemented on Windows",
                            )
                            .lua(),
                        )
                    }
                }
            })?,
        )?;
    }
    let c = ctx.clone();
    ble.set("open",lua.create_async_function(move|_,(reference,opts):(Table,Option<Table>)|{let c=c.clone();async move{
        let path=reference.get::<String>("path")?;let timeout=options(opts.as_ref())?;
        #[cfg(not(windows))]
        let(index,peripheral)=hub().devices.lock().unwrap().get(&path).map(|(index,p,_)|(*index,p.clone())).ok_or_else(||Fault::new("not_found","ble.open","scan/list first; device reference not present").lua())?;
        #[cfg(windows)]
        let (address,random)={let h=hub().devices.lock().unwrap();let v=&h.get(&path).ok_or_else(||Fault::new("not_found","ble.open","scan/list first; reference not present").lua())?.2;windows_address(v).map_err(Fault::lua)?};
        {let mut leases=c.shared.leases.lock().unwrap();if let Some(owner)=leases.get(&path){return Err(Fault::new("busy","ble.open",format!("device leased by {owner}")).lua());}leases.insert(path.clone(),c.name.clone());}
        #[cfg(windows)] {
            let native=Arc::new(crate::windows_gatt::Connection::new(address,random));
            let core=new_core(&c,path,Backend::Windows(native.clone()),vec![]);
            let g=Gatt{core:core.clone(),budget:c.budget.clone()};c.manage(Rc::new(g.clone()))?;
            let mut opening=Opening(Some(core.clone()));
            let cache=cache_option(opts.as_ref())?;
            operation(core.clone(),"connect",timeout,move|c|Box::pin(async move{
                let weak=Arc::downgrade(&c);
                let values=native.connect(&cache,Arc::new(move||if let Some(c)=weak.upgrade(){c.connected.store(false,Ordering::Release);c.state("disconnected");})).await?;
                *c.attributes.lock().unwrap()=windows_attributes(values,&c.generation);c.connected.store(true,Ordering::Release);c.state("connected");Ok(())
            })).await.map_err(Fault::lua)?;
            opening.0.take();Ok(g)
        }
        #[cfg(not(windows))] {
        let core=new_core(&c,path,Backend::Real(peripheral.clone()),vec![]);let g=Gatt{core:core.clone(),budget:c.budget.clone()};c.manage(Rc::new(g.clone()))?;
        let mut opening = Opening(Some(core.clone()));
        let result=operation(core.clone(),"connect",timeout,move|core|Box::pin(async move{
            let adapter=adapters().await?.get(index).cloned().ok_or_else(||Fault::new("not_found","gatt.connect","adapter disappeared"))?;
            let mut changes=adapter.events().await.map_err(|e|io_error("gatt.events",e))?;
            let cc=core.clone();let id=peripheral.id();executor().spawn(async move{loop{tokio::select!{_=cc.cancel.cancelled()=>break,e=changes.next()=>{match e{Some(CentralEvent::DeviceDisconnected(found))if found==id=>{cc.connected.store(false,Ordering::Release);cc.state("disconnected");},Some(_)=>{},None=>break}}}}});
            peripheral.connect().await.map_err(|e|io_error("gatt.connect",e))?;peripheral.discover_services().await.map_err(|e|io_error("gatt.discover",e))?;
            let attrs=peripheral.characteristics().into_iter().enumerate().map(|(index,a)|Attribute{handle:0,service_handle:0,reference:format!("{}:{index}",core.generation),service:a.service_uuid.to_string(),uuid:a.uuid.to_string(),properties:a.properties.bits(),value:vec![],descriptors:a.descriptors.iter().map(|d|json!({"uuid":d.uuid.to_string()})).collect(),real:Some(a)}).collect::<Vec<_>>();
            if attrs.len()>256{return Err(Fault::new("resource_limit","gatt.discover","more than 256 characteristics"));}
            *core.attributes.lock().unwrap()=attrs;
            notification_pump(core.clone(), &peripheral).await?;core.state("connected");Ok(())
        })).await;
        if let Err(e)=result{core.close();return Err(e.lua());}opening.0.take();Ok(g)
        }
    }})?)?;
    ble.set(
        "uuid",
        lua.create_function(|_, s: String| Ok(uuid(&s).map_err(Fault::lua)?.to_string()))?,
    )?;
    lua.globals().set("ble", ble)?;
    let mock: Table = lua.globals().get("mock")?;
    let c = ctx.clone();
    mock.set(
        "gatt",
        lua.create_function(move |_, spec: Table| {
            let delay = spec.get::<Option<u64>>("delay_ms")?.unwrap_or(0);
            if delay > 30000 {
                return Err(
                    Fault::new("invalid_argument", "mock.gatt", "delay exceeds 30000ms").lua(),
                );
            }
            let path = format!("mock-gatt:{}", uuid::Uuid::new_v4());
            let mut attrs = vec![];
            for item in spec
                .get::<Table>("characteristics")?
                .sequence_values::<Table>()
            {
                if attrs.len() >= 64 {
                    return Err(Fault::new(
                        "resource_limit",
                        "mock.gatt",
                        "64 characteristics maximum",
                    )
                    .lua());
                }
                let t = item?;
                let service = uuid(&t.get::<String>("service")?)
                    .map_err(Fault::lua)?
                    .to_string();
                let id = uuid(&t.get::<String>("uuid")?)
                    .map_err(Fault::lua)?
                    .to_string();
                let value = bindings::unhex(&t.get::<Option<String>>("hex")?.unwrap_or_default())?;
                if value.len() > 512 {
                    return Err(Fault::new(
                        "resource_limit",
                        "mock.gatt",
                        "value exceeds 512 bytes",
                    )
                    .lua());
                }
                let properties = t.get::<Option<u8>>("properties")?.unwrap_or(2 | 8 | 16);
                let mut descriptors = t
                    .get::<Option<Table>>("descriptors")?
                    .map(|t| bindings::to_json(LV::Table(t), 0))
                    .transpose()?
                    .and_then(|v| v.as_array().cloned())
                    .unwrap_or_default();
                if descriptors.len() > 16 {
                    return Err(Fault::new(
                        "resource_limit",
                        "mock.gatt",
                        "16 descriptors maximum",
                    )
                    .lua());
                }
                for (i, d) in descriptors.iter_mut().enumerate() {
                    d["handle"] = json!(i + 1);
                    d["reference"] = json!(format!("{path}:{}:descriptor:{i}", attrs.len()));
                    d["uuid"] = json!(
                        uuid(d["uuid"].as_str().unwrap_or(""))
                            .map_err(Fault::lua)?
                            .to_string()
                    );
                    let bytes = bindings::unhex(d["hex"].as_str().unwrap_or(""))?;
                    if bytes.len() > 512 {
                        return Err(Fault::new(
                            "resource_limit",
                            "mock.gatt",
                            "descriptor exceeds 512 bytes",
                        )
                        .lua());
                    }
                }
                attrs.push(Attribute {
                    handle: (attrs.len() + 1) as u16,
                    service_handle: t.get::<Option<u16>>("service_handle")?.unwrap_or(1),
                    reference: format!("{path}:{}", attrs.len()),
                    service,
                    uuid: id,
                    properties,
                    real: None,
                    value,
                    descriptors,
                });
            }
            let allocation = crate::bindings::Charge::new(
                c.budget.clone(),
                attrs.iter().map(|a| 512 * (1 + a.descriptors.len())).sum(),
            )?;
            let core = new_core(
                &c,
                path,
                Backend::Mock {
                    delay,
                    fail: Mutex::new(None),
                },
                attrs,
            );
            *core.allocation.lock().unwrap() = Some(allocation);
            let g = Gatt {
                core,
                budget: c.budget.clone(),
            };
            c.manage(Rc::new(g.clone()))?;
            g.core.state("connected");
            Ok(g)
        })?,
    )?;
    Ok(())
}
