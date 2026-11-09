//! Built-in plugin lifecycle. A package owns one automatically managed session.
pub mod package;
pub mod permissions;
pub mod resources;
mod snapshot;
use crate::runtime::Runtime;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Mutex};
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Setting {
    pub enabled: bool,
    #[serde(default)]
    pub config: Option<Value>,
    /// Last Lua declaration, for editing a disabled plugin without executing it.
    #[serde(default)]
    pub schema: Option<Value>,
    /// Registration survives a failed restart; telemetry is never persisted as live.
    #[serde(default)]
    pub device: Option<Value>,
}
#[derive(Default)]
pub struct State {
    pub settings: BTreeMap<String, Setting>,
    pub errors: BTreeMap<String, String>,
}
pub struct Manager {
    state: Mutex<State>,
}
impl Manager {
    pub fn new(root: &Path) -> Result<Self> {
        let path = root.join(".peripheral/plugins.json");
        let mut settings: BTreeMap<String, Setting> = if path.exists() {
            let bytes = std::fs::read(path)?;
            anyhow::ensure!(
                bytes.len() <= 1024 * 1024,
                "plugin preferences exceed limit"
            );
            serde_json::from_slice(&bytes)?
        } else {
            BTreeMap::new()
        };
        // Also sanitize preferences written by older builds: display contained
        // translated telemetry even when state itself had already been removed.
        for setting in settings.values_mut() {
            setting.device = setting.device.take().map(snapshot::registration_only);
        }
        Ok(Self {
            state: Mutex::new(State {
                settings,
                errors: BTreeMap::new(),
            }),
        })
    }
    fn save(root: &Path, state: &State) -> Result<()> {
        let dir = root.join(".peripheral");
        std::fs::create_dir_all(&dir)?;
        anyhow::ensure!(
            dir.canonicalize()?.starts_with(root),
            "plugin preferences escape workspace"
        );
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(&dir)?;
        file.write_all(&serde_json::to_vec_pretty(&state.settings)?)?;
        file.as_file().sync_all()?;
        file.persist(dir.join("plugins.json"))
            .map_err(|e| e.error)?;
        Ok(())
    }
    pub fn dispatch(&self, runtime: &Runtime, root: &Path, request: &Value) -> Result<Value> {
        let mut state = self.state.lock().unwrap();
        let op = request["op"].as_str().unwrap_or("");
        if op == "plugin_install" {
            let id = if let Some(source) = request["source"].as_str() {
                resources::install_from(root, Path::new(source))?
            } else {
                let manifest = serde_json::from_value(request["manifest"].clone())?;
                let files = serde_json::from_value(request["files"].clone())?;
                package::install(root, manifest, files)?
            };
            drop(state);
            // Installing is enabling a business unit; startup errors remain visible.
            let result = self.dispatch(
                runtime,
                root,
                &json!({"op":"plugin_enable","plugin":id,"enabled":true}),
            );
            return Ok(
                json!({"installed":id,"permissions":result.as_ref().ok().and_then(|v|v.get("permissions")),"authorization_required":result.as_ref().ok().is_some_and(|v|v["authorization_required"]==true),"startup_error":result.err().map(|e|format!("{e:#}"))}),
            );
        }
        let catalog = package::catalog(root)?;
        if op == "plugins" {
            let mut items = catalog
                .iter()
                .map(|(manifest, _)| snapshot::plugin(runtime, manifest, &state, root))
                .collect::<Vec<_>>();
            items.extend(package::broken(root)?);
            return Ok(json!(items));
        }
        if op == "plugins_restore" {
            let enabled = state
                .settings
                .iter()
                .filter(|(_, s)| s.enabled)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            drop(state);
            for id in enabled {
                let _ = self.dispatch(
                    runtime,
                    root,
                    &json!({"op":"plugin_enable","plugin":id,"enabled":true}),
                );
            }
            return Ok(json!({"restored":true}));
        }
        let id = request["plugin"].as_str().context("plugin ID")?;
        let (manifest, path) = catalog
            .iter()
            .find(|(m, _)| m.id == id)
            .context("plugin not installed")?;
        if op == "plugin_permissions" {
            return permissions::describe(root, manifest);
        }
        if op == "plugin_permissions_save" {
            let revision = request["revision"]
                .as_str()
                .context("permission revision")?;
            let approved: Vec<String> = serde_json::from_value(request["approved"].clone())?;
            // Stop existing device owners before replacing their immutable grant set.
            // An invalid request is rejected before stopping a working plugin.
            let current = permissions::describe(root, manifest)?;
            anyhow::ensure!(
                current["revision"] == revision,
                "permissions changed; reopen permission editor"
            );
            anyhow::ensure!(
                approved.len() <= manifest.permissions.len()
                    && approved
                        .iter()
                        .all(|id| manifest.permissions.iter().any(|r| &r.id == id))
                    && approved
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        == approved.len(),
                "invalid permission selection"
            );
            let instance = session(id);
            let exists = runtime
                .dispatch(json!({"op":"instances"}))?
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == &instance));
            if exists {
                runtime.dispatch(json!({"op":"destroy","instance":instance}))?;
            }
            permissions::save(root, manifest, revision, &approved)?;
            state.errors.remove(id);
            let enabled = state.settings.get(id).is_some_and(|s| s.enabled);
            let result = permissions::describe(root, manifest)?;
            drop(state);
            runtime
                .shared
                .emit(&instance, "plugin.permissions", json!({"plugin":id}));
            let startup_error = if enabled && result["ready"] == true {
                self.dispatch(
                    runtime,
                    root,
                    &json!({"op":"plugin_enable","plugin":id,"enabled":true}),
                )
                .err()
                .map(|e| format!("{e:#}"))
            } else {
                None
            };
            return Ok(json!({"plugin":id,"permissions":result,"startup_error":startup_error}));
        }
        let instance = session(id);
        let previous = state.settings.get(id).cloned().unwrap_or_default();
        let mut setting = previous.clone();
        if let Ok(description) =
            runtime.dispatch(json!({"op":"config_description","instance":instance}))
        {
            setting.config = Some(description["value"].clone());
            setting.schema = description.get("schema").filter(|s| !s.is_null()).cloned();
        }
        let mut config = setting.config.clone().unwrap_or_else(|| json!({}));
        if op == "plugin_configure" {
            config = request["value"].clone();
            let schema = setting
                .schema
                .as_ref()
                .context("plugin has not declared configuration; enable it first")?;
            if let Some(expected) = request.get("schema") {
                anyhow::ensure!(
                    expected == schema,
                    "configuration fields changed; reopen configuration"
                );
            }
            crate::extras::validate(schema, &config)?;
            setting.config = Some(config.clone());
        }
        match op {
            "plugin_enable" => {
                setting.enabled = request["enabled"].as_bool().context("enabled boolean")?
            }
            "plugin_restart" => setting.enabled = true,
            "plugin_configure" => {}
            _ => anyhow::bail!("unknown plugin operation"),
        }
        // Persist intent before starting devices; if this fails no runtime changes occur.
        state.settings.insert(id.into(), setting.clone());
        if let Err(e) = Self::save(root, &state) {
            state.settings.insert(id.into(), previous);
            return Err(e);
        }
        let permission_state = permissions::describe(root, manifest)?;
        if setting.enabled && permission_state["ready"] != true {
            if runtime
                .dispatch(json!({"op":"instances"}))?
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == &instance))
            {
                runtime.dispatch(json!({"op":"destroy","instance":instance}))?;
            }
            state.errors.remove(id);
            runtime
                .shared
                .emit(&instance, "plugin.permissions", json!({"plugin":id}));
            return Ok(
                json!({"plugin":id,"enabled":true,"authorization_required":true,"permissions":permission_state}),
            );
        }
        let result = (|| -> Result<()> {
            let exists = runtime
                .dispatch(json!({"op":"instances"}))?
                .as_array()
                .is_some_and(|names| names.iter().any(|n| n == &json!(instance)));
            if op == "plugin_enable"
                && setting.enabled
                && exists
                && runtime.dispatch(json!({"op":"lifecycle","instance":instance}))?["running"]
                    == true
            {
                return Ok(());
            }
            if op == "plugin_configure" && exists {
                runtime.dispatch(json!({"op":"configure","instance":instance,"value":config,"schema":setting.schema}))?;
                return Ok(());
            }
            if exists {
                runtime.dispatch(json!({"op":"destroy","instance":instance}))?;
            }
            if !setting.enabled {
                return Ok(());
            }
            runtime.create_plugin_session(&instance, path, permissions::policy(root, manifest)?)?;
            let start = (|| -> Result<()> {
                runtime.dispatch(json!({"op":"configure","instance":instance,"value":config}))?;
                let code = crate::runtime::read_source(path, &manifest.entry)?;
                runtime.dispatch(json!({"op":"eval","instance":instance,"code":code,"timeout_ms":5000,"label":manifest.entry}))?;
                runtime.dispatch(json!({"op":"start","instance":instance}))?;
                Ok(())
            })();
            if start.is_err() {
                let _ = runtime.dispatch(json!({"op":"destroy","instance":instance}));
            }
            start
        })();
        match &result {
            Ok(_) => {
                if let Ok(description) =
                    runtime.dispatch(json!({"op":"config_description","instance":instance}))
                {
                    setting.config = Some(description["value"].clone());
                    setting.schema = description.get("schema").filter(|s| !s.is_null()).cloned();
                    setting.device = runtime
                        .dispatch(json!({"op":"device_snapshot","instance":instance}))
                        .ok()
                        .filter(|v| !v.is_null())
                        .map(snapshot::registration_only);
                    state.settings.insert(id.into(), setting.clone());
                    Self::save(root, &state)?;
                }
                state.errors.remove(id);
            }
            Err(error) => {
                if op == "plugin_configure" {
                    state.settings.insert(id.into(), previous);
                    Self::save(root, &state)?;
                }
                state.errors.insert(id.into(), format!("{error:#}"));
            }
        }
        runtime.shared.emit(
            &instance,
            "plugin.changed",
            json!({"plugin":id,"enabled":setting.enabled,"error":state.errors.get(id)}),
        );
        result?;
        Ok(json!({"plugin":id,"enabled":setting.enabled}))
    }
}
pub fn session(id: &str) -> String {
    format!("plugin_{id}")
}

#[cfg(test)]
mod permission_tests;
