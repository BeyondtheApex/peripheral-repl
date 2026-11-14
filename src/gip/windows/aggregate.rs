//! Isolated COM aggregation adapter. Windows' custom-controller outer requests
//! IAggregableInspectable (not published in the Windows SDK). ABI reconstructed
//! and verified against Nexus 2.5.8; no vendor code or DLL is shipped.
//! A different OS ABI must fail controller registration, never fabricate data.
#![allow(unsafe_op_in_unsafe_fn)]
use super::NativeDevice;
use std::{
    ffi::c_void,
    ptr,
    sync::{
        Weak,
        atomic::{AtomicPtr, AtomicU32, Ordering},
    },
};
use windows::{
    Gaming::Input::Custom::{
        GipMessageClass, IGameControllerInputSink, IGameControllerInputSink_Vtbl,
        IGipGameControllerInputSink, IGipGameControllerInputSink_Vtbl,
    },
    Win32::{
        Foundation::{E_NOINTERFACE, E_POINTER, E_UNEXPECTED, S_OK},
        System::Com::CoTaskMemAlloc,
    },
    core::{
        GUID, HRESULT, HSTRING, IInspectable, IInspectable_Vtbl, IUnknown, IUnknown_Vtbl, Interface,
    },
};
const AGGREGABLE: GUID = GUID::from_u128(0x06e58977_7684_4dc5_bad1_cda52a4aa06d);
const AGILE: GUID = GUID::from_u128(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90);
#[repr(C)]
struct AggregateVtable {
    base: IInspectable_Vtbl,
    set_outer: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT,
}
#[repr(C)]
struct Slot {
    vtable: *const c_void,
    owner: *mut Inner,
}
struct Inner {
    aggregate: Slot,
    input: Slot,
    gip: Slot,
    refs: AtomicU32,
    outer: AtomicPtr<c_void>,
    device: Weak<NativeDevice>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(d) = self.device.upgrade() {
            d.feed.disconnect();
        }
    }
}
unsafe fn owner<'a>(this: *mut c_void) -> &'a Inner {
    &*(*(this as *mut Slot)).owner
}
unsafe fn unknown<'a>(this: *mut c_void) -> &'a IUnknown_Vtbl {
    &**(this as *mut *const IUnknown_Vtbl)
}
unsafe extern "system" fn add(this: *mut c_void) -> u32 {
    owner(this).refs.fetch_add(1, Ordering::Relaxed) + 1
}
unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let count = owner(this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if count == 0 {
        drop(Box::from_raw((*(this as *mut Slot)).owner));
    }
    count
}
unsafe extern "system" fn delegate_add(this: *mut c_void) -> u32 {
    let outer = owner(this).outer.load(Ordering::Acquire);
    if outer.is_null() {
        add(this)
    } else {
        (unknown(outer).AddRef)(outer)
    }
}
unsafe extern "system" fn delegate_release(this: *mut c_void) -> u32 {
    let outer = owner(this).outer.load(Ordering::Acquire);
    if outer.is_null() {
        release(this)
    } else {
        (unknown(outer).Release)(outer)
    }
}
unsafe extern "system" fn query(
    this: *mut c_void,
    iid: *const GUID,
    result: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || result.is_null() {
        return E_POINTER;
    }
    *result = ptr::null_mut();
    let inner = owner(this);
    if [IUnknown::IID, IInspectable::IID, AGGREGABLE, AGILE].contains(&*iid) {
        *result = ptr::from_ref(&inner.aggregate).cast_mut().cast();
        add(*result);
        return S_OK;
    }
    if *iid == IGameControllerInputSink::IID {
        *result = ptr::from_ref(&inner.input).cast_mut().cast();
    }
    if *iid == IGipGameControllerInputSink::IID {
        *result = ptr::from_ref(&inner.gip).cast_mut().cast();
    }
    if result.read().is_null() {
        E_NOINTERFACE
    } else {
        delegate_add(*result);
        S_OK
    }
}
unsafe extern "system" fn delegate_query(
    this: *mut c_void,
    iid: *const GUID,
    result: *mut *mut c_void,
) -> HRESULT {
    let outer = owner(this).outer.load(Ordering::Acquire);
    if outer.is_null() {
        query(this, iid, result)
    } else {
        (unknown(outer).QueryInterface)(outer, iid, result)
    }
}
unsafe extern "system" fn iids(_: *mut c_void, count: *mut u32, result: *mut *mut GUID) -> HRESULT {
    if count.is_null() || result.is_null() {
        return E_POINTER;
    }
    *count = 0;
    *result = ptr::null_mut();
    let ids = CoTaskMemAlloc(2 * size_of::<GUID>()) as *mut GUID;
    if ids.is_null() {
        return HRESULT(0x8007000eu32 as i32);
    }
    ids.write(IGameControllerInputSink::IID);
    ids.add(1).write(IGipGameControllerInputSink::IID);
    *count = 2;
    *result = ids;
    S_OK
}
unsafe extern "system" fn name(_: *mut c_void, result: *mut *mut c_void) -> HRESULT {
    if result.is_null() {
        return E_POINTER;
    }
    let text = HSTRING::from("Peripheral.GipController");
    *result = std::mem::transmute::<HSTRING, *mut c_void>(text);
    S_OK
}
unsafe extern "system" fn trust(_: *mut c_void, result: *mut i32) -> HRESULT {
    if result.is_null() {
        E_POINTER
    } else {
        *result = 0;
        S_OK
    }
}
unsafe extern "system" fn set_outer(this: *mut c_void, outer: *mut c_void) -> HRESULT {
    if outer.is_null() || outer == this {
        return E_POINTER;
    }
    let inner = owner(this);
    if inner
        .outer
        .compare_exchange(ptr::null_mut(), outer, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return E_UNEXPECTED;
    }
    let mut identity = ptr::null_mut();
    let hr = (unknown(outer).QueryInterface)(outer, &IUnknown::IID, &mut identity);
    if hr.is_ok() && !identity.is_null() {
        super::super::diagnostics::record("aggregate_attached", serde_json::json!({}));
        if let Some(d) = inner.device.upgrade() {
            d.outer_identity.store(identity as usize, Ordering::Release);
        }
        (unknown(identity).Release)(identity);
    }
    hr
}
fn callback(work: impl FnOnce()) -> HRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .map(|_| S_OK)
        .unwrap_or(HRESULT(0x80004005u32 as i32))
}
unsafe extern "system" fn resumed(this: *mut c_void, timestamp: u64) -> HRESULT {
    callback(|| {
        if let Some(d) = owner(this).device.upgrade() {
            d.feed.resumed.store(true, Ordering::Release);
            d.feed
                .emit(serde_json::json!({"kind":"input_resumed","timestamp":timestamp}));
        }
    })
}
unsafe extern "system" fn suspended(this: *mut c_void, timestamp: u64) -> HRESULT {
    callback(|| {
        if let Some(d) = owner(this).device.upgrade() {
            d.feed.resumed.store(false, Ordering::Release);
            d.feed
                .emit(serde_json::json!({"kind":"input_suspended","timestamp":timestamp}));
        }
    })
}
unsafe extern "system" fn key(
    this: *mut c_void,
    timestamp: u64,
    code: u8,
    pressed: bool,
) -> HRESULT {
    callback(|| {
        if let Some(d) = owner(this).device.upgrade() {
            d.feed.emit(serde_json::json!({"kind":"key","timestamp":timestamp,"code":code,"pressed":pressed}));
        }
    })
}
unsafe extern "system" fn message(
    this: *mut c_void,
    timestamp: u64,
    class: GipMessageClass,
    id: u8,
    seq: u8,
    size: u32,
    bytes: *const u8,
) -> HRESULT {
    if size > super::super::policy::MAX_MESSAGE as u32 || (size != 0 && bytes.is_null()) {
        let _ = callback(|| {
            if let Some(d) = owner(this).device.upgrade() {
                d.feed.emit(serde_json::json!({"kind":"invalid_frame","actual_length":size,"message":"invalid GIP callback payload size or pointer"}));
            }
        });
        return HRESULT(0x80070057u32 as i32);
    }
    callback(|| {
        if let Some(d) = owner(this).device.upgrade() {
            let payload = if size == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(bytes, size as usize)
            };
            d.feed.message(timestamp, class.0, id, seq, payload);
        }
    })
}
const fn base(delegated: bool) -> IInspectable_Vtbl {
    IInspectable_Vtbl {
        base: IUnknown_Vtbl {
            QueryInterface: if delegated { delegate_query } else { query },
            AddRef: if delegated { delegate_add } else { add },
            Release: if delegated { delegate_release } else { release },
        },
        GetIids: iids,
        GetRuntimeClassName: name,
        GetTrustLevel: trust,
    }
}
static AGGREGATE_VTABLE: AggregateVtable = AggregateVtable {
    base: base(false),
    set_outer,
};
static INPUT_VTABLE: IGameControllerInputSink_Vtbl = IGameControllerInputSink_Vtbl {
    base__: base(true),
    OnInputResumed: resumed,
    OnInputSuspended: suspended,
};
static GIP_VTABLE: IGipGameControllerInputSink_Vtbl = IGipGameControllerInputSink_Vtbl {
    base__: base(true),
    OnKeyReceived: key,
    OnMessageReceived: message,
};
pub fn create(device: Weak<NativeDevice>) -> IInspectable {
    let slot = |vtable| Slot {
        vtable,
        owner: ptr::null_mut(),
    };
    let mut inner = Box::new(Inner {
        aggregate: slot(ptr::from_ref(&AGGREGATE_VTABLE).cast()),
        input: slot(ptr::from_ref(&INPUT_VTABLE).cast()),
        gip: slot(ptr::from_ref(&GIP_VTABLE).cast()),
        refs: AtomicU32::new(1),
        outer: AtomicPtr::new(ptr::null_mut()),
        device,
    });
    let owner = ptr::from_mut(inner.as_mut());
    inner.aggregate.owner = owner;
    inner.input.owner = owner;
    inner.gip.owner = owner;
    let result = ptr::from_mut(&mut inner.aggregate).cast();
    let _ = Box::into_raw(inner);
    unsafe { IInspectable::from_raw(result) }
}
