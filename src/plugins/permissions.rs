//! Explicit, package-scoped grants. Plugin Lua cannot read or write this store.
use super::package::Manifest;
use crate::bindings::{Policy, validate_policy};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub title: String,
    pub description: String,
    #[serde(default = "required")]
    pub required: bool,
    pub policy: Policy,
}
fn required() -> bool {
    true
}

pub fn validate(requests: &[Request]) -> Result<()> {
    ensure!(requests.len() <= 32, "maximum 32 permission groups");
    let mut ids = BTreeSet::new();
    for request in requests {
        ensure!(
            peripheral_ui_protocol::identifier(&request.id) && ids.insert(&request.id),
            "invalid or duplicate permission ID"
        );
        ensure!(
            !request.title.is_empty()
                && request.title.len() <= 160
                && !request.description.is_empty()
                && request.description.len() <= 2048,
            "permission title and explanation required"
        );
        validate_policy(&request.policy)?;
    }
    Ok(())
}
fn digest(value: &impl Serialize) -> Result<String> {
    // A canonical object order is necessary even when serde_json's preserve_order
    // feature is enabled transitively by the UI dependency graph.
    fn canonical(v: Value) -> Value {
        match v {
            Value::Object(m) => Value::Object(
                m.into_iter()
                    .map(|(k, v)| (k, canonical(v)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(a) => Value::Array(a.into_iter().map(canonical).collect()),
            v => v,
        }
    }
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(serde_json::to_value(
            value
        )?))?)
    ))
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Grants {
    reviewed: String,
    approved: BTreeMap<String, String>,
}
fn path(root: &Path, id: &str) -> Result<PathBuf> {
    ensure!(peripheral_ui_protocol::identifier(id), "invalid plugin ID");
    let dir = root.join(".peripheral/permissions");
    if dir.exists() {
        ensure!(
            dir.canonicalize()?.starts_with(root.canonicalize()?),
            "permission directory escapes workspace"
        );
    }
    let path = dir.join(format!("{id}.json"));
    if path.exists() {
        ensure!(
            path.canonicalize()?.starts_with(root.canonicalize()?),
            "permission file escapes workspace"
        );
    }
    Ok(path)
}
fn read(root: &Path, id: &str) -> Result<Grants> {
    let path = path(root, id)?;
    if !path.exists() {
        return Ok(Grants::default());
    }
    ensure!(
        std::fs::metadata(&path)?.len() <= 32768,
        "permission file exceeds limit"
    );
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
pub fn describe(root: &Path, manifest: &Manifest) -> Result<Value> {
    let grants = read(root, &manifest.id)?;
    let revision = digest(&manifest.permissions)?;
    let mut ready = true;
    let mut items = Vec::new();
    for request in &manifest.permissions {
        let approved = grants.approved.get(&request.id) == Some(&digest(request)?);
        ready &= !request.required || approved;
        items.push(json!({"request":request,"approved":approved}));
    }
    Ok(
        json!({"revision":revision,"reviewed":manifest.permissions.is_empty() || grants.reviewed == revision,"ready":ready,"items":items}),
    )
}
pub fn policy(root: &Path, manifest: &Manifest) -> Result<Policy> {
    let grants = read(root, &manifest.id)?;
    let mut policy = Policy {
        name: format!("plugin:{}", manifest.id),
        ..Default::default()
    };
    for request in &manifest.permissions {
        if grants.approved.get(&request.id) != Some(&digest(request)?) {
            ensure!(
                !request.required,
                "plugin permission required: {}",
                request.title
            );
            continue;
        }
        policy
            .output_rules
            .extend(request.policy.output_rules.clone());
        policy.hid_rules.extend(request.policy.hid_rules.clone());
        policy
            .ble_write_rules
            .extend(request.policy.ble_write_rules.clone());
        policy
            .ble_pair_rules
            .extend(request.policy.ble_pair_rules.clone());
        policy
            .gip_write_rules
            .extend(request.policy.gip_write_rules.clone());
    }
    Ok(policy)
}
pub fn save(root: &Path, manifest: &Manifest, revision: &str, approved: &[String]) -> Result<()> {
    ensure!(
        revision == digest(&manifest.permissions)?,
        "permissions changed; reopen permission editor"
    );
    ensure!(
        approved.len() <= manifest.permissions.len(),
        "too many grants"
    );
    let mut grants = Grants {
        reviewed: revision.into(),
        ..Default::default()
    };
    for id in approved {
        let request = manifest
            .permissions
            .iter()
            .find(|r| &r.id == id)
            .context("unknown permission ID")?;
        ensure!(
            grants
                .approved
                .insert(id.clone(), digest(request)?)
                .is_none(),
            "duplicate permission ID"
        );
    }
    let path = path(root, &manifest.id)?;
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    let path = self::path(root, &manifest.id)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(&serde_json::to_vec_pretty(&grants)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
