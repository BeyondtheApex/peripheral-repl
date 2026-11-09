//! Backend lifecycle commands and their shared tray/sidebar presentation.
use crate::{app::Application, backend_status::BackendStatus, native};
use anyhow::{Result, ensure};
use gpui_kit::App;
use serde_json::json;
impl Application {
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
        ensure!(!self.backend_status.busy(), "后台操作进行中");
        let (op, status) = match action {
            "start" => ("start_backend", BackendStatus::Starting),
            "stop" => ("stop_backend", BackendStatus::Stopping),
            "restart" => ("restart_backend", BackendStatus::Restarting),
            _ => anyhow::bail!("unknown backend action"),
        };
        self.backend.command(json!({"op":op}))?;
        self.set_backend_status(status, cx)
    }
}
