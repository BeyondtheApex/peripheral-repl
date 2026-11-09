use super::*;
pub(super) fn svg_icon(path: &str, tint: u32, cx: &gpui_kit::App) -> Result<HICON> {
    let bytes = gpui_kit::assets::AllAssets
        .load(path)?
        .context("missing Lucide icon")?;
    let parsed = cx.svg_renderer().parse_svg(&bytes)?;
    let rendered = cx.svg_renderer().render_parsed(
        &parsed,
        gpui_kit::SvgSize::ExactSize(gpui_kit::size(
            gpui_kit::DevicePixels(32),
            gpui_kit::DevicePixels(32),
        )),
    )?;
    let mut pixels = rendered.as_bytes(0).context("SVG frame")?.to_vec();
    // GPUI exposes straight-alpha BGRA; Win32 icons require premultiplied BGRA.
    for p in pixels.chunks_exact_mut(4) {
        p[0] = ((tint & 255) * p[3] as u32 / 255) as u8;
        p[1] = (((tint >> 8) & 255) * p[3] as u32 / 255) as u8;
        p[2] = (((tint >> 16) & 255) * p[3] as u32 / 255) as u8;
    }
    let color = bitmap(32, 32, &pixels)?;
    unsafe {
        let mask = CreateBitmap(32, 32, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: color,
            ..Default::default()
        });
        let _ = DeleteObject(mask.into());
        let _ = DeleteObject(color.into());
        Ok(icon?)
    }
}

pub fn add_tray(h: usize, id: u32, path: &str, cx: &gpui_kit::App) -> Result<()> {
    let hook = hook(h)?;
    let icon = svg_icon(path, hook.backend_status.get().color(), cx)?;
    unsafe {
        let mut data = tray_data(handle(h), id, icon);
        if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            let _ = DestroyIcon(icon);
            anyhow::bail!("tray registration failed");
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
        hook.tray.borrow_mut().push((id, icon));
    }
    Ok(())
}
pub fn set_backend_tray(
    h: usize,
    status: crate::backend_status::BackendStatus,
    cx: &gpui_kit::App,
) -> Result<()> {
    let hook = hook(h)?;
    if hook.backend_status.get() == status {
        return Ok(());
    }
    let icon = svg_icon("icons/cpu.svg", status.color(), cx)?;
    let previous = hook.backend_status.replace(status);
    let data = tray_data(handle(h), 1, icon);
    unsafe {
        if !Shell_NotifyIconW(NIM_MODIFY, &data).as_bool() {
            hook.backend_status.set(previous);
            let _ = DestroyIcon(icon);
            anyhow::bail!("application tray update failed");
        }
        let mut icons = hook.tray.borrow_mut();
        if let Some((_, old)) = icons.iter_mut().find(|(id, _)| *id == 1) {
            let _ = DestroyIcon(*old);
            *old = icon;
        } else {
            let _ = DestroyIcon(icon);
        }
    }
    hook.tray_updates.set(hook.tray_updates.get() + 1);
    Ok(())
}
/// Tray icons are small CPU bitmaps; read back only when their description changes.
pub fn set_rendered_tray(h: usize, id: u32, image: &image::RgbaImage) -> Result<()> {
    let hook = hook(h)?;
    let thumbnail = image::imageops::thumbnail(image, 32, 32);
    let mut image = image::RgbaImage::new(32, 32);
    image::imageops::overlay(
        &mut image,
        &thumbnail,
        ((32 - thumbnail.width()) / 2) as i64,
        ((32 - thumbnail.height()) / 2) as i64,
    );
    let mut bgra = image.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        for i in 0..3 {
            pixel[i] = (pixel[i] as u16 * pixel[3] as u16 / 255) as u8;
        }
    }
    let color = bitmap(32, 32, &bgra)?;
    unsafe {
        let mask = CreateBitmap(32, 32, 1, 1, None);
        let result = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: color,
            ..Default::default()
        });
        let _ = DeleteObject(mask.into());
        let _ = DeleteObject(color.into());
        let icon = result?;
        let mut icons = hook.tray.borrow_mut();
        let previous = icons.iter().position(|(key, _)| *key == id);
        let mut data = tray_data(handle(h), id, icon);
        if !Shell_NotifyIconW(
            if previous.is_some() {
                NIM_MODIFY
            } else {
                NIM_ADD
            },
            &data,
        )
        .as_bool()
        {
            let _ = DestroyIcon(icon);
            anyhow::bail!("tray update failed");
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
        if let Some(index) = previous {
            let _ = DestroyIcon(icons[index].1);
            icons[index].1 = icon;
        } else {
            icons.push((id, icon));
        }
    }
    Ok(())
}
pub(super) fn tray_data(hwnd: HWND, id: u32, icon: HICON) -> NOTIFYICONDATAW {
    let mut d = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: id,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: TRAY_MESSAGE,
        hIcon: icon,
        ..Default::default()
    };
    let tip = hook(hwnd.0 as usize)
        .map(|h| {
            if h.id == "main" {
                format!("Peripheral · {}", h.backend_status.get().label())
            } else {
                h.title.borrow().clone()
            }
        })
        .unwrap_or_else(|_| "Peripheral".into());
    for (i, c) in tip.encode_utf16().take(127).enumerate() {
        d.szTip[i] = c;
    }
    d
}
