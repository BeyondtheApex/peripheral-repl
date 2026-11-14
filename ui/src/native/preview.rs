use super::*;
pub fn enable_preview(h: usize) -> Result<()> {
    let yes = 1i32;
    unsafe {
        DwmSetWindowAttribute(
            handle(h),
            DWMWA_FORCE_ICONIC_REPRESENTATION,
            &yes as *const _ as _,
            4,
        )?;
        DwmSetWindowAttribute(handle(h), DWMWA_HAS_ICONIC_BITMAP, &yes as *const _ as _, 4)?;
    }
    Ok(())
}
pub fn cache_preview(h: usize, image: image::RgbaImage) -> Result<()> {
    *hook(h)?.preview.borrow_mut() = Some(image);
    unsafe {
        DwmInvalidateIconicBitmaps(handle(h))?;
    }
    Ok(())
}
pub fn present_preview(h: usize, image: &image::RgbaImage, live: bool) -> Result<()> {
    let mut bytes = image.as_raw().clone();
    for p in bytes.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
        for c in 0..3 {
            p[c] = (p[c] as u16 * p[3] as u16 / 255) as u8;
        }
    }
    let bmp = bitmap(image.width(), image.height(), &bytes)?;
    let result = unsafe {
        if live {
            DwmSetIconicLivePreviewBitmap(handle(h), bmp, None, 0)
        } else {
            DwmSetIconicThumbnail(handle(h), bmp, 0)
        }
    };
    unsafe {
        let _ = DeleteObject(bmp.into());
    }
    result?;
    Ok(())
}
