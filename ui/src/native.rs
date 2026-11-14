//! Windows presentation policy. All HWND mutations stay on the owning UI thread.
use crate::control::Request;
use anyhow::{Context, Result};
use gpui_kit::{AssetSource, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    sync::mpsc,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::w,
};

pub mod appearance;
pub mod clipboard;
mod messages;
pub mod popup;
mod preview;
mod tray;
mod tray_menu;
use messages::proc;
use preview::present_preview;
pub use preview::{cache_preview, enable_preview};
use tray::tray_data;
pub use tray::{add_tray, set_backend_tray, set_rendered_tray};
const SUBCLASS: usize = 0x5052504c;
const TRAY_MESSAGE: u32 = WM_APP + 11;
struct Hook {
    frameless: Cell<bool>,
    passive: Cell<bool>,
    overlay: bool,
    popup: Cell<bool>,
    commands: async_channel::Sender<Request>,
    id: String,
    title: RefCell<String>,
    tray: RefCell<Vec<(u32, HICON)>>,
    backend_status: Cell<crate::backend_status::BackendStatus>,
    tray_updates: Cell<u64>,
    tray_retry_attempts: Cell<u32>,
    tray_error: RefCell<Option<String>>,
    taskbar_created: u32,
    clicks: Cell<u64>,
    thumb_requests: Cell<u64>,
    preview: RefCell<Option<image::RgbaImage>>,
    preview_error: RefCell<Option<String>>,
}
impl Drop for Hook {
    fn drop(&mut self) {
        for (_, icon) in self.tray.get_mut().drain(..) {
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }
    }
}
fn enqueue(h: &Hook, body: serde_json::Value) {
    let (tx, _) = mpsc::sync_channel(1);
    let _ = h.commands.try_send(Request {
        body,
        reply: tx,
        expires: None,
    });
}

