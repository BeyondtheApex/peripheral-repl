//! Host-owned taskbar placement preferences; Lua retains per-item ordering.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    #[default]
    Right,
}
impl Side {
    pub fn effective_left(self) -> bool {
        self == Self::Left && centered()
    }
    pub fn load() -> Self {
        std::fs::read(crate::backend::data_dir().join("preferences.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| serde_json::from_value(v["taskbar_side"].clone()).ok())
            .unwrap_or_default()
    }
}

pub fn centered() -> bool {
    use windows::{
        Win32::{
            Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
            System::Registry::*,
        },
        core::w,
    };
    let mut alignment = 1u32;
    let mut size = 4u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"),
            w!("TaskbarAl"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut alignment as *mut u32).cast()),
            Some(&mut size),
        )
    };
    (result == ERROR_SUCCESS && alignment == 1) || result == ERROR_FILE_NOT_FOUND
}

/// Identity and pixel geometry only: tray expansion/auto-hide are native layout
/// changes, whereas moving the primary display requires a new consumer bridge.
pub fn primary_target() -> Value {
    use windows::{
        Win32::{Foundation::POINT, Graphics::Gdi::*, UI::WindowsAndMessaging::*},
        core::w,
    };
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool()
            || info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY == 0
        {
            return Value::Null;
        }
        let bar = FindWindowW(w!("Shell_TrayWnd"), None).ok();
        let island = bar.and_then(|bar| {
            FindWindowExW(
                Some(bar),
                None,
                w!("Windows.UI.Composition.DesktopWindowContentBridge"),
                None,
            )
            .ok()
        });
        let r = info.monitorInfo.rcMonitor;
        serde_json::json!({
            "monitor": monitor.0 as usize,
            "centered": centered(),
            "device": String::from_utf16_lossy(&info.szDevice).trim_end_matches('\0'),
            "bounds": [r.left, r.top, r.right, r.bottom],
            "display_count": GetSystemMetrics(SM_CMONITORS),
            "desktop": [GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)],
            "taskbar": bar.map(|h| h.0 as usize),
            "island": island.map(|h| h.0 as usize),
        })
    }
}
