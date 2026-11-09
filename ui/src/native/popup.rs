//! Non-activating menu surfaces. The temporary mouse observer never consumes input.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
thread_local! {
    static MENUS: RefCell<BTreeMap<usize,(String,bool)>> = const {RefCell::new(BTreeMap::new())};
    static OBSERVER: Cell<Option<HHOOK>> = const {Cell::new(None)};
}
pub fn register(h: usize, family: String) -> Result<()> {
    if OBSERVER.with(|v| v.get().is_none()) {
        let observer = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), None, 0) }?;
        OBSERVER.with(|v| v.set(Some(observer)));
    }
    MENUS.with(|v| {
        v.borrow_mut().insert(h, (family, false));
    });
    Ok(())
}
pub(super) fn unregister(h: usize) {
    let empty = MENUS.with(|v| {
        let mut menus = v.borrow_mut();
        menus.remove(&h);
        menus.is_empty()
    });
    if empty && let Some(observer) = OBSERVER.with(|v| v.take()) {
        unsafe {
            let _ = UnhookWindowsHookEx(observer);
        }
    }
}
unsafe extern "system" fn mouse(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    let _ = std::panic::catch_unwind(|| {
        if code < 0 || ![WM_LBUTTONDOWN, WM_RBUTTONDOWN, WM_MBUTTONDOWN].contains(&(w.0 as u32)) {
            return;
        }
        let point = unsafe { (*(l.0 as *const MSLLHOOKSTRUCT)).pt };
        dismiss_outside(point);
    });
    unsafe { CallNextHookEx(None, code, w, l) }
}
fn dismiss_outside(point: POINT) {
    MENUS.with(|menus| {
        let mut menus = menus.borrow_mut();
        let inside = menus
            .iter()
            .filter_map(|(h, (family, _))| {
                let mut rect = RECT::default();
                unsafe {
                    if IsWindowVisible(handle(*h)).as_bool()
                        && GetWindowRect(handle(*h), &mut rect).is_ok()
                        && point.x >= rect.left
                        && point.x < rect.right
                        && point.y >= rect.top
                        && point.y < rect.bottom
                    {
                        Some(family.clone())
                    } else {
                        None
                    }
                }
            })
            .collect::<BTreeSet<_>>();
        let mut dismissed = BTreeSet::new();
        for (h, (family, pending)) in menus.iter_mut() {
            if !*pending
                && !inside.contains(family)
                && let Ok(hook) = hook(*h)
            {
                *pending = true;
                if dismissed.insert(family.clone()) {
                    enqueue(hook, json!({"op":"dismiss_popup","surface":if family==crate::dashboard::ID{crate::dashboard::ID}else{&hook.id}}));
                }
            }
        }
    });
}
pub fn show_at(h: usize, parent: Option<usize>) -> Result<()> {
    unsafe {
        hook(h)?.popup.set(true);
        let mut point = POINT::default();
        GetCursorPos(&mut point)?;
        let mut owner = RECT::default();
        if let Some(parent) = parent {
            GetWindowRect(handle(parent), &mut owner)?;
            point = POINT {
                x: owner.right,
                y: owner.top,
            };
        }
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        anyhow::ensure!(
            GetMonitorInfoW(monitor, &mut info).as_bool(),
            "monitor work area unavailable"
        );
        let mut rect = RECT::default();
        GetWindowRect(handle(h), &mut rect)?;
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let width = width.min(info.rcWork.right - info.rcWork.left);
        let height = height.min(info.rcWork.bottom - info.rcWork.top);
        if width != rect.right - rect.left || height != rect.bottom - rect.top {
            super::place(h, rect.left, rect.top, width, height)?;
        }
        let mut x = if parent.is_some() {
            point.x + 4
        } else {
            point.x - width / 2
        };
        let y = if parent.is_some() {
            point.y
        } else {
            point.y - height - 12
        };
        if parent.is_some() && x + width > info.rcWork.right {
            x = owner.left - width - 4;
        }
        let x = x.clamp(
            info.rcWork.left,
            (info.rcWork.right - width).max(info.rcWork.left),
        );
        let y = y.clamp(
            info.rcWork.top,
            (info.rcWork.bottom - height).max(info.rcWork.top),
        );
        SetWindowPos(
            handle(h),
            Some(HWND_TOPMOST),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_SHOWWINDOW | SWP_NOACTIVATE,
        )?;
        Ok(())
    }
}
