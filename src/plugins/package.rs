//! Plugin packages are business units, each with an entry and private modules.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub entry: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub resources: BTreeMap<String, String>,
    #[serde(default)]
    pub locales: crate::environment::Catalog,
    #[serde(default = "default_locale")]
    pub default_locale: String,
}
fn default_locale() -> String {
    "zh-CN".into()
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        crate::environment::validate_catalog(&self.locales, &self.default_locale)?;
        super::resources::validate(&self.resources)?;
        ensure!(
            peripheral_ui_protocol::identifier(&self.id) && self.id.len() <= 48,
            "invalid plugin ID"
        );
        ensure!(
            !self.name.is_empty() && self.name.len() <= 120 && self.description.len() <= 512,
            "invalid plugin name/description"
        );
        ensure!(
            !self.version.is_empty() && self.version.len() <= 32,
            "invalid plugin version"
        );
        ensure!(
            self.files.len() <= 64 && self.files.contains(&self.entry),
            "entry must be included in at most 64 files"
        );
        let unique = self
            .files
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        ensure!(
            unique.len() == self.files.len(),
            "duplicate plugin file path"
        );
        for path in &self.files {
            ensure!(valid_path(path), "plugin files must be relative Lua paths");
        }
        Ok(())
    }
}
fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 180
        && path.ends_with(".lua")
        && path.split('/').all(|p| {
            !p.is_empty()
                && p != "."
                && p != ".."
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
}
pub fn read(root: &Path) -> Result<Manifest> {
    let bytes = std::fs::read(root.join("plugin.json"))?;
    ensure!(bytes.len() <= 32768, "plugin manifest too large");
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    manifest.validate()?;
    for path in manifest.resources.values() {
        let source = root.join(path).canonicalize()?;
        ensure!(
            source.starts_with(root.canonicalize()?),
            "resource escapes package"
        );
        ensure!(
            std::fs::metadata(source)?.len() <= super::resources::MAX_IMAGE_BYTES,
            "image exceeds 4 MiB"
        );
    }
    for path in &manifest.files {
        let source = root.join(path).canonicalize()?;
        ensure!(
            source.starts_with(root.canonicalize()?),
            "plugin file escapes its package"
        );
        ensure!(
            std::fs::metadata(source)?.len() <= 65536,
            "plugin source exceeds 64 KiB"
        );
    }
    Ok(manifest)
}
pub fn catalog(root: &Path) -> Result<Vec<(Manifest, PathBuf)>> {
    let dir = root.join("plugins");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if !path.join("plugin.json").exists() {
            continue;
        }
        let Ok(manifest) = read(&path) else {
            continue;
        };
        if entry.file_name() != manifest.id.as_str() {
            continue;
        }
        ensure!(items.len() < 64, "plugin catalog limit reached");
        items.push((manifest, path));
    }
    items.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    Ok(items)
}
pub fn install(root: &Path, manifest: Manifest, files: BTreeMap<String, String>) -> Result<String> {
    manifest.validate()?;
    ensure!(
        manifest.resources.is_empty(),
        "resource packages must be installed from a local folder"
    );
    ensure!(
        files.len() == manifest.files.len() && files.len() <= 64,
        "package file inventory mismatch"
    );
    let total = files.values().map(String::len).sum::<usize>();
    ensure!(total <= 524288, "plugin package exceeds 512 KiB");
    for name in &manifest.files {
        ensure!(
            files.get(name).is_some_and(|s| s.len() <= 65536),
            "missing/oversize plugin source"
        );
    }
    ensure!(
        files.keys().all(|name| manifest.files.contains(name)),
        "undeclared plugin file"
    );
    let dir = root.join("plugins");
    std::fs::create_dir_all(&dir)?;
    ensure!(
        dir.canonicalize()?.starts_with(root),
        "plugin directory escapes workspace"
    );
    let target = dir.join(&manifest.id);
    ensure!(
        !target.exists(),
        "plugin already installed; use its configuration/restart controls"
    );
    let scratch = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&dir)?;
    for (name, source) in files {
        let path = scratch.path().join(name);
        std::fs::create_dir_all(path.parent().context("source parent")?)?;
        std::fs::write(path, source)?;
    }
    std::fs::write(
        scratch.path().join("plugin.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    read(scratch.path())?;
    std::fs::rename(scratch.path(), target)?;
    Ok(manifest.id)
}

/// Keep malformed packages visible without blocking healthy plugins.
pub fn broken(root: &Path) -> Result<Vec<Value>> {
    let dir = root.join("plugins");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut errors = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || !entry.path().join("plugin.json").exists() {
            continue;
        }
        let result = read(&entry.path()).and_then(|m| {
            ensure!(
                entry.file_name() == m.id.as_str(),
                "plugin directory must equal its ID"
            );
            Ok(m)
        });
        if let Err(error) = result {
            let id = entry.file_name().to_string_lossy().into_owned();
            errors.push(json!({"id":id,"name":id,"version":"?","description":"插件文件需要修复","enabled":false,"running":false,"state":"error","error":format!("{error:#}"),"devices":{},"views":[],"config":{},"config_schema":{"type":"object"}}));
        }
        ensure!(errors.len() <= 64, "too many malformed plugins");
    }
    Ok(errors)
}
