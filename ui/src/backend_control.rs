//! Backend lifecycle commands and their shared tray/sidebar presentation.
use crate::{app::Application, backend_status::BackendStatus, native};
use anyhow::{Context, Result, ensure};
use gpui_kit::App;
use serde_json::{Value, json};
impl Application {
    pub fn backend_preferences_command(
        &mut self,
        v: &Value,
        cx: &mut App,
    ) -> Option<Result<Value>> {
        if v["op"] != "backend_preferences" {
            return None;
        }
        Some((|| {
            let preferences = crate::backend::preferences::Preferences {
                start_elevated: v["start_elevated"]
                    .as_bool()
                    .context("start_elevated must be a boolean")?,
            };
            preferences.save()?;
            self.manager(cx, |m, _| m.backend_preferences = Ok(preferences));
            self.notify("success", "后台启动设置已保存，下次启动后台时生效", cx);
            Ok(json!({"preferences":preferences}))
        })())
    }
    pub fn set_backend_status(&mut self, status: BackendStatus, cx: &mut App) -> Result<()> {
        let changed = self.backend_status != status;
        if let Some(main) = self.surfaces.get("main") {
            native::set_backend_tray(main.hwnd, status, cx)?;
        }
        self.backend_status = status;
        if let Some(menu) = self.surfaces.get(crate::app_menu::ID) {
            menu.view.update(cx, |v, cx| {
                v.app_menu = Some(status);
                cx.notify();
            });
        }
        if changed && status == BackendStatus::Running {
            self.backend
                .command(json!({"op":"ui_environment","value":crate::appearance::environment()}))?;
        }
        self.manager(cx, |m, _| m.backend_status = status);
        Ok(())
    }
    pub fn control_backend(&mut self, action: &str, cx: &mut App) -> Result<()> {
        ensure!(
            !self.shell_busy,
            "更新或资源管理器重启进行中，请稍后操作后台"
        );
        ensure!(!self.backend_status.busy(), "后台操作进行中");
        // Explicit update recovery preserves the previous privilege mode.
        // Only a fresh manual/login start follows the user's preference.
        let action = if action == "start" {
            crate::backend::preferences::Preferences::load()?.start_action()
        } else {
            action
        };
        let (op, status) = match action {
            "start" | "start_normal" => ("start_backend", BackendStatus::Starting),
            "stop" => ("stop_backend", BackendStatus::Stopping),
            "restart" => ("restart_backend", BackendStatus::Restarting),
            "elevate" => ("elevate_backend", BackendStatus::Restarting),
            _ => anyhow::bail!("unknown backend action"),
        };
        self.backend.command(json!({"op":op}))?;
        self.set_backend_status(status, cx)
    }
}
