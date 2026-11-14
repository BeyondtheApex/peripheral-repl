//! Minimal, version-pinned GameInput v4 ABI from Microsoft's GameInput.h 4.0.306.
//! Only EnableBackgroundInput is called. No device reports or game input are sent.
//! The IID gates compatibility; older runtimes fail explicitly without a fallback ABI.
use crate::error::Fault;
use std::{ffi::c_void, sync::Mutex};
use windows::{
    Win32::{
        Foundation::{FreeLibrary, HMODULE},
        System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
    },
    core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, w},
};

const IID_V4: GUID = GUID::from_u128(0xf2daae4c_4d21_4250_8953_4f791e6279ab);
#[repr(C)]
struct Vtable {
    base: IUnknown_Vtbl,
    // GetCurrentTimestamp through FindDeviceFromPlatformString, in SDK order.
    unused: [usize; 10],
    enable: unsafe extern "system" fn(*mut c_void, bool),
}
struct Input {
    object: Option<IUnknown>,
    library: HMODULE,
}
// IGameInput is free-threaded. All access here is additionally serialized by STATE.
unsafe impl Send for Input {}
impl Input {
    fn new() -> Result<Self, Fault> {
        unsafe {
            let library = LoadLibraryExW(
                w!("GameInputRedist.dll"),
                None,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .map_err(|e| super::error("gip.background", e))?;
            let mut value = Self {
                object: None,
                library,
            };
            let Some(entry) = GetProcAddress(library, windows::core::s!("GameInputInitialize"))
            else {
                return Err(Fault::new(
                    "unsupported",
                    "gip.background",
                    "GameInput v4 runtime is required",
                ));
            };
            let initialize: unsafe extern "system" fn(*const GUID, *mut *mut c_void) -> HRESULT =
                std::mem::transmute(entry);
            let mut raw = std::ptr::null_mut();
            initialize(&IID_V4, &mut raw)
                .ok()
                .map_err(|e| super::error("gip.background", e))?;
            if raw.is_null() {
                return Err(Fault::new(
                    "backend",
                    "gip.background",
                    "GameInput returned a null interface",
                ));
            }
            value.object = Some(IUnknown::from_raw(raw));
            value.enable(true);
            Ok(value)
        }
    }
    fn enable(&self, on: bool) {
        if let Some(object) = &self.object {
            unsafe {
                let raw = object.as_raw();
                let table = &**(raw as *const *const Vtable);
                (table.enable)(raw, on);
            }
        }
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        self.enable(false);
        drop(self.object.take());
        unsafe {
            let _ = FreeLibrary(self.library);
        }
    }
}
static STATE: Mutex<(usize, Option<Input>)> = Mutex::new((0, None));
pub fn acquire() -> Result<(), Fault> {
    let _apartment = super::Apartment::new()?;
    let _hub = super::hub()?;
    let mut state = STATE.lock().unwrap();
    if state.0 == 0 {
        state.1 = Some(Input::new()?);
    }
    state.0 += 1;
    super::super::diagnostics::record("background_acquired", serde_json::json!({"users":state.0}));
    Ok(())
}
pub fn release() {
    let mut state = STATE.lock().unwrap();
    if state.0 > 0 {
        state.0 -= 1;
        if state.0 == 0 {
            state.1.take();
        }
    }
}
