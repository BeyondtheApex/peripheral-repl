mod factory;
use factory::Factory;
mod aggregate;
pub(super) mod background;
use super::{
    feed::{Feed, Filter},
    policy::MessageClass,
};
use crate::error::Fault;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use windows::{
    Gaming::Input::{
        Custom::{
            GameControllerFactoryManager, GipGameControllerProvider, GipMessageClass,
            ICustomGameControllerFactory, ICustomGameControllerFactory_Impl,
            IGameControllerProvider,
        },
        IGameController, IGameControllerBatteryInfo, RawGameController,
    },
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::{AgileReference, GUID, IInspectable, IUnknown, Interface, Ref, implement},
};

pub(super) fn error(op: &str, e: windows::core::Error) -> Fault {
    let code = match e.code().0 as u32 {
        0x80070005 => "access_denied",
        0x80004002 | 0x80040154 => "unsupported",
        0x8007048f | 0x80010108 | 0x800706ba => "disconnected",
        _ => "backend",
    };
    Fault::new(
        code,
        op,
        format!("{e} (HRESULT 0x{:08X})", e.code().0 as u32),
    )
}
struct Apartment(bool);
impl Apartment {
    fn new() -> Result<Self, Fault> {
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Ok(Self(true)),
            Err(e) if e.code().0 as u32 == 0x80010106 => Ok(Self(false)),
            Err(e) => Err(error("gip.initialize", e)),
        }
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                RoUninitialize();
            }
        }
    }
}