pub fn hwnd(window: &Window) -> Result<usize> {
    match HasWindowHandle::window_handle(window)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?
        .as_raw()
    {
        RawWindowHandle::Win32(h) => Ok(h.hwnd.get() as usize),
        _ => anyhow::bail!("not a Windows window"),
    }
}
fn handle(h: usize) -> HWND {
    HWND(h as *mut _)
}
pub fn open_file(path: &std::path::Path) -> Result<()> {
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &windows::core::HSTRING::from(path.as_os_str()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    anyhow::ensure!(result.0 as usize > 32, "open installation report failed");
    Ok(())
}
pub struct Singleton(HANDLE);
impl Drop for Singleton {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
pub fn singleton() -> Result<Singleton> {
    // Debug builds can use an explicitly isolated data directory for full UI
    // lifecycle tests without stopping the user's production manager.
    #[cfg(debug_assertions)]
    let isolated_name = std::env::var_os("PERIPHERAL_UI_DATA").map(|path| {
        use sha2::{Digest, Sha256};
        windows::core::HSTRING::from(format!(
            "Local\\PeripheralManagerTest_{:x}",
            Sha256::digest(path.to_string_lossy().as_bytes())
        ))
    });
    #[cfg(not(debug_assertions))]
    let isolated_name: Option<windows::core::HSTRING> = None;
    unsafe {
        let mutex = windows::Win32::System::Threading::CreateMutexW(
            None,
            false,
            isolated_name
                .as_ref()
                .map_or(w!("Local\\PeripheralManager"), |name| {
                    windows::core::PCWSTR(name.as_ptr())
                }),
        )?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(mutex);
            anyhow::bail!("Peripheral manager is already running; use its control endpoint");
        }
        Ok(Singleton(mutex))
    }
}
pub fn floating_frame(h: usize, no_activate: bool) -> Result<()> {
    unsafe {
        let hwnd = handle(h);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            ((style & !(WS_CAPTION.0 | WS_THICKFRAME.0)) | WS_POPUP.0) as isize,
        );
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            ((ex & !(WS_EX_WINDOWEDGE.0
                | WS_EX_CLIENTEDGE.0
                | WS_EX_DLGMODALFRAME.0
                | WS_EX_STATICEDGE.0
                | WS_EX_APPWINDOW.0))
                | WS_EX_TOOLWINDOW.0
                | if no_activate { WS_EX_NOACTIVATE.0 } else { 0 }) as isize,
        );
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )?;
    }
    post(h, WM_APP + 79, 0, 0);
    Ok(())
}
pub fn install(
    h: usize,
    id: String,
    overlay: bool,
    commands: async_channel::Sender<Request>,
) -> Result<()> {
    let hook = Box::new(Hook {
        frameless: Cell::new(false),
        passive: Cell::new(overlay),
        popup: Cell::new(false),
        overlay,
        commands,
        id,
        title: RefCell::new("Peripheral".into()),
        tray: RefCell::new(Vec::new()),
        backend_status: Cell::new(Default::default()),
        tray_updates: Cell::new(0),
        tray_retry_attempts: Cell::new(0),
        tray_error: RefCell::new(None),
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        clicks: Cell::new(0),
        thumb_requests: Cell::new(0),
        preview: RefCell::new(None),
        preview_error: RefCell::new(None),
    });
    let ptr = Box::into_raw(hook);
    if !unsafe { SetWindowSubclass(handle(h), Some(proc), SUBCLASS, ptr as usize) }.as_bool() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
        anyhow::bail!("SetWindowSubclass failed");
    }
    if overlay {
        set_passive(h, true)?;
    }
    Ok(())
}
fn hook(h: usize) -> Result<&'static Hook> {
    let mut data = 0;
    anyhow::ensure!(
        unsafe { GetWindowSubclass(handle(h), Some(proc), SUBCLASS, Some(&mut data)) }.as_bool(),
        "surface closed"
    );
    Ok(unsafe { &*(data as *const Hook) })
}
pub fn set_passive(h: usize, passive: bool) -> Result<()> {
    let hook = hook(h)?;
    anyhow::ensure!(hook.overlay, "not an overlay");
    unsafe {
        let base = GetWindowLongPtrW(handle(h), GWL_STYLE) as u32;
        SetWindowLongPtrW(
            handle(h),
            GWL_STYLE,
            ((base & !(WS_CAPTION.0 | WS_THICKFRAME.0)) | WS_POPUP.0) as isize,
        );
        let mut style = GetWindowLongPtrW(handle(h), GWL_EXSTYLE) as u32;
        style |= WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;
        style &= !WS_EX_APPWINDOW.0;
        if passive {
            style |= WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0;
        } else {
            style &= !(WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0);
        }
        SetWindowLongPtrW(handle(h), GWL_EXSTYLE, style as isize);
        if passive {
            SetLayeredWindowAttributes(handle(h), COLORREF(0), 255, LWA_ALPHA)?;
        }
        SetWindowPos(
            handle(h),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )?;
    }
    hook.passive.set(passive);
    Ok(())
}
pub fn info(h: usize) -> serde_json::Value {
    unsafe {
        let mut rect = RECT::default();
        let _ = GetWindowRect(handle(h), &mut rect);
        let hk = hook(h).ok();
        json!({"hwnd":h,"bounds":{"x":rect.left,"y":rect.top,"width":rect.right-rect.left,"height":rect.bottom-rect.top},"corners":appearance::corner_status(h),"backdrop":gpui_windows::backdrop::status(h).map(|s|json!({"path":s.path,"error":s.error})),"ex_style":GetWindowLongPtrW(handle(h),GWL_EXSTYLE),"foreground":GetForegroundWindow()==handle(h),"visible":IsWindowVisible(handle(h)).as_bool(),"backend_tray":hk.filter(|h|h.id=="main").map(|h|json!({"status":h.backend_status.get(),"color":format!("#{:06x}",h.backend_status.get().color()),"label":h.backend_status.get().label(),"updates":h.tray_updates.get(),"registration_error":h.tray_error.borrow().clone(),"retry_attempts":h.tray_retry_attempts.get(),"menu":tray_menu::items(h.backend_status.get())})),"native_clicks":hk.map(|x|x.clicks.get()),"thumbnail_requests":hk.map(|x|x.thumb_requests.get()),"preview_error":hk.and_then(|x|x.preview_error.borrow().clone())})
    }
}
pub fn place(h: usize, x: i32, y: i32, width: i32, height: i32) -> Result<()> {
    anyhow::ensure!(
        (16..=4096).contains(&width) && (16..=2160).contains(&height),
        "invalid bounds"
    );
    unsafe {
        SetWindowPos(
            handle(h),
            None,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )?;
    }
    post(h, WM_APP + 79, 0, 0);
    Ok(())
}
pub fn show(h: usize, visible: bool) {
    unsafe {
        let _ = ShowWindow(handle(h), if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
}
pub fn is_visible(h: usize) -> bool {
    unsafe { IsWindowVisible(handle(h)).as_bool() }
}
/// GPUI caches the original caption border and its resize may activate HWNDs.
/// Frameless surfaces use the actual current rectangle with NOACTIVATE instead.
pub fn resize_floating(h: usize, width: i32, height: i32) -> Result<()> {
    let mut rect = RECT::default();
    unsafe {
        GetWindowRect(handle(h), &mut rect)?;
    }
    place(h, rect.left, rect.top, width, height)
}
pub fn foreground() -> usize {
    unsafe { GetForegroundWindow().0 as usize }
}
pub fn compositor_timing() -> serde_json::Value {
    unsafe {
        let mut info = DWM_TIMING_INFO {
            cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
            ..Default::default()
        };
        match DwmGetCompositionTimingInfo(HWND::default(), &mut info) {
            Ok(()) => {
                let refresh_numerator = info.rateRefresh.uiNumerator;
                let refresh_denominator = info.rateRefresh.uiDenominator;
                let composition_numerator = info.rateCompose.uiNumerator;
                let composition_denominator = info.rateCompose.uiDenominator;
                json!({"refresh_numerator":refresh_numerator,"refresh_denominator":refresh_denominator,"composition_numerator":composition_numerator,"composition_denominator":composition_denominator})
            }
            Err(e) => json!({"error":e.to_string()}),
        }
    }
}
pub fn prepare_export(h: usize, width: i32, height: i32) -> Result<()> {
    unsafe {
        let style = GetWindowLongPtrW(handle(h), GWL_STYLE) as u32;
        SetWindowLongPtrW(
            handle(h),
            GWL_STYLE,
            ((style & !(WS_CAPTION.0 | WS_THICKFRAME.0)) | WS_POPUP.0) as isize,
        );
        let ex = GetWindowLongPtrW(handle(h), GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(
            handle(h),
            GWL_EXSTYLE,
            ((ex & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as isize,
        );
        SetWindowPos(
            handle(h),
            None,
            -10000,
            -10000,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW | SWP_FRAMECHANGED,
        )?;
    }
    post(h, WM_APP + 79, 0, 0);
    Ok(())
}
pub fn post(h: usize, message: u32, w: usize, l: isize) {
    unsafe {
        let _ = PostMessageW(Some(handle(h)), message, WPARAM(w), LPARAM(l));
    }
}

pub fn process_alive(pid: u32) -> bool {
    use windows::Win32::System::Threading::*;
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Ok(handle) => {
                let result = WaitForSingleObject(handle, 0);
                let _ = CloseHandle(handle);
                result != WAIT_OBJECT_0
            }
            Err(_) => GetLastError() != ERROR_INVALID_PARAMETER,
        }
    }
}
pub fn bitmap(width: u32, height: u32, bgra: &[u8]) -> Result<HBITMAP> {
    anyhow::ensure!(
        width > 0
            && height > 0
            && width <= 4096
            && height <= 2160
            && bgra.len() == width as usize * height as usize * 4,
        "invalid bitmap"
    );
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bmp = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)? };
    unsafe {
        std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
    }
    Ok(bmp)
}

pub fn valid_window(h: usize) -> bool {
    unsafe { IsWindow(Some(handle(h))).as_bool() }
}

pub fn set_title(h: usize, title: &str) -> Result<()> {
    *hook(h)?.title.borrow_mut() = title.into();
    unsafe {
        SetWindowTextW(handle(h), &windows::core::HSTRING::from(title))?;
    }
    Ok(())
}
