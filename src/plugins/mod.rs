//! Built-in plugin lifecycle. A package owns one automatically managed session.
pub mod package;
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
        let settings = if path.exists() {
            let bytes = std::fs::read(path)?;
            anyhow::ensure!(
                bytes.len() <= 1024 * 1024,
                "plugin preferences exceed limit"
            );
            serde_json::from_slice(&bytes)?
        } else {
            BTreeMap::new()
        };
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
                json!({"installed":id,"startup_error":result.err().map(|e|format!("{e:#}"))}),
            );
        }
        let catalog = package::catalog(root)?;
        if op == "plugins" {
            let mut items = catalog
                .iter()
                .map(|(manifest, _)| snapshot::plugin(runtime, manifest, &state))
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
            runtime.create_plugin_session(&instance, path)?;
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
                        .map(|mut v| {
                            v["state"] = json!({});
                            v["stale"] = json!(true);
                            v
                        });
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
