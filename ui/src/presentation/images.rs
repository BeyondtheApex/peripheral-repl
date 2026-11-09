//! File reads, resampling and alpha masks run once per asset/size on a worker.
//! GPUI caches the result and uploads it to the normal shared-texture path.
use gpui_kit::*;
use peripheral_ui_protocol::{Clip, Fit, ImageSpec, ResourceRef};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
mod mask;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    resource: ResourceRef,
    width: u32,
    height: u32,
    radius: u32,
    fit: Fit,
    clip: Clip,
}
struct PreparedImage;
impl Asset for PreparedImage {
    type Source = Key;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;
    fn load(source: Key, cx: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        cx.background_executor()
            .spawn(async move { prepare(&source).map_err(Into::into) })
    }
}
fn prepare(key: &Key) -> anyhow::Result<Arc<RenderImage>> {
    let bytes = peripheral_repl::plugins::resources::read_image(Path::new(&key.resource.path))?;
    anyhow::ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == key.resource.sha256,
        "plugin image changed; restart the plugin to reload its resources"
    );
    let decoded = image::load_from_memory(&bytes)?;
    let resized = match key.fit {
        Fit::Contain => {
            decoded.resize(key.width, key.height, image::imageops::FilterType::Triangle)
        }
        Fit::Cover => {
            decoded.resize_to_fill(key.width, key.height, image::imageops::FilterType::Triangle)
        }
        Fit::Fill => {
            decoded.resize_exact(key.width, key.height, image::imageops::FilterType::Triangle)
        }
    }
    .into_rgba8();
    let mut output = image::RgbaImage::new(key.width, key.height);
    image::imageops::overlay(
        &mut output,
        &resized,
        ((key.width - resized.width()) / 2) as i64,
        ((key.height - resized.height()) / 2) as i64,
    );
    mask::apply(&mut output, key.clip, key.radius as f32);
    // RenderImage is straight-alpha BGRA (the renderer premultiplies at sampling).
    for pixel in output.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(output)])))
}
pub fn element(
    spec: &ImageSpec,
    resource: Option<&ResourceRef>,
    width: f32,
    height: f32,
) -> AnyElement {
    let Some(resource) = resource.cloned() else {
        return placeholder("图片未声明").into_any_element();
    };
    let spec = spec.clone();
    img(move |window: &mut Window, cx: &mut App| {
        let scale = window.scale_factor();
        let key = Key {
            resource: resource.clone(),
            width: (width * scale).round().clamp(1., 1024.) as u32,
            height: (height * scale).round().clamp(1., 1024.) as u32,
            radius: (spec.radius * scale).round().clamp(0., 1024.) as u32,
            fit: spec.fit,
            clip: spec.clip,
        };
        window.use_asset::<PreparedImage>(&key, cx)
    })
    .w(px(width))
    .h(px(height))
    .object_fit(ObjectFit::Fill)
    .with_loading(|| placeholder("加载图片").into_any_element())
    .with_fallback(|| placeholder("图片不可用").into_any_element())
    .into_any_element()
}
fn placeholder(text: &'static str) -> Div {
    div()
        .size_full()
        .rounded_lg()
        .bg(rgb(0x222d3d))
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(rgb(0x96a2b7))
        .child(text)
}

#[cfg(test)]
mod tests {
    use super::{Clip, Digest, Fit, Key, ResourceRef, Sha256, prepare};
    #[test]
    fn image_fit_alpha_color_and_content_identity_are_preserved() {
        let dir = std::env::temp_dir().join(format!("peripheral-image-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("photo.png");
        image::RgbaImage::from_pixel(20, 10, image::Rgba([200, 100, 50, 128]))
            .save(&path)
            .unwrap();
        let resource = ResourceRef {
            path: path.to_string_lossy().into_owned(),
            sha256: format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap())),
        };
        let mut key = Key {
            resource,
            width: 20,
            height: 20,
            radius: 0,
            fit: Fit::Contain,
            clip: Clip::None,
        };
        let contain = prepare(&key).unwrap();
        let bytes = contain.as_bytes(0).unwrap();
        assert_eq!(bytes[3], 0);
        assert_eq!(
            &bytes[(10 * 20 + 10) * 4..(10 * 20 + 10) * 4 + 4],
            &[50, 100, 200, 128]
        );
        for fit in [Fit::Cover, Fit::Fill] {
            key.fit = fit;
            assert_eq!(prepare(&key).unwrap().as_bytes(0).unwrap()[3], 128);
        }
        std::fs::write(&path, b"changed resource").unwrap();
        assert!(prepare(&key).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
