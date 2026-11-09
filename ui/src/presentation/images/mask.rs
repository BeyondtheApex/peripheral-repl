use peripheral_ui_protocol::Clip;
pub(super) fn apply(image: &mut image::RgbaImage, clip: Clip, radius: f32) {
    if clip == Clip::None {
        return;
    }
    let (w, h) = (image.width() as f32, image.height() as f32);
    let r = radius.min(w.min(h) / 2.);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        // Four subpixel samples preserve smooth transparent edges on tray icons.
        let mut coverage = 0;
        for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
            let (px, py) = (x as f32 + dx, y as f32 + dy);
            let inside = if clip == Clip::Circle {
                (px - w / 2.).powi(2) + (py - h / 2.).powi(2) <= (w.min(h) / 2.).powi(2)
            } else if r == 0. {
                true
            } else {
                let nx = ((px - w / 2.).abs() - (w / 2. - r)).max(0.) / r;
                let ny = ((py - h / 2.).abs() - (h / 2. - r)).max(0.) / r;
                match clip {
                    Clip::Bevel => nx + ny <= 1.,
                    Clip::Superellipse => nx.powi(4) + ny.powi(4) <= 1.,
                    _ => nx * nx + ny * ny <= 1.,
                }
            };
            if inside {
                coverage += 1;
            }
        }
        pixel[3] = (u16::from(pixel[3]) * coverage / 4) as u8;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipping_keeps_center_and_source_alpha_but_removes_corners() {
        for clip in [Clip::Circle, Clip::Rounded, Clip::Superellipse, Clip::Bevel] {
            let mut pixels = image::RgbaImage::from_pixel(40, 40, image::Rgba([255, 0, 0, 128]));
            apply(&mut pixels, clip, 16.);
            assert_eq!(pixels.get_pixel(20, 20)[3], 128);
            assert_eq!(pixels.get_pixel(0, 0)[3], 0);
        }
    }
}
