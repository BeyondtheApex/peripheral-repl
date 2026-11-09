//! Windows backdrop and caption policy. Failures fall back to opaque application colors.
use super::*;
use windows::Win32::{
    Globalization::GetUserDefaultLocaleName,
    System::Registry::*,
    UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
};
pub struct System {
    pub dark: bool,
    pub transparency: bool,
    pub locale: String,
}
fn preference(name: &str, default: u32) -> u32 {
    let mut value = default;
    let mut size = 4;
    unsafe {
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            &windows::core::HSTRING::from(name),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        );
    }
    value
}
pub fn system() -> System {
    let mut name = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(&mut name) };
    let locale = if n > 1 {
        String::from_utf16_lossy(&name[..n as usize - 1])
    } else {
        "en".into()
    };
    let mut contrast = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            Some((&mut contrast as *mut HIGHCONTRASTW).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    System {
        dark: preference("AppsUseLightTheme", 0) == 0,
        transparency: preference("EnableTransparency", 1) != 0
            && contrast.dwFlags.0 & HCF_HIGHCONTRASTON.0 == 0,
        locale: if locale.starts_with("zh") {
            "zh-CN".into()
        } else {
            "en".into()
        },
    }
}
pub fn apply(h: usize, main: bool, popup: bool, options: &crate::appearance::Options) -> bool {
    let hwnd = handle(h);
    let dark = i32::from(crate::appearance::environment().theme == "dark");
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const i32).cast(),
            4,
        );
    }
    if main {
        let _ = caption(h, options.hide_titlebar);
    }
    if !main && !popup {
        return false;
    }
    // Removing WS_CAPTION removes Windows' automatic rounding heuristic.
    // Explicitly opt the real HWND into compositor clipping, which also clips
    // Acrylic/Mica behind our transparent client area. Rounding only the GPUI
    // root would leave a square native backdrop visible at the corners.
    apply_corners(hwnd);
    if popup {
        // Frameless menus paint their own edge. Suppress the additional DWM
        // outline, which otherwise shows as a dark strip at the bottom.
        let color = DWMWA_COLOR_NONE;
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                (&color as *const u32).cast(),
                std::mem::size_of_val(&color) as u32,
            );
        }
    }
    // GPUI owns both the native backdrop and transparent swapchain state.
    // Do not layer a second DWM backdrop over its Acrylic composition policy.
    let status = gpui_windows::backdrop::status(h);
    let available = status.as_ref().is_some_and(|s| s.error.is_none());
    if !available {
        crate::appearance::unavailable_material();
    }
    available && options.material != "solid"
}

fn apply_corners(hwnd: HWND) {
    let preference = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&preference as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
            std::mem::size_of_val(&preference) as u32,
        );
    }
    // DWM intentionally uses square edges while maximized or snapped, and
    // restores rounded corners when the window returns to a floating state.
}

pub fn corner_status(h: usize) -> serde_json::Value {
    let mut preference = DWM_WINDOW_CORNER_PREFERENCE::default();
    let result = unsafe {
        DwmGetWindowAttribute(
            handle(h),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&mut preference as *mut DWM_WINDOW_CORNER_PREFERENCE).cast(),
            std::mem::size_of_val(&preference) as u32,
        )
    };
    json!({"preference":result.is_ok().then_some(preference.0),"error":result.err().map(|e|e.to_string())})
}

pub(super) unsafe fn non_client_size(hwnd: HWND, l: LPARAM) {
    if unsafe { IsZoomed(hwnd) }.as_bool() {
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            // Keep maximized borderless windows inside this monitor's work area.
            unsafe {
                (*(l.0 as *mut NCCALCSIZE_PARAMS)).rgrc[0] = info.rcWork;
            }
        }
    }
}
fn caption(h: usize, hidden: bool) -> Result<()> {
    let hook = hook(h)?;
    if hook.frameless.get() == hidden {
        return Ok(());
    }
    hook.frameless.set(hidden);
    unsafe {
        let style = GetWindowLongPtrW(handle(h), GWL_STYLE) as u32;
        SetWindowLongPtrW(
            handle(h),
            GWL_STYLE,
            if hidden {
                style & !WS_CAPTION.0
            } else {
                style | WS_CAPTION.0
            } as isize,
        );
        SetWindowPos(
            handle(h),
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
pub fn window_action(h: usize, action: &str) {
    unsafe {
        match action {
            "minimize" => {
                let _ = ShowWindowAsync(handle(h), SW_MINIMIZE);
            }
            "maximize" => {
                let _ = ShowWindowAsync(
                    handle(h),
                    if IsZoomed(handle(h)).as_bool() {
                        SW_RESTORE
                    } else {
                        SW_MAXIMIZE
                    },
                );
            }
            "hide" => {
                let _ = ShowWindowAsync(handle(h), SW_HIDE);
            }
            _ => {}
        }
    }
}
