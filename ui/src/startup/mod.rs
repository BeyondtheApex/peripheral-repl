//! Host-owned logon settings and launch behavior.
mod registry;
mod settings;
use crate::app::Application;
use anyhow::{Context, Result};
use gpui_kit::App;
use serde::Serialize;
use serde_json::{Value, json};
pub use settings::panel;

#[derive(Default, Serialize)]
pub struct State {
    pub registered: bool,
    pub current_installation: bool,
    pub system_disabled: bool,
    pub start_in_tray: bool,
    pub error: Option<String>,
}
pub fn state() -> State {
    let tray = std::fs::read(crate::backend::data_dir().join("preferences.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["startup"]["start_in_tray"].as_bool())
        .unwrap_or(true);
    match registry::status() {
        Ok((registered, current_installation, system_disabled)) => State {
            registered,
            current_installation,
            system_disabled,
            start_in_tray: tray,
            error: None,
        },
        Err(error) => State {
            start_in_tray: tray,
            error: Some(error.to_string()),
            ..Default::default()
        },
    }
}
impl Application {
    pub(crate) fn startup_command(&mut self, value: &Value, cx: &mut App) -> Option<Result<Value>> {
        let op = value["op"].as_str()?;
        if ![
            "startup_status",
            "startup_setting",
            "startup_system_settings",
        ]
        .contains(&op)
        {
            return None;
        }
        Some((|| {
            match op {
                "startup_setting" => {
                    let enabled = value["value"]
                        .as_bool()
                        .context("startup setting boolean")?;
                    match value["field"].as_str() {
                        Some("enabled") => {
                            if enabled {
                                // Explorer's working directory is unrelated to the plugin workspace.
                                // Persist the selected workspace before scheduling a logon launch.
                                crate::backend::save_workspace(&crate::backend::workspace())?;
                            }
                            registry::set(enabled)?;
                        }
                        Some("start_in_tray") => crate::dashboard::save_preference(
                            "startup",
                            json!({"start_in_tray":enabled}),
                        )?,
                        _ => anyhow::bail!("unknown startup setting"),
                    }
                }
                "startup_system_settings" => {
                    crate::native::open_file(std::path::Path::new("ms-settings:startupapps"))?
                }
                _ => {}
            }
            let state = state();
            let result = serde_json::to_value(&state)?;
            self.manager(cx, |m, _| m.startup = state);
            Ok(result)
        })())
    }
}
