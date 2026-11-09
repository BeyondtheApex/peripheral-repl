//! Session operations and native pickers, kept out of the renderer/window layer.
use crate::app::{Application, send};
use crate::backend_status::BackendStatus;
use anyhow::{Context, Result};
use gpui_kit::*;
use serde_json::{Value, json};
impl Application {
    pub fn management_command(&mut self, v: &Value, cx: &mut App) -> Option<Result<Value>> {
        let op = v["op"].as_str()?;
        if ![
            "backend_command",
            "backend_result",
            "backend_state",
            "backend_ui",
            "backend_ui_ready",
            "backend_events",
            "backend_disconnected",
            "select_session",
            "eval_selected",
            "configure_selected",
            "manager_confirm",
            "manager_confirmed",
            "pick_workspace",
            "install_plugin_picker",
            "pick_update",
            "picked_path",
            "update_staged",
            "tray_action",
        ]
        .contains(&op)
        {
            return None;
        }
        Some((|| -> Result<Value> {
            match op {
                "backend_command" => match v["command"]["op"].as_str() {
                    Some("start_backend") => self.control_backend("start", cx)?,
                    Some("stop_backend" | "shutdown") => self.control_backend("stop", cx)?,
                    Some("restart_backend") => self.control_backend("restart", cx)?,
                    _ => self.backend.command(v["command"].clone())?,
                },
                "backend_result" => {
                    let operation = v["command"]["op"].as_str().unwrap_or("");
                    if ["start_backend", "stop_backend", "restart_backend"].contains(&operation) {
                        self.set_backend_status(
                            if v["ok"] != true {
                                BackendStatus::Error
                            } else if operation == "stop_backend" {
                                BackendStatus::Stopped
                            } else {
                                BackendStatus::Running
                            },
                            cx,
                        )?;
                        if operation == "stop_backend" && v["ok"] == true {
                            self.accept_publications(&json!({"surfaces":[]}), cx)?;
                            self.manager(cx, |m, _| {
                                m.backend["connection"] = json!("disconnected")
                            });
                        }
                    } else if ["connect", "refresh"].contains(&operation)
                        && v["ok"] != true
                        && !self.backend_status.busy()
                    {
                        self.set_backend_status(BackendStatus::Error, cx)?;
                    }
                    if v["ok"] == true && v["result"]["connection"] == "connected" {
                        self.accept_state(v["result"].clone(), cx)?;
                    }
                    if v["ok"] == true && v["command"]["op"] == "plugin_configure" {
                        self.manager(cx, |m, _| {
                            m.plugin_config = None;
                            m.transition();
                        });
                    }
                    self.manager(cx, |m, _| {
                        m.log(format!(
                            "{}: {}",
                            v["command"]["op"].as_str().unwrap_or("command"),
                            if v["ok"] == true && v["result"]["connection"] == "connected" {
                                "已连接 · 插件状态已刷新".into()
                            } else if v["ok"] == true {
                                v["result"].to_string()
                            } else {
                                v["error"].to_string()
                            }
                        ));
                        m.status = if v["ok"] == true {
                            "就绪".into()
                        } else {
                            v["error"].as_str().unwrap_or("Command failed").into()
                        };
                    });
                    if v["command"]["op"] == "workspace" && v["ok"] == true {
                        self.manager(cx, |m, _| {
                            m.workspace = v["result"]["workspace"].as_str().unwrap_or("").into()
                        });
                    }
                }
                "backend_state" => self.accept_state(v["state"].clone(), cx)?,
                "backend_ui_ready" => {
                    if let Some(snapshot) = self.backend.take_ui() {
                        self.accept_publications(&snapshot, cx)?;
                    }
                }
                "backend_ui" => self.accept_publications(&v["snapshot"], cx)?,
                "backend_events" => {
                    self.manager(cx, |m, _| {
                        for event in v["events"].as_array().into_iter().flatten() {
                            if event["kind"] == "device.changed"
                                && let Some(plugins) = m.backend["plugins"].as_array_mut()
                            {
                                for plugin in plugins.iter_mut().filter(|p| {
                                    p["instance"] == event["instance"] && p["enabled"] == true
                                }) {
                                    // Ignore queued events from a destroyed/restarted session.
                                    if plugin["device"].is_null()
                                        || plugin["device"]["generation"]
                                            == event["data"]["generation"]
                                    {
                                        plugin["device"] = event["data"].clone();
                                    }
                                }
                            }
                            if event["kind"] == "state"
                                && let Some(plugins) = m.backend["plugins"].as_array_mut()
                            {
                                for plugin in plugins
                                    .iter_mut()
                                    .filter(|p| p["instance"] == event["instance"])
                                {
                                    if let Some(key) = event["data"]["key"].as_str() {
                                        plugin["devices"][key] = event["data"]["value"].clone();
                                    }
                                }
                            }
                            if !event["kind"].as_str().is_some_and(|s| s.starts_with("ui.")) {
                                m.log(event.to_string());
                            }
                        }
                    });
                    self.schedule_device_expiry(cx);
                }
                "backend_disconnected" => {
                    self.device_expiry.clear();
                    if !self.backend_status.busy() {
                        self.set_backend_status(
                            if v["alive"] == true {
                                BackendStatus::Disconnected
                            } else {
                                BackendStatus::Stopped
                            },
                            cx,
                        )?;
                    }
                    self.accept_publications(&json!({"surfaces":[]}), cx)?;
                    self.manager(cx, |m, _| {
                        m.backend["connection"] = json!("disconnected");
                        m.status = v["error"].as_str().unwrap_or("后台未连接").into();
                    });
                }
                "select_session" => self.manager(cx, |m, _| {
                    m.confirm = None;
                    m.selected = v["instance"].as_str().unwrap_or("default").into()
                }),
                "eval_selected" | "configure_selected" => {
                    let m = self.surfaces["main"]
                        .view
                        .read(cx)
                        .manager
                        .as_ref()
                        .context("manager")?;
                    let command = match op {
                        "eval_selected" => {
                            json!({"op":"eval","instance":m.selected,"code":m.code.read(cx).value()})
                        }
                        _ => {
                            json!({"op":"configure","instance":m.selected,"value":serde_json::from_str::<Value>(&m.config.read(cx).value())?})
                        }
                    };
                    self.backend.command(command)?;
                }
                "manager_confirm" => self.manager(cx, |m, _| {
                    m.confirm = v["action"].as_str().map(str::to_owned)
                }),
                "manager_confirmed" => {
                    let m = self.surfaces["main"]
                        .view
                        .read(cx)
                        .manager
                        .as_ref()
                        .context("manager")?;
                    let action = v["action"].as_str().context("confirmation action")?;
                    anyhow::ensure!(
                        m.confirm.as_deref() == Some(action),
                        "confirmation must be shown before applying"
                    );
                    match action {
                        "shutdown_backend" => self.control_backend("stop", cx)?,
                        "reset" | "destroy" => self
                            .backend
                            .command(json!({"op":action,"instance":m.selected}))?,
                        "restart_explorer" => self.restart_or_update(true, cx)?,
                        _ => anyhow::bail!("invalid confirmation"),
                    }
                    self.manager(cx, |m, _| m.confirm = None);
                }
                "pick_workspace" | "install_plugin_picker" | "pick_update" => {
                    let paths = cx.prompt_for_paths(PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: None,
                    });
                    let sender = self.commands.clone();
                    let purpose = op.to_owned();
                    cx.spawn(async move |_| {
                        if let Ok(Ok(Some(paths))) = paths.await
                            && let Some(path) = paths.first()
                        {
                            send(
                                &sender,
                                json!({"op":"picked_path","purpose":purpose,"path":path}),
                            );
                        }
                    })
                    .detach();
                }
                "picked_path" => match v["purpose"].as_str() {
                    Some("pick_workspace") => self
                        .backend
                        .command(json!({"op":"workspace","path":v["path"]}))?,
                    Some("install_plugin_picker") => self
                        .backend
                        .command(json!({"op":"install_plugin","path":v["path"]}))?,
                    Some("pick_update") => {
                        let path =
                            std::path::PathBuf::from(v["path"].as_str().context("package folder")?);
                        let root = crate::updates::root()?;
                        let sender = self.commands.clone();
                        std::thread::spawn(move || {
                            let result = crate::updates::stage(&path, &root);
                            send(
                                &sender,
                                json!({"op":"update_staged","error":result.err().map(|e|format!("{e:#}"))}),
                            );
                        });
                        self.status("Verifying and preparing update…".into(), cx);
                    }
                    _ => anyhow::bail!("invalid picker purpose"),
                },
                "update_staged" => {
                    if let Some(error) = v["error"].as_str() {
                        anyhow::bail!("{error}");
                    }
                    self.status("Update prepared. Restart Explorer to apply.".into(), cx);
                }
                "tray_action" => {
                    if v["surface"].as_str().unwrap_or("main") == "main" {
                        if v["button"] == "right" {
                            return self
                                .app_menu_command(&json!({"op":"app_menu_toggle"}), cx)
                                .unwrap();
                        }
                        return self
                            .dashboard_command(&json!({"op":"dashboard_toggle"}), cx)
                            .unwrap();
                    }
                    if let Some(s) = self.surfaces.get(v["surface"].as_str().unwrap_or("main")) {
                        if let Some(p) = s.view.read(cx).publications.first() {
                            let button = v["button"].as_str().unwrap_or("left");
                            anyhow::ensure!(
                                ["left", "right"].contains(&button),
                                "unsupported tray button"
                            );
                            if p.surface.events.action(button).is_some()
                                || (button == "left" && p.surface.action.is_some())
                            {
                                self.backend.command(crate::presentation::input::request(
                                    p, None, button,
                                ))?;
                            }
                        } else {
                            crate::native::show(s.hwnd, true);
                        }
                    }
                }
                _ => {}
            }
            Ok(json!({"queued":true}))
        })())
    }
    fn accept_state(&mut self, state: Value, cx: &mut App) -> Result<()> {
        if !self.backend_status.busy() {
            self.set_backend_status(BackendStatus::Running, cx)?;
        }
        let identity = state["ping"]["runtime_id"].as_str().unwrap_or("");
        if self.runtime_identity != identity {
            self.revision = None;
            self.runtime_identity = identity.into();
        }
        let presentation = self.accept_publications(&state["ui"], cx);
        self.manager(cx, |m, _| {
            if let Some(root) = state["ping"]["root"].as_str() {
                m.workspace = root.into();
            }
            m.backend = state;
        });
        let main = &self.surfaces["main"];
        let view = main.view.clone();
        main.window.update(cx, |_, window, cx| {
            view.update(cx, |v, cx| {
                if let Some(manager) = &mut v.manager
                    && let Some(editor) = &manager.plugin_config
                    && let Some(plugin) = manager.backend["plugins"]
                        .as_array()
                        .and_then(|p| p.iter().find(|p| p["id"] == editor.id))
                    && plugin["config_schema"] != editor.schema
                {
                    manager.plugin_config = Some(crate::manager::configuration::Editor::new(
                        plugin, window, cx,
                    ));
                    manager.status = "插件已更新配置项，请按新的选项设置。".into();
                    cx.notify();
                }
            })
        })?;
        self.refresh_dashboard(cx)?;
        self.schedule_device_expiry(cx);
        presentation
    }
}
