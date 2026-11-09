use super::*;
use crate::{app::Application, model::SurfaceKind, native};
use anyhow::Result;
use serde_json::{Value, json};
impl Application {
    pub(crate) fn appearance_command(&mut self, v: &Value, cx: &mut App) -> Option<Result<Value>> {
        let op = v["op"].as_str()?;
        if ![
            "appearance_setting",
            "appearance_system_changed",
            "window_action",
        ]
        .contains(&op)
        {
            return None;
        }
        Some((|| {
            if op == "window_action" {
                let action = v["action"].as_str().unwrap_or("");
                anyhow::ensure!(
                    ["minimize", "maximize", "hide"].contains(&action),
                    "invalid window action"
                );
                if let Some(s) = self.surfaces.get("main") {
                    native::appearance::window_action(s.hwnd, action);
                }
                return Ok(json!({}));
            }
            let mut options = options();
            let previous_environment = environment();
            if op == "appearance_system_changed" && !system_changed() {
                return Ok(json!({"changed":false}));
            }
            if op == "appearance_setting" {
                let field = v["field"].as_str().unwrap_or("");
                anyhow::ensure!(
                    [
                        "mode",
                        "accent",
                        "language",
                        "material",
                        "opacity",
                        "hide_titlebar"
                    ]
                    .contains(&field),
                    "invalid appearance field"
                );
                let mut value = serde_json::to_value(&options)?;
                value[field] = v["value"].clone();
                options = serde_json::from_value(value)?;
                options.validate()?;
                crate::dashboard::save_preference("appearance", serde_json::to_value(&options)?)?;
            }
            apply(&options, cx);
            for (id, s) in &self.surfaces {
                let popup = s.view.read(cx).kind == SurfaceKind::Popup;
                if id == "main" || popup {
                    s.window.update(cx, |_, w, _| {
                        w.set_background_appearance(window_background(popup))
                    })?;
                }
                native::appearance::apply(s.hwnd, id == "main", popup, &options);
                if id == "main" {
                    s.window.update(cx, |_, w, _| {
                        w.set_window_title(&crate::i18n::tr("Peripheral · 插件管理"))
                    })?;
                }
                s.view.update(cx, |_, cx| cx.notify());
                s.window.update(cx, |_, w, _| w.refresh())?;
            }
            self.manager(cx, |m, _| m.transition());
            if self.backend_status == crate::backend_status::BackendStatus::Running
                && previous_environment != environment()
            {
                self.backend
                    .command(json!({"op":"ui_environment","value":environment()}))?;
            }
            Ok(
                json!({"options":options,"environment":environment(),"material_available":material_enabled()}),
            )
        })())
    }
}
