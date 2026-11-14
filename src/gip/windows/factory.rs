use super::*;
#[implement(ICustomGameControllerFactory)]
pub(super) struct Factory {
    pub(super) hub: Weak<Hub>,
}
impl ICustomGameControllerFactory_Impl for Factory_Impl {
    fn CreateGameController(
        &self,
        p: Ref<IGameControllerProvider>,
    ) -> windows::core::Result<IInspectable> {
        let result = guarded(|| self.create_controller(p));
        if let Err(e) = &result {
            super::super::diagnostics::record(
                "create_error",
                json!({"hresult":format!("0x{:08X}",e.code().0 as u32),"message":e.to_string()}),
            );
        }
        result
    }
    fn OnGameControllerAdded(&self, value: Ref<IGameController>) -> windows::core::Result<()> {
        guarded(|| self.added(value))
    }
    fn OnGameControllerRemoved(&self, value: Ref<IGameController>) -> windows::core::Result<()> {
        guarded(|| self.removed(value))
    }
}
fn guarded<T>(work: impl FnOnce() -> windows::core::Result<T>) -> windows::core::Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|_| {
        Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_FAIL,
        ))
    })
}
impl Factory_Impl {
    fn create_controller(
        &self,
        p: Ref<IGameControllerProvider>,
    ) -> windows::core::Result<IInspectable> {
        let p = p.ok()?;
        let hub = self
            .hub
            .upgrade()
            .ok_or(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_FAIL,
            ))?;
        let (vendor, product) = (p.HardwareVendorId()?, p.HardwareProductId()?);
        super::super::diagnostics::record(
            "create",
            json!({"vendor_id":vendor,"product_id":product}),
        );
        if !hub
            .registry
            .lock()
            .unwrap()
            .targets
            .contains(&(vendor, product))
        {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_NOINTERFACE,
            ));
        }
        let provider = p.cast::<GipGameControllerProvider>()?;
        let generation = uuid::Uuid::new_v4().to_string();
        let path = format!("gip:{generation}");
        let firmware = p
            .FirmwareVersionInfo()
            .ok()
            .map(|v| format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision));
        let device = Arc::new(NativeDevice {
            info: Mutex::new(
                json!({"backend":"gip","path":path,"generation":generation,"vendor_id":vendor,"product_id":product,"firmware_version":firmware,"identity":"connection_generation"}),
            ),
            feed: Arc::new(Feed::default()),
            outer_identity: AtomicUsize::new(0),
            provider,
            controller: Mutex::new(None),
            identity_attempts: AtomicUsize::new(0),
        });
        {
            let mut registry = hub.registry.lock().unwrap();
            if registry.devices.len() >= 64 {
                return Err(windows::core::Error::from_hresult(
                    windows::Win32::Foundation::E_OUTOFMEMORY,
                ));
            }
            registry.devices.insert(path, device.clone());
        }
        Ok(aggregate::create(Arc::downgrade(&device)))
    }
    fn added(&self, value: Ref<IGameController>) -> windows::core::Result<()> {
        let value = value.ok()?;
        let identity = value.cast::<IUnknown>()?.as_raw() as usize;
        if let Some(hub) = self.hub.upgrade() {
            let device = hub
                .registry
                .lock()
                .unwrap()
                .devices
                .values()
                .find(|d| d.outer_identity.load(Ordering::Acquire) == identity)
                .cloned();
            if let Some(d) = device {
                super::super::diagnostics::record("added", d.info.lock().unwrap().clone());
                *d.controller.lock().unwrap() = Some(AgileReference::new(value)?);
                if let Ok(raw) = RawGameController::FromGameController(value) {
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
                    let mut info = d.info.lock().unwrap();
                    info["name"] = name;
                    info["os_id"] = id;
                }
                d.feed.emit(json!({"kind":"connected"}));
            } else {
                super::super::diagnostics::record(
                    "added_unmatched",
                    json!({"message":"custom controller identity was not associated by SetOuter"}),
                );
            }
        }
        Ok(())
    }
    fn removed(&self, value: Ref<IGameController>) -> windows::core::Result<()> {
        let identity = value.ok()?.cast::<IUnknown>()?.as_raw() as usize;
        if let Some(hub) = self.hub.upgrade() {
            let mut registry = hub.registry.lock().unwrap();
            let keys: Vec<_> = registry
                .devices
                .iter()
                .filter(|(_, d)| d.outer_identity.load(Ordering::Acquire) == identity)
                .map(|(k, _)| k.clone())
                .collect();
            let removed: Vec<_> = keys
                .iter()
                .filter_map(|key| registry.devices.remove(key))
                .collect();
            drop(registry);
            for d in removed {
                super::super::diagnostics::record("removed", d.info.lock().unwrap().clone());
                d.feed.disconnect();
                let controller = d.controller.lock().unwrap().take();
                drop(controller);
            }
        }
        Ok(())
    }
}
