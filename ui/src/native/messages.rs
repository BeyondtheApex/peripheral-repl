use super::*;
// shellapi.h macro omitted by windows-rs metadata: NIN_SELECT | NINF_KEY.
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
pub(super) unsafe extern "system" fn proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // Never unwind through Explorer/Win32 callback boundaries.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let h = unsafe { &*(data as *const Hook) };
        if h.id == "main" && (msg == WM_SETTINGCHANGE || msg == WM_THEMECHANGED) {
            enqueue(h, json!({"op":"appearance_system_changed"}));
        }
        if h.frameless.get() && msg == WM_NCCALCSIZE && w.0 != 0 {
            unsafe {
                appearance::non_client_size(hwnd, l);
            }
            return Some(LRESULT(0));
        }
        if h.frameless.get() && msg == WM_NCHITTEST && !unsafe { IsZoomed(hwnd) }.as_bool() {
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(hwnd, &mut r);
            }
            let x = l.0 as u16 as i16 as i32;
            let y = (l.0 >> 16) as u16 as i16 as i32;
            let edge = 8;
            let left = x < r.left + edge;
            let right = x >= r.right - edge;
            let top = y < r.top + edge;
            let bottom = y >= r.bottom - edge;
            let hit = match (left, right, top, bottom) {
                (true, _, true, _) => HTTOPLEFT,
                (_, true, true, _) => HTTOPRIGHT,
                (true, _, _, true) => HTBOTTOMLEFT,
                (_, true, _, true) => HTBOTTOMRIGHT,
                (true, _, _, _) => HTLEFT,
                (_, true, _, _) => HTRIGHT,
                (_, _, true, _) => HTTOP,
                (_, _, _, true) => HTBOTTOM,
                _ => 0,
            };
            if hit != 0 {
                return Some(LRESULT(hit as isize));
            }
        }
        if msg == WM_APP + 79 {
            // Native resize inside an App update cannot reenter GPUI's resize
            // callback. Deliver after that borrow ends, using current geometry.
            let mut client = RECT::default();
            unsafe {
                if GetClientRect(hwnd, &mut client).is_ok() {
                    let packed = ((client.bottom - client.top) as u32) << 16
                        | ((client.right - client.left) as u32 & 0xffff);
                    return Some(DefSubclassProc(
                        hwnd,
                        WM_SIZE,
                        WPARAM(SIZE_RESTORED as usize),
                        LPARAM(packed as isize),
                    ));
                }
            }
        }
        if msg == WM_NCDESTROY {
            gpui_windows::backdrop::forget(hwnd.0 as usize);
            popup::unregister(hwnd.0 as usize);
            for (id, icon) in h.tray.borrow().iter() {
                let d = tray_data(hwnd, *id, *icon);
                unsafe {
                    let _ = Shell_NotifyIconW(NIM_DELETE, &d);
                }
            }
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS);
                drop(Box::from_raw(data as *mut Hook));
            }
            return None;
        }
        if msg == h.taskbar_created {
            for (id, icon) in h.tray.borrow().iter() {
                let mut d = tray_data(hwnd, *id, *icon);
                unsafe {
                    let _ = Shell_NotifyIconW(NIM_ADD, &d);
                    d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                    let _ = Shell_NotifyIconW(NIM_SETVERSION, &d);
                }
            }
        }
        if h.popup.get() && msg == WM_ACTIVATE && (w.0 & 0xffff) == WA_INACTIVE as usize {
            enqueue(h, json!({"op":"hide","surface":h.id}));
        }
        if (h.overlay || h.popup.get()) && msg == WM_MOUSEACTIVATE {
            return Some(LRESULT(MA_NOACTIVATE as isize));
        }
        if h.overlay && h.passive.get() && msg == WM_NCHITTEST {
            return Some(LRESULT(HTTRANSPARENT as isize));
        }
        if msg == WM_LBUTTONDOWN {
            h.clicks.set(h.clicks.get() + 1);
        }
        if msg == WM_APP + 78 {
            enqueue(
                h,
                json!({"op":"pointer","surface":h.id,"event":{"message":w.0&0xffff,"wparam":w.0>>16,"x":l.0 as u16 as i16,"y":((l.0 as u32)>>16) as u16 as i16}}),
            );
            return Some(LRESULT(0));
        }
        if msg == TRAY_MESSAGE {
            let event = (l.0 as u32) & 0xffff;
            // All icons use NOTIFYICON_VERSION_4. Explorer also delivers raw
            // mouse-up; handling both would toggle twice for one physical click.
            if event == NIN_SELECT || event == NIN_KEYSELECT {
                enqueue(
                    h,
                    json!({"op":"tray_action","surface":h.id,"button":"left"}),
                );
            }
            if event == WM_CONTEXTMENU {
                if h.id != "main" {
                    enqueue(
                        h,
                        json!({"op":"tray_action","surface":h.id,"button":"right"}),
                    );
                    return Some(LRESULT(0));
                }
                tray_menu::show(hwnd, h);
            }
            return Some(LRESULT(0));
        }
        if msg == WM_DWMSENDICONICTHUMBNAIL || msg == WM_DWMSENDICONICLIVEPREVIEWBITMAP {
            h.thumb_requests.set(h.thumb_requests.get() + 1);
            if let Some(image) = h.preview.borrow().as_ref() {
                let live = msg == WM_DWMSENDICONICLIVEPREVIEWBITMAP;
                let w = ((l.0 as u32) >> 16).clamp(1, 1024);
                let height = ((l.0 as u32) & 0xffff).clamp(1, 768);
                let resized = if live {
                    image.clone()
                } else {
                    image::DynamicImage::ImageRgba8(image.clone())
                        .thumbnail(w, height)
                        .to_rgba8()
                };
                *h.preview_error.borrow_mut() = present_preview(hwnd.0 as usize, &resized, live)
                    .err()
                    .map(|e| e.to_string());
            } else {
                enqueue(h, json!({"op":"refresh_preview","surface":h.id}));
            }
            return Some(LRESULT(0));
        }
        None
    }));
    match result {
        Ok(Some(r)) => r,
        _ => unsafe { DefSubclassProc(hwnd, msg, w, l) },
    }
}
