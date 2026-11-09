//! User-facing plugin controls; session IDs stay behind this boundary.
use crate::{app::Application, manager::configuration::Editor};
use anyhow::{Context, Result};
use gpui_kit::*;
use serde_json::{Value, json};
impl Application {
    pub fn plugin_command(&mut self, v: &Value, cx: &mut App) -> Option<Result<Value>> {
        let op = v["op"].as_str()?;
        if ![
            "navigate",
            "device_flip",
            "backend_toggle",
            "plugin_toggle",
            "plugin_restart",
            "plugin_config_open",
            "plugin_config_close",
            "plugin_config_option",
            "plugin_config_save",
            "plugin_details",
        ]
        .contains(&op)
        {
            return None;
        }
        Some((|| -> Result<Value> {
            match op {
                "device_flip" => {
                    let id = v["plugin"].as_str().context("plugin ID")?;
                    self.manager(cx, |m, _| {
                        let entry = m.device_faces.entry(id.into()).or_insert((false, 0));
                        entry.0 = !entry.0;
                        entry.1 = entry.1.wrapping_add(1);
                    });
                }
                "plugin_details" => {
                    let id = v["plugin"].as_str().context("plugin ID")?;
                    self.manager(cx, |m, _| {
                        if !m.expanded_plugins.remove(id) {
                            m.expanded_plugins.insert(id.to_owned());
                        }
                    });
                }
                "navigate" => {
                    let page = v["page"].as_str().context("page")?;
                    anyhow::ensure!(
                        ["plugins", "devices", "repl", "settings"].contains(&page),
                        "unknown page"
                    );
                    self.manager(cx, |m, _| {
                        if m.page != page {
                            m.transition();
                        }
                        m.page = page.into();
                        if page == "settings" {
                            m.startup = crate::startup::state();
                        }
                        m.confirm = None;
                    });
                }
                "backend_toggle" => {
                    if self.backend_status.busy() {
                        return Ok(json!({"queued":false}));
                    }
                    let connected = self.surfaces["main"]
                        .view
                        .read(cx)
                        .manager
                        .as_ref()
                        .is_some_and(|m| m.backend["connection"] == "connected");
                    if connected {
                        self.manager(cx, |m, _| m.confirm = Some("shutdown_backend".into()));
                    } else {
                        self.control_backend("start", cx)?;
                    }
                }
                "plugin_toggle" | "plugin_restart" => {
                    let id = v["plugin"].as_str().context("plugin ID")?;
                    self.backend.command(if op == "plugin_toggle" {
                        json!({"op":"plugin_enable","plugin":id,"enabled":v["enabled"]})
                    } else {
                        json!({"op":"plugin_restart","plugin":id})
                    })?;
                    self.status("正在更新插件运行状态…".into(), cx);
                }
                "plugin_config_open" => {
                    let id = v["plugin"].as_str().context("plugin ID")?.to_owned();
                    let surface = &self.surfaces["main"];
                    let view = surface.view.clone();
                    surface.window.update(cx, |_, window, cx| {
                        view.update(cx, |v, cx| {
                            let manager = v.manager.as_mut().context("manager")?;
                            let plugin = manager.backend["plugins"]
                                .as_array()
                                .and_then(|p| p.iter().find(|p| p["id"] == id))
                                .context("plugin not found")?
                                .clone();
                            manager.page = "plugins".into();
                            manager.transition();
                            manager.permission_editor = None;
                            manager.plugin_config = Some(Editor::new(&plugin, window, cx));
                            cx.notify();
                            Ok::<(), anyhow::Error>(())
                        })
                    })??;
                }
                "plugin_config_close" => self.manager(cx, |m, _| {
                    m.plugin_config = None;
                    m.transition();
                }),
                "plugin_config_option" => {
                    let key = v["key"].as_str().context("field key")?;
                    let new_value = v["value"].clone();
                    let surface = &self.surfaces["main"];
                    let view = surface.view.clone();
                    surface.window.update(cx, |_, window, cx| {
                        view.update(cx, |v, cx| -> Result<()> {
                            let editor = v
                                .manager
                                .as_mut()
                                .context("manager")?
                                .plugin_config
                                .as_mut()
                                .context("configuration not open")?;
                            anyhow::ensure!(!editor.saving, "configuration save in progress");
                            let field = editor
                                .fields
                                .iter()
                                .find(|f| f.key == key)
                                .context("unknown configuration field")?;
                            if field.schema["enum"].is_array() || field.schema["type"] == "boolean"
                            {
                                peripheral_repl::extras::validate(&field.schema, &new_value)?;
                                editor.values[key] = new_value.clone();
                            } else {
                                let text = new_value
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| new_value.to_string());
                                field
                                    .input
                                    .update(cx, |input, cx| input.set_value(text, window, cx));
                            }
                            editor.errors.remove(key);
                            cx.notify();
                            Ok(())
                        })
                    })??;
                }
                "plugin_config_save" => {
                    anyhow::ensure!(
                        self.backend_status == crate::backend_status::BackendStatus::Running,
                        "{}",
                        crate::i18n::tr("请先启动后台")
                    );
                    let view = self.surfaces["main"].view.clone();
                    let command=view.update(cx,|v,cx| -> Result<Option<Value>> {
                        let editor=v.manager.as_mut().context("manager")?.plugin_config.as_mut().context("configuration not open")?;
                        if editor.saving { return Ok(None); }
                        let result=editor.value(cx);
                        cx.notify();
                        let value=result?;
                        Ok(Some(json!({"op":"plugin_configure","plugin":editor.id,"value":value,"schema":editor.schema})))
                    })?;
                    if let Some(command) = command {
                        self.backend.command(command)?;
                        self.manager(cx, |m, _| {
                            if let Some(e) = &mut m.plugin_config {
                                e.saving = true;
                            }
                        });
                        self.notify("info", "正在保存…", cx);
                    }
                }
                _ => {}
            }
            Ok(json!({"queued":true}))
        })())
    }
}
