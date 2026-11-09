//! Static package images: scoped paths, bounded decodes and content identities.
use anyhow::{Result, ensure};
use peripheral_ui_protocol::ResourceRef;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, path::Path};
pub const MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024;
pub fn validate(resources: &BTreeMap<String, String>) -> Result<()> {
    ensure!(
        resources.len() <= 32,
        "at most 32 image resources per plugin"
    );
    for (id, path) in resources {
        ensure!(
            peripheral_ui_protocol::identifier(id),
            "invalid resource ID"
        );
        ensure!(
            path.len() <= 180
                && path.split('/').all(|p| !p.is_empty()
                    && p != "."
                    && p != ".."
                    && p.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))),
            "resource path must stay inside its package"
        );
        ensure!(
            ["png", "jpg", "jpeg", "webp"].contains(&path.rsplit('.').next().unwrap_or("")),
            "images must be PNG, JPEG or WebP"
        );
    }
    Ok(())
}
pub fn read_image(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_IMAGE_BYTES, "image exceeds 4 MiB");
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format()?;
    ensure!(
        matches!(
            reader.format(),
            Some(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP)
        ),
        "unsupported image content"
    );
    let (w, h) = reader.into_dimensions()?;
    ensure!(
        w > 0 && h > 0 && w <= 4096 && h <= 4096 && u64::from(w) * u64::from(h) <= 8_000_000,
        "image dimensions exceed budget"
    );
    Ok(bytes)
}
pub fn resolve(
    root: &Path,
    resources: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, ResourceRef>> {
    validate(resources)?;
    let root = root.canonicalize()?;
    let mut total = 0;
    resources
        .iter()
        .map(|(id, relative)| {
            let path = root.join(relative).canonicalize()?;
            ensure!(path.starts_with(&root), "image escapes plugin directory");
            let bytes = read_image(&path)?;
            total += bytes.len();
            ensure!(total <= 16 * 1024 * 1024, "plugin images exceed 16 MiB");
            Ok((
                id.clone(),
                ResourceRef {
                    path: path.to_string_lossy().into_owned(),
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                },
            ))
        })
        .collect()
}

/// GUI and backend are local; copy the validated package transactionally without
/// sending binary assets through the small JSON command/event channel.
pub fn install_from(root: &Path, source: &Path) -> Result<String> {
    let source = source.canonicalize()?;
    let manifest = super::package::read(&source)?;
    let dir = root.join("plugins");
    std::fs::create_dir_all(&dir)?;
    ensure!(
        dir.canonicalize()?.starts_with(root),
        "plugin directory escapes workspace"
    );
    let target = dir.join(&manifest.id);
    ensure!(!target.exists(), "plugin already installed");
    let scratch = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&dir)?;
    let mut total = 0;
    for relative in manifest.files.iter().chain(manifest.resources.values()) {
        let path = source.join(relative).canonicalize()?;
        ensure!(path.starts_with(&source), "file escapes plugin directory");
        let bytes = if manifest.files.contains(relative) {
            let text = crate::runtime::read_source(&source, relative)?;
            total += text.len();
            ensure!(total <= 524288, "plugin source exceeds 512 KiB");
            text.into_bytes()
        } else {
            read_image(&path)?
        };
        let destination = scratch.path().join(relative);
        std::fs::create_dir_all(destination.parent().unwrap())?;
        std::fs::write(destination, bytes)?;
    }
    std::fs::write(
        scratch.path().join("plugin.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    super::package::read(scratch.path())?;
    resolve(scratch.path(), &manifest.resources)?;
    std::fs::rename(scratch.path(), target)?;
    Ok(manifest.id)
}