pub struct NativeDevice {
    pub info: Mutex<Value>,
    pub feed: Arc<Feed>,
    pub outer_identity: AtomicUsize,
    provider: GipGameControllerProvider,
    controller: Mutex<Option<AgileReference<IGameController>>>,
    identity_attempts: AtomicUsize,
}
impl NativeDevice {
    fn refresh_identity(&self) {
        if self
            .info
            .lock()
            .unwrap()
            .get("os_id")
            .is_some_and(|v| v.as_str().is_some_and(|v| !v.is_empty()))
        {
            return;
        }
        // FromGameController may be unavailable while OnAdded is still running.
        // Retry only a few times on discovery workers, never on the callback thread.
        if self.identity_attempts.fetch_add(1, Ordering::Relaxed) >= 3 {
            return;
        }
        let reference = self.controller.lock().unwrap().clone();
        let Some(reference) = reference else {
            return;
        };
        let Ok(controller) = reference.resolve() else {
            return;
        };
        let Ok(raw) = RawGameController::FromGameController(&controller) else {
            return;
        };
        let name = raw
            .DisplayName()
            .ok()
            .map(|v| Value::String(v.to_string()))
            .unwrap_or(Value::Null);
        let id = raw
            .NonRoamableId()
            .ok()
            .map(|v| Value::String(v.to_string().trim_end_matches('\0').to_owned()))
            .unwrap_or(Value::Null);
        let mut info = self.info.lock().unwrap();
        info["name"] = name;
        info["os_id"] = id;
    }
    pub fn send(&self, class: MessageClass, id: u8, payload: &[u8]) -> Result<(), Fault> {
        let _apartment = Apartment::new()?;
        if !self.feed.connected.load(Ordering::Acquire) {
            return Err(Fault::new(
                "disconnected",
                "gip.send",
                "device disconnected",
            ));
        }
        self.provider
            .SendMessage(GipMessageClass(class.native()), id, payload)
            .map_err(|e| error("gip.send", e))
    }
    pub fn battery_info(&self) -> Result<Value, Fault> {
        let _apartment = Apartment::new()?;
        let reference = self.controller.lock().unwrap().clone().ok_or_else(|| {
            Fault::new("unavailable", "gip.battery_info", "controller is not ready")
        })?;
        let controller = reference
            .resolve()
            .map_err(|e| error("gip.battery_info", e))?;
        let battery = controller
            .cast::<IGameControllerBatteryInfo>()
            .map_err(|e| error("gip.battery_info", e))?;
        let report = match battery.TryGetBatteryReport() {
            Ok(v) => v,
            Err(e) if e.code().0 as u32 == 0x80004003 => {
                return Ok(json!({"available":false,"source":"windows_gaming_input"}));
            }
            Err(e) => return Err(error("gip.battery_info", e)),
        };
        let status = report.Status().map_err(|e| error("gip.battery_info", e))?.0;
        Ok(
            json!({"available":true,"source":"windows_gaming_input","precision":"driver_defined","status_code":status,
            "status":match status {0=>"not_present",1=>"discharging",2=>"idle",3=>"charging",_=>"unknown"},
            "remaining_capacity_mwh":report.RemainingCapacityInMilliwattHours().ok().and_then(|v|v.Value().ok()),
            "full_charge_capacity_mwh":report.FullChargeCapacityInMilliwattHours().ok().and_then(|v|v.Value().ok()),
            "design_capacity_mwh":report.DesignCapacityInMilliwattHours().ok().and_then(|v|v.Value().ok()),
            "charge_rate_mw":report.ChargeRateInMilliwatts().ok().and_then(|v|v.Value().ok()),
            "warning":"Capacity values may be synthesized from battery levels; no exact percentage is inferred."}),
        )
    }
}
struct Registry {
    devices: BTreeMap<String, Arc<NativeDevice>>,
    targets: BTreeSet<(u16, u16)>,
    hardware: BTreeSet<(u16, u16)>,
    interfaces: BTreeSet<String>,
}
struct Hub {
    registry: Mutex<Registry>,
    factory: OnceLock<AgileReference<ICustomGameControllerFactory>>,
    registration: Mutex<()>,
}
fn hub() -> Result<Arc<Hub>, Fault> {
    static HUB: OnceLock<Result<Arc<Hub>, Fault>> = OnceLock::new();
    HUB.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("gip-winrt".into())
            .spawn(move || {
                let init = || -> Result<_, Fault> {
                    let apartment = Apartment::new()?;
                    let hub = Arc::new(Hub {
                        registry: Mutex::new(Registry {
                            devices: BTreeMap::new(),
                            targets: BTreeSet::new(),
                            hardware: BTreeSet::new(),
                            interfaces: BTreeSet::new(),
                        }),
                        factory: OnceLock::new(),
                        registration: Mutex::new(()),
                    });
                    let factory: ICustomGameControllerFactory = Factory {
                        hub: Arc::downgrade(&hub),
                    }
                    .into();
                    let reference =
                        AgileReference::new(&factory).map_err(|e| error("gip.initialize", e))?;
                    let _ = hub.factory.set(reference);
                    Ok((hub, apartment, factory))
                };
                match init() {
                    Ok((hub, _apartment, _factory)) => {
                        let _ = tx.send(Ok(hub));
                        loop {
                            std::thread::park();
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| Fault::new("backend", "gip.initialize", e))?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|e| Fault::new("timeout", "gip.initialize", e))?
    })
    .clone()
}
pub fn discover(filter: &Filter) -> Result<(), Fault> {
    let _apartment = Apartment::new()?;
    let hub = hub()?;
    let controllers = RawGameController::RawGameControllers();
    let _registration = hub.registration.lock().unwrap();
    let factory = hub
        .factory
        .get()
        .unwrap()
        .resolve()
        .map_err(|e| error("gip.list", e))?;
    let mut targets = BTreeSet::new();
    if let (Some(vendor), Some(product)) = (filter.vendor_id, filter.product_id) {
        targets.insert((vendor, product));
    } else if let Ok(controllers) = controllers {
        for raw in controllers {
            if let (Ok(v), Ok(p)) = (raw.HardwareVendorId(), raw.HardwareProductId())
                && filter.matches(&json!({"vendor_id":v,"product_id":p}))
            {
                targets.insert((v, p));
            }
        }
    }
    for (v, p) in targets {
        {
            let mut r = hub.registry.lock().unwrap();
            if r.hardware.contains(&(v, p)) {
                continue;
            }
            if r.targets.len() >= 64 {
                return Err(Fault::new(
                    "resource_limit",
                    "gip.list",
                    "maximum 64 hardware registrations per process",
                ));
            }
            r.targets.insert((v, p));
        }
        GameControllerFactoryManager::RegisterCustomFactoryForHardwareId(&factory, v, p)
            .map_err(|e| error("gip.list", e))?;
        super::diagnostics::record("hardware_registered", json!({"vendor_id":v,"product_id":p}));
        hub.registry.lock().unwrap().hardware.insert((v, p));
    }
    for text in &filter.interface_guids {
        let guid = uuid::Uuid::parse_str(text)
            .map_err(|e| Fault::new("invalid_argument", "gip.list", e))?;
        let key = guid.to_string();
        {
            let r = hub.registry.lock().unwrap();
            if r.interfaces.contains(&key) {
                continue;
            }
            if r.interfaces.len() >= 32 {
                return Err(Fault::new(
                    "resource_limit",
                    "gip.list",
                    "maximum 32 interface registrations per process",
                ));
            }
        }
        GameControllerFactoryManager::RegisterCustomFactoryForGipInterface(
            &factory,
            GUID::from_u128(guid.as_u128()),
        )
        .map_err(|e| error("gip.list", e))?;
        super::diagnostics::record("interface_registered", json!({"guid":key}));
        hub.registry.lock().unwrap().interfaces.insert(key);
    }
    Ok(())
}
pub fn snapshot(filter: &Filter) -> Result<Vec<Value>, Fault> {
    let _apartment = Apartment::new()?;
    let hub = hub()?;
    let mut registry = hub.registry.lock().unwrap();
    // Failed aggregation releases its inner without OnRemoved. Retire those entries
    // outside the registry lock, where COM releases may re-enter our callbacks.
    let keys: Vec<_> = registry
        .devices
        .iter()
        .filter(|(_, d)| !d.feed.connected.load(Ordering::Acquire))
        .map(|(k, _)| k.clone())
        .collect();
    let retired: Vec<_> = keys
        .iter()
        .filter_map(|k| registry.devices.remove(k))
        .collect();
    let devices: Vec<_> = registry.devices.values().cloned().collect();
    drop(registry);
    drop(retired);
    Ok(devices
        .iter()
        .filter_map(|d| {
            if d.controller.lock().unwrap().is_none() || !d.feed.connected.load(Ordering::Acquire) {
                return None;
            }
            d.refresh_identity();
            let mut info = d.info.lock().unwrap().clone();
            info["connection"] = d.feed.inspect();
            filter.matches(&info).then_some(info)
        })
        .collect())
}
pub fn open(reference: &Value) -> Result<Arc<NativeDevice>, Fault> {
    let path = reference["path"]
        .as_str()
        .ok_or_else(|| Fault::new("invalid_argument", "gip.open", "path required"))?;
    let hub = hub()?;
    let d = hub
        .registry
        .lock()
        .unwrap()
        .devices
        .get(path)
        .cloned()
        .ok_or_else(|| {
            Fault::new(
                "not_found",
                "gip.open",
                "device absent; rediscover after reconnect",
            )
        })?;
    if d.controller.lock().unwrap().is_none() || !d.feed.connected.load(Ordering::Acquire) {
        return Err(Fault::new(
            "disconnected",
            "gip.open",
            "controller is not ready; rediscover before opening",
        ));
    }
    let info = d.info.lock().unwrap();
    if ["generation", "vendor_id", "product_id"]
        .iter()
        .any(|k| reference.get(k).is_none() || reference[*k] != info[*k])
    {
        return Err(Fault::new(
            "identity_changed",
            "gip.open",
            "reference no longer matches this device generation",
        ));
    }
    drop(info);
    Ok(d)
}
