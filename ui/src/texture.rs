//! Two-slot cross-process D3D11 bridge. No CPU pixel transfer in the frame path.
use anyhow::{Context, Result};
use gpui_windows::frame_bridge::Frame;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            DirectComposition::*,
            Dxgi::{Common::*, *},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    },
    core::{HSTRING, Interface, w},
};

mod consumer;
pub use consumer::run_consumer;
pub const FRAME_MESSAGE: u32 = WM_APP + 77;
struct Slot {
    texture: ID3D11Texture2D,
    mutex: IDXGIKeyedMutex,
    handle: HANDLE,
    name: String,
}
impl Drop for Slot {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}
#[derive(Default)]
pub struct Stats {
    pub published: u64,
    pub dropped: u64,
    pub gpu_copy_bytes: u64,
    pub error: Option<String>,
}
pub struct Exporter {
    slots: Vec<Slot>,
    size: (u32, u32),
    device_identity: usize,
    generation: u64,
    next: usize,
    consumer: usize,
    producer: usize,
    dir: PathBuf,
    stats: Rc<RefCell<Stats>>,
}
impl Exporter {
    pub fn new(consumer: usize, producer: usize, dir: PathBuf, stats: Rc<RefCell<Stats>>) -> Self {
        Self {
            slots: Vec::new(),
            size: (0, 0),
            device_identity: 0,
            generation: 0,
            next: 0,
            consumer,
            producer,
            dir,
            stats,
        }
    }
    fn configure(&mut self, frame: &Frame<'_>, desc: D3D11_TEXTURE2D_DESC) -> Result<()> {
        self.slots.clear();
        self.generation += 1;
        let desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX)
                .0 as u32,
            ..desc
        };
        for _ in 0..2 {
            let mut texture = None;
            unsafe {
                frame
                    .device
                    .CreateTexture2D(&desc, None, Some(&mut texture))?;
            }
            let texture = texture.context("shared texture")?;
            let name = format!("Local\\PeripheralFrame_{}", uuid::Uuid::new_v4());
            let resource: IDXGIResource1 = texture.cast()?;
            let handle = unsafe {
                resource.CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    &HSTRING::from(&name),
                )?
            };
            let mutex = texture.cast()?;
            self.slots.push(Slot {
                texture,
                mutex,
                handle,
                name,
            });
        }
        let dxgi: IDXGIDevice = frame.device.cast()?;
        let adapter = unsafe { dxgi.GetAdapter()? };
        let info = unsafe { adapter.GetDesc()? };
        let metadata = json!({"generation":self.generation,"width":desc.Width,"height":desc.Height,"format":desc.Format.0,"adapter_low":info.AdapterLuid.LowPart,"adapter_high":info.AdapterLuid.HighPart,"producer_hwnd":self.producer,"names":self.slots.iter().map(|s|s.name.clone()).collect::<Vec<_>>()});
        std::fs::write(self.dir.join("frame.tmp"), serde_json::to_vec(&metadata)?)?;
        std::fs::rename(self.dir.join("frame.tmp"), self.dir.join("frame.json"))?;
        self.size = (desc.Width, desc.Height);
        self.device_identity = Interface::as_raw(frame.device) as usize;
        Ok(())
    }
    pub fn publish(&mut self, frame: Frame<'_>) {
        if let Err(e) = self.publish_inner(frame) {
            self.stats.borrow_mut().error = Some(format!("{e:#}"));
        }
    }
    fn publish_inner(&mut self, frame: Frame<'_>) -> Result<()> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            frame.texture.GetDesc(&mut desc);
        }
        if self.size != (desc.Width, desc.Height)
            || self.device_identity != Interface::as_raw(frame.device) as usize
        {
            self.configure(&frame, desc)?;
        }
        let mut chosen = None;
        for offset in 0..2 {
            let index = (self.next + offset) % 2;
            let slot = &self.slots[index];
            // AcquireSync may return WAIT_TIMEOUT (a positive HRESULT). Do not use is_ok().
            let hr = unsafe {
                (Interface::vtable(&slot.mutex).AcquireSync)(Interface::as_raw(&slot.mutex), 0, 0)
            };
            if hr.0 == 0 {
                chosen = Some(index);
                break;
            }
        }
        let Some(index) = chosen else {
            self.stats.borrow_mut().dropped += 1;
            return Ok(());
        };
        let slot = &self.slots[index];
        unsafe {
            frame.context.CopyResource(&slot.texture, frame.texture);
            slot.mutex.ReleaseSync(1)?;
            frame.context.Flush();
        }
        let sent = unsafe {
            PostMessageW(
                Some(HWND(self.consumer as *mut _)),
                FRAME_MESSAGE,
                WPARAM(index),
                LPARAM(self.generation as isize),
            )
        };
        if sent.is_err() {
            self.stats.borrow_mut().error = Some("consumer unavailable".into());
        }
        let mut stats = self.stats.borrow_mut();
        stats.published += 1;
        stats.gpu_copy_bytes += desc.Width as u64 * desc.Height as u64 * 4;
        self.next = (index + 1) % 2;
        Ok(())
    }
}
