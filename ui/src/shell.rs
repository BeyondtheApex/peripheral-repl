//! Explorer lifecycle and update orchestration. No shell operations in views.
use crate::{
    app::{Application, send},
    updates,
};
use anyhow::Result;
use gpui_kit::App;
use serde_json::{Value, json};
impl Application {
    pub fn start_watch(&self) {
        let sender = self.commands.clone();
        std::thread::spawn(move || {
            loop {
                let pid = updates::explorer_pid();
                let modules = pid.map(updates::loaded_modules).transpose();
                let root = updates::root().ok();
                send(
                    &sender,
                    json!({"op":"shell_status","explorer_pid":pid,"modules":modules.as_ref().ok().and_then(|m|m.as_ref()),"error":modules.err().map(|e|format!("{e:#}")),"dll":crate::distribution::dll_path().ok(),"pending":root.as_ref().and_then(|r|updates::pending(r)),"update_result":root.as_ref().and_then(|r|updates::result(r))}),
                );
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        });
    }
    pub fn shell_command(&mut self, v: &Value, cx: &mut App) -> Option<Result<Value>> {
        match v["op"].as_str()? {
            "taskbar_enable" => Some((|| {
                self.taskbar_enabled = v["enabled"].as_bool().unwrap_or(false);
                crate::dashboard::save_preference("taskbar_enabled", json!(self.taskbar_enabled))?;
                if self.surfaces.contains_key("taskbar") {
                    self.close("taskbar", cx)?;
                }
                self.reconcile(cx)?;
                Ok(json!({"enabled":self.taskbar_enabled}))
            })()),
            "shell_status" => Some((|| {
                let pid = v["explorer_pid"].as_u64().map(|p| p as u32);
                let changed = self.explorer.is_some() && pid.is_some() && self.explorer != pid;
                if pid.is_some() {
                    self.explorer = pid;
                }
                if changed {
                    if self.surfaces.contains_key("taskbar") {
                        self.close("taskbar", cx)?;
                    }
                    if v["pending"].is_object() {
                        self.restart_or_update(false, cx)?;
                    } else {
                        self.reconcile(cx)?;
                    }
                }
                let state = if !self.taskbar_enabled {
                    "已关闭"
                } else if let Some(b) = self.surfaces.get("taskbar").and_then(|s| s.bridge.as_ref())
                {
                    b.state.as_str()
                } else {
                    "等待插件发布任务栏内容"
                };
                let mut status = v.clone();
                status["state"] = json!(match state {
                    "ready" => "已挂载",
                    "starting" => "正在挂载",
                    "failed" => "挂载失败",
                    other => other,
                });
                status["enabled"] = json!(self.taskbar_enabled);
                status["compatibility"] = json!("Windows 11 实验性集成");
                if status != self.shell {
                    self.shell = status.clone();
                    self.manager(cx, |m, _| m.shell = status);
                }
                Ok(json!({}))
            })()),
            _ => None,
        }
    }
    pub fn restart_or_update(&mut self, restart: bool, cx: &mut App) -> Result<()> {
        if updates::pending(&updates::root()?).is_some() {
            // A running backend may be mapping this EXE. Do not silently discard
            // live Lua/device state merely to install an update.
            if let Ok(bytes) = std::fs::read(&self.backend.endpoint)
                && let Ok(endpoint) =
                    serde_json::from_slice::<peripheral_repl::protocol::Endpoint>(&bytes)
                && crate::native::process_alive(endpoint.pid)
            {
                anyhow::bail!(
                    "Stop the backend from Sessions before applying this update; running Lua state is not serializable. The prepared package is retained."
                );
            }
            updates::launch_apply(restart, self.taskbar_enabled)?;
            self.surface_command(&json!({"op":"shutdown"}), cx)?;
        } else {
            updates::launch_restart()?;
        }
        Ok(())
    }
}
