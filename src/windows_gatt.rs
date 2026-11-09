//! Windows GATT uses attribute handles, so duplicate UUIDs remain distinct.
use crate::error::Fault;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use windows::{
    Devices::{
        Bluetooth::{
            BluetoothAddressType, BluetoothCacheMode, BluetoothLEDevice, GenericAttributeProfile::*,
        },
        Enumeration::DevicePairingProtectionLevel,
    },
    Foundation::TypedEventHandler,
    Storage::Streams::{DataReader, DataWriter, IBuffer},
};
async fn wait<T: windows::core::RuntimeType + 'static>(
    op: windows_future::IAsyncOperation<T>,
) -> Result<T, Fault> {
    struct Cancel<T: windows::core::RuntimeType + 'static>(
        Option<windows_future::IAsyncOperation<T>>,
    );
    impl<T: windows::core::RuntimeType + 'static> Drop for Cancel<T> {
        fn drop(&mut self) {
            if let Some(op) = self.0.take() {
                let _ = op.Cancel();
            }
        }
    }
    let mut guard = Cancel(Some(op.clone()));
    let result = op.await.map_err(err);
    guard.0.take();
    result
}
fn err(e: windows::core::Error) -> Fault {
    Fault::new("backend", "winrt.gatt", e)
}
fn check(s: GattCommunicationStatus) -> Result<(), Fault> {
    if s == GattCommunicationStatus::Success {
        Ok(())
    } else {
        Err(Fault::new(
            if s == GattCommunicationStatus::AccessDenied {
                "access_denied"
            } else if s == GattCommunicationStatus::Unreachable {
                "disconnected"
            } else {
                "protocol_error"
            },
            "winrt.gatt",
            format!("status {}", s.0),
        ))
    }
}
fn cache(s: &str) -> BluetoothCacheMode {
    if s == "cached" {
        BluetoothCacheMode::Cached
    } else {
        BluetoothCacheMode::Uncached
    }
}
fn bytes(buffer: IBuffer) -> Result<Vec<u8>, Fault> {
    let r = DataReader::FromBuffer(&buffer).map_err(err)?;
    let n = r.UnconsumedBufferLength().map_err(err)? as usize;
    if n > 512 {
        return Err(Fault::new("resource_limit", "gatt.read", "over 512 bytes"));
    }
    let mut v = vec![0; n];
    r.ReadBytes(&mut v).map_err(err)?;
    Ok(v)
}
fn buffer(data: &[u8]) -> Result<IBuffer, Fault> {
    let w = DataWriter::new().map_err(err)?;
    w.WriteBytes(data).map_err(err)?;
    w.DetachBuffer().map_err(err)
}
struct State {
    device: BluetoothLEDevice,
    services: Vec<GattDeviceService>,
    session: Option<GattSession>,
    chars: BTreeMap<u16, GattCharacteristic>,
    descriptors: BTreeMap<u16, GattDescriptor>,
    tokens: BTreeMap<u16, i64>,
    connection_token: Option<i64>,
}
impl Drop for State {
    fn drop(&mut self) {
        for (h, t) in &self.tokens {
            if let Some(c) = self.chars.get(h) {
                let _ = c.RemoveValueChanged(*t);
            }
        }
        if let Some(t) = self.connection_token {
            let _ = self.device.RemoveConnectionStatusChanged(t);
        }
        for s in &self.services {
            let _ = s.Close();
        }
        if let Some(s) = &self.session {
            let _ = s.Close();
        }
        let _ = self.device.Close();
    }
}
pub struct Connection {
    address: u64,
    random: bool,
    state: Mutex<Option<State>>,
}
impl Connection {
    pub fn new(address: u64, random: bool) -> Self {
        Self {
            address,
            random,
            state: Mutex::new(None),
        }
    }
    pub fn close(&self) {
        let state = self.state.lock().unwrap().take();
        drop(state);
    }
    pub async fn connect(
        &self,
        mode: &str,
        on_disconnect: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Vec<Value>, Fault> {
        self.close();
        let d = wait(
            BluetoothLEDevice::FromBluetoothAddressWithBluetoothAddressTypeAsync(
                self.address,
                if self.random {
                    BluetoothAddressType::Random
                } else {
                    BluetoothAddressType::Public
                },
            )
            .map_err(err)?,
        )
        .await?;
        let mut state = State {
            device: d.clone(),
            services: vec![],
            session: None,
            chars: BTreeMap::new(),
            descriptors: BTreeMap::new(),
            tokens: BTreeMap::new(),
            connection_token: None,
        };
        let result = wait(
            d.GetGattServicesWithCacheModeAsync(cache(mode))
                .map_err(err)?,
        )
        .await?;
        check(result.Status().map_err(err)?)?;
        let mut out = vec![];
        let services: Vec<_> = result.Services().map_err(err)?.into_iter().collect();
        for service in services {
            if state.services.len() >= 64 {
                return Err(Fault::new(
                    "resource_limit",
                    "gatt.discover",
                    "over 64 services",
                ));
            }
            state.services.push(service.clone());
            if state.session.is_none() {
                state.session = Some(service.Session().map_err(err)?);
            }
            let found = wait(
                service
                    .GetCharacteristicsWithCacheModeAsync(cache(mode))
                    .map_err(err)?,
            )
            .await?;
            check(found.Status().map_err(err)?)?;
            let characteristics: Vec<_> =
                found.Characteristics().map_err(err)?.into_iter().collect();
            for c in characteristics {
                if out.len() >= 256 {
                    return Err(Fault::new(
                        "resource_limit",
                        "gatt.discover",
                        "over 256 characteristics",
                    ));
                }
                let handle = c.AttributeHandle().map_err(err)?;
                let dr = wait(
                    c.GetDescriptorsWithCacheModeAsync(cache(mode))
                        .map_err(err)?,
                )
                .await?;
                check(dr.Status().map_err(err)?)?;
                let mut ds = vec![];
                for desc in dr.Descriptors().map_err(err)? {
                    if ds.len() >= 32 {
                        return Err(Fault::new(
                            "resource_limit",
                            "gatt.discover",
                            "over 32 descriptors per characteristic",
                        ));
                    }
                    let dh = desc.AttributeHandle().map_err(err)?;
                    ds.push(json!({"uuid":uuid::Uuid::from_u128(desc.Uuid().map_err(err)?.to_u128()).to_string(),"handle":dh}));
                    state.descriptors.insert(dh, desc);
                }
                out.push(json!({"handle":handle,"service_handle":service.AttributeHandle().map_err(err)?,"service":uuid::Uuid::from_u128(service.Uuid().map_err(err)?.to_u128()).to_string(),"uuid":uuid::Uuid::from_u128(c.Uuid().map_err(err)?.to_u128()).to_string(),"properties":c.CharacteristicProperties().map_err(err)?.0,"descriptors":ds}));
                state.chars.insert(handle, c);
            }
        }
        let token = d
            .ConnectionStatusChanged(&TypedEventHandler::new(
                move |sender: windows::core::Ref<BluetoothLEDevice>, _| {
                    if let Some(d) = sender.as_ref()
                        && d.ConnectionStatus()?
                            == windows::Devices::Bluetooth::BluetoothConnectionStatus::Disconnected
                    {
                        on_disconnect();
                    }
                    Ok(())
                },
            ))
            .map_err(err)?;
        state.connection_token = Some(token);
        *self.state.lock().unwrap() = Some(state);
        Ok(out)
    }
    fn characteristic(&self, h: u16) -> Result<GattCharacteristic, Fault> {
        self.state
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.chars.get(&h))
            .cloned()
            .ok_or_else(|| Fault::new("disconnected", "gatt.characteristic", "handle unavailable"))
    }
    fn descriptor(&self, h: u16) -> Result<GattDescriptor, Fault> {
        self.state
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.descriptors.get(&h))
            .cloned()
            .ok_or_else(|| Fault::new("disconnected", "gatt.descriptor", "handle unavailable"))
    }
    pub async fn read(&self, h: u16, mode: &str) -> Result<Vec<u8>, Fault> {
        let r = wait(
            self.characteristic(h)?
                .ReadValueWithCacheModeAsync(cache(mode))
                .map_err(err)?,
        )
        .await?;
        check(r.Status().map_err(err)?)?;
        bytes(r.Value().map_err(err)?)
    }
    pub async fn read_descriptor(&self, h: u16, mode: &str) -> Result<Vec<u8>, Fault> {
        let r = wait(
            self.descriptor(h)?
                .ReadValueWithCacheModeAsync(cache(mode))
                .map_err(err)?,
        )
        .await?;
        check(r.Status().map_err(err)?)?;
        bytes(r.Value().map_err(err)?)
    }
    pub async fn write(&self, h: u16, data: &[u8], response: bool) -> Result<(), Fault> {
        if !response && data.len() > self.mtu().unwrap_or(23).saturating_sub(3) as usize {
            return Err(Fault::new(
                "invalid_argument",
                "gatt.write",
                "without-response value exceeds current MTU-3",
            ));
        }
        let pending = {
            let b = buffer(data)?;
            self.characteristic(h)?
                .WriteValueWithResultAndOptionAsync(
                    &b,
                    if response {
                        GattWriteOption::WriteWithResponse
                    } else {
                        GattWriteOption::WriteWithoutResponse
                    },
                )
                .map_err(err)?
        };
        let r = wait(pending).await?;
        check(r.Status().map_err(err)?)
    }
    pub async fn write_descriptor(&self, h: u16, data: &[u8]) -> Result<(), Fault> {
        let pending = {
            let b = buffer(data)?;
            self.descriptor(h)?
                .WriteValueWithResultAsync(&b)
                .map_err(err)?
        };
        let r = wait(pending).await?;
        check(r.Status().map_err(err)?)
    }
    pub async fn reliable(&self, values: Vec<(u16, Vec<u8>)>) -> Result<(), Fault> {
        let tx = GattReliableWriteTransaction::new().map_err(err)?;
        for (h, data) in values {
            tx.WriteValue(&self.characteristic(h)?, &buffer(&data)?)
                .map_err(err)?;
        }
        let r = wait(tx.CommitWithResultAsync().map_err(err)?).await?;
        check(r.Status().map_err(err)?)
    }
    pub async fn subscribe(
        &self,
        h: u16,
        deliver: Arc<dyn Fn(Vec<u8>) + Send + Sync>,
    ) -> Result<(), Fault> {
        let c = self.characteristic(h)?;
        let token = c
            .ValueChanged(&TypedEventHandler::new(
                move |_, args: windows::core::Ref<GattValueChangedEventArgs>| {
                    if let Some(a) = args.as_ref()
                        && let Ok(v) = a.CharacteristicValue().map_err(err).and_then(bytes)
                    {
                        deliver(v);
                    }
                    Ok(())
                },
            ))
            .map_err(err)?;
        let kind = if c.CharacteristicProperties().map_err(err)?.0 & 16 != 0 {
            GattClientCharacteristicConfigurationDescriptorValue::Notify
        } else {
            GattClientCharacteristicConfigurationDescriptorValue::Indicate
        };
        if let Some(s) = self.state.lock().unwrap().as_mut() {
            s.tokens.insert(h, token);
        }
        let result = async {
            check(
                wait(
                    c.WriteClientCharacteristicConfigurationDescriptorAsync(kind)
                        .map_err(err)?,
                )
                .await?,
            )
        }
        .await;
        if let Err(e) = result {
            let _ = c.RemoveValueChanged(token);
            if let Some(s) = self.state.lock().unwrap().as_mut() {
                s.tokens.remove(&h);
            }
            return Err(e);
        }
        if let Some(s) = self.state.lock().unwrap().as_mut() {
            s.tokens.insert(h, token);
        }
        Ok(())
    }
    pub async fn unsubscribe(&self, h: u16) -> Result<(), Fault> {
        let c = self.characteristic(h)?;
        check(
            wait(
                c.WriteClientCharacteristicConfigurationDescriptorAsync(
                    GattClientCharacteristicConfigurationDescriptorValue::None,
                )
                .map_err(err)?,
            )
            .await?,
        )?;
        if let Some(t) = self
            .state
            .lock()
            .unwrap()
            .as_mut()
            .and_then(|s| s.tokens.remove(&h))
        {
            c.RemoveValueChanged(t).map_err(err)?;
        }
        Ok(())
    }
    pub fn mtu(&self) -> Option<u16> {
        self.state
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.session.as_ref())
            .and_then(|s| s.MaxPduSize().ok())
    }
}
pub async fn pairing(address: u64, random: bool, protection: Option<&str>) -> Result<Value, Fault> {
    let d = wait(
        BluetoothLEDevice::FromBluetoothAddressWithBluetoothAddressTypeAsync(
            address,
            if random {
                BluetoothAddressType::Random
            } else {
                BluetoothAddressType::Public
            },
        )
        .map_err(err)?,
    )
    .await?;
    struct Close(BluetoothLEDevice);
    impl Drop for Close {
        fn drop(&mut self) {
            let _ = self.0.Close();
        }
    }
    let _close = Close(d.clone());
    let result=async{
        let p=d.DeviceInformation().map_err(err)?.Pairing().map_err(err)?;
        if protection==Some("unpair") {let result=wait(p.UnpairAsync().map_err(err)?).await?;return Ok(json!({"status":result.Status().map_err(err)?.0,"paired":p.IsPaired().map_err(err)?}));}
        if let Some(level)=protection {
            let level=match level{"none"=>DevicePairingProtectionLevel::None,"encryption"=>DevicePairingProtectionLevel::Encryption,"encryption_and_authentication"=>DevicePairingProtectionLevel::EncryptionAndAuthentication,_=>return Err(Fault::new("invalid_argument","ble.pair","invalid protection level"))};
            let result=wait(p.PairWithProtectionLevelAsync(level).map_err(err)?).await?;
            Ok(json!({"status":result.Status().map_err(err)?.0,"paired":p.IsPaired().map_err(err)?,"protection":result.ProtectionLevelUsed().map_err(err)?.0}))
        }else{Ok(json!({"paired":p.IsPaired().map_err(err)?,"can_pair":p.CanPair().map_err(err)?}))}
    }.await;
    let _ = d.Close();
    result
}

/// OS container identity is metadata only; never opens a HID communication channel.
pub fn container_id(path: &str) -> Option<String> {
    use windows::{
        Devices::Enumeration::{DeviceInformation, DeviceInformationKind},
        Foundation::IPropertyValue,
        core::{HSTRING, Interface},
    };
    let key = HSTRING::from("System.Devices.ContainerId");
    let properties: windows_collections::IIterable<HSTRING> = vec![key.clone()].into();
    let info = DeviceInformation::CreateFromIdAsyncWithKindAndAdditionalProperties(
        &HSTRING::from(path),
        &properties,
        DeviceInformationKind::DeviceInterface,
    )
    .ok()?
    .join()
    .ok()?;
    let id = info
        .Properties()
        .ok()?
        .Lookup(&key)
        .ok()?
        .cast::<IPropertyValue>()
        .ok()?
        .GetGuid()
        .ok()?;
    if id.to_u128() == 0 {
        None
    } else {
        Some(uuid::Uuid::from_u128(id.to_u128()).to_string())
    }
}
