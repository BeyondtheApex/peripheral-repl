//! Explorer lifecycle and update orchestration. No shell operations in views.
use crate::{
    app::{Application, send},
    updates,
};
use anyhow::Result;
use gpui_kit::App;
use serde_json::{Value, json};
impl Application {
    fn refresh_taskbar_target(&mut self, cx: &mut App) -> Result<()> {
        if self.shell_busy {
            return Ok(());
        }
        let target = crate::taskbar::primary_target();
        if target != self.taskbar_target {
            self.taskbar_target = target;
            if self.surfaces.contains_key("taskbar") {
                self.close("taskbar", cx)?;
            }
            self.reconcile(cx)?;
        }
        Ok(())
    }
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
            "shell_worker_finished" => Some((|| {
                let updating = v["updating"] == true;
                if let Some(error) = v["error"].as_str() {
                    self.shell_busy = false;
                    let resume: crate::backend::Resume =
                        serde_json::from_value(v["resume_backend"].clone())?;
                    if let Some(action) = resume.action() {
                        self.control_backend(action, cx)?;
                    }
                    self.reconcile(cx)?;
                    anyhow::bail!("{error}");
                }
                if updating {
                    self.surface_command(&json!({"op":"shutdown"}), cx)?;
                } else {
                    self.shell_busy = false;
                    self.taskbar_target = crate::taskbar::primary_target();
                    self.reconcile(cx)?;
                    self.notify("success", "资源管理器已重启", cx);
                }
                Ok(json!({}))
            })()),
            "taskbar_display_changed" => Some(self.refresh_taskbar_target(cx).map(|_| json!({}))),
            "taskbar_side" => Some((|| {
                let side: crate::taskbar::Side = serde_json::from_value(v["side"].clone())?;
                if side == self.taskbar_side {
                    return Ok(json!({"side":side}));
                }
                crate::dashboard::save_preference("taskbar_side", json!(side))?;
                self.taskbar_side = side;
                self.shell["side"] = json!(side);
                self.manager(cx, |m, _| m.shell["side"] = json!(side));
                if self.surfaces.contains_key("taskbar") {
                    self.close("taskbar", cx)?;
                }
                self.reconcile(cx)?;
                Ok(json!({"side":side}))
            })()),
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
                // Watcher snapshots can predate staging completion. Never let a
                // stale snapshot temporarily re-enable the old DLL.
                self.update_pending = updates::root()
                    .ok()
                    .is_some_and(|r| updates::pending(&r).is_some());
                if changed && !self.shell_busy {
                    // Explorer replacement already recreates the bridge below.
                    self.taskbar_target = crate::taskbar::primary_target();
                    if self.surfaces.contains_key("taskbar") {
                        self.close("taskbar", cx)?;
                    }
                    if self.update_pending {
                        self.restart_or_update(false, cx)?;
                    } else {
                        self.reconcile(cx)?;
                    }
                }
                self.refresh_taskbar_target(cx)?;
                let state = if self.shell_busy {
                    "正在重启或更新"
                } else if self.update_pending {
                    "更新待应用，已暂停挂载旧版本"
                } else if !self.taskbar_enabled {
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
                status["busy"] = json!(self.shell_busy);
                // Watcher data may have been queued before staging/recovery.
                status["pending"] = updates::root()
                    .ok()
                    .and_then(|r| updates::pending(&r))
                    .unwrap_or(Value::Null);
                status["executable"] = json!(std::env::current_exe().ok());
                status["enabled"] = json!(self.taskbar_enabled);
                status["side"] = json!(self.taskbar_side);
                status["target"] = self.taskbar_target.clone();
                status["layout"] = self
                    .surfaces
                    .get("taskbar")
                    .and_then(|s| s.bridge.as_ref())
                    .and_then(|b| std::fs::read(b.dir.join("consumer.json")).ok())
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .map(|v| v["layout"].clone())
                    .unwrap_or(Value::Null);
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
        anyhow::ensure!(
            !self.shell_busy,
            "Explorer restart or update is already in progress"
        );
        anyhow::ensure!(
            !self.backend_status.busy(),
            "Wait for the current backend start/stop operation to finish before updating or restarting Explorer"
        );
        let root = updates::root()?;
        let updating = updates::pending(&root).is_some();
        self.update_pending = updating;
        if self.surfaces.contains_key("taskbar") {
            self.close("taskbar", cx)?;
        }
        self.shell_busy = true;
        self.notify(
            "info",
            if updating {
                "正在停止后台并准备应用更新…"
            } else {
                "正在重启资源管理器…"
            },
            cx,
        );
        let endpoint = self.backend.endpoint.clone();
        let mount = self.taskbar_enabled;
        let sender = self.commands.clone();
        std::thread::spawn(move || {
            let mut resume_backend = crate::backend::Resume::Stopped;
            let result = (|| -> Result<()> {
                if updating {
                    updates::record_preparation(&root)?;
                    resume_backend = crate::backend::stop_for_update(&endpoint, &root)?;
                    updates::launch_apply(restart, mount, resume_backend)?;
                } else {
                    updates::launch_restart()?;
                }
                Ok(())
            })();
            if updating && let Err(error) = &result {
                updates::record_preparation_error(&root, error);
            }
            send(
                &sender,
                json!({"op":"shell_worker_finished","updating":updating,"resume_backend":resume_backend,
                "error":result.err().map(|e|format!("{e:#}"))}),
            );
        });
        Ok(())
    }
}
