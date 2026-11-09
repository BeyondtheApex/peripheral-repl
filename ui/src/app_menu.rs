//! Application-owned tray context menu shares theme, blur and popup dismissal with Lua menus.
use crate::{
    app::Application, backend_status::BackendStatus, control::Request, model::SurfaceKind, native,
};
use gpui_kit::*;
use serde_json::{Value, json};
pub const ID: &str = "app-tray-menu";
pub fn render(status: BackendStatus, sender: &async_channel::Sender<Request>) -> AnyElement {
    let running = status == BackendStatus::Running;
    let mut menu = div()
        .size_full()
        .rounded(px(12.))
        .bg(crate::appearance::popup_background(None))
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .overflow_hidden()
        .child(
            div()
                .px_3()
                .py_2()
                .text_sm()
                .text_color(crate::appearance::token("muted"))
                .child(status.label()),
        );
    for (title, icon, enabled, command) in [
        (
            "打开 Peripheral",
            "panel-top",
            true,
            json!({"op":"show","surface":"main"}),
        ),
        (
            if running {
                "停止后台"
            } else {
                "启动后台"
            },
            if running { "square" } else { "play" },
            !status.busy(),
            json!({"op":"backend_control","action":if running{"stop"}else{"start"}}),
        ),
        (
            "重启后台",
            "rotate-cw",
            running,
            json!({"op":"backend_control","action":"restart"}),
        ),
        (
            "重新连接后台",
            "plug",
            !status.busy(),
            json!({"op":"backend_command","command":{"op":"refresh"}}),
        ),
        (
            "退出界面（保留后台）",
            "log-out",
            true,
            json!({"op":"shutdown"}),
        ),
    ] {
        let sender = sender.clone();
        let row = div()
            .id(SharedString::from(format!("menu-{icon}")))
            .h(px(36.))
            .px_3()
            .rounded_md()
            .flex()
            .items_center()
            .gap_3()
            .text_sm()
            .text_color(crate::appearance::token("text"))
            .opacity(if enabled { 1. } else { 0.45 })
            .child(crate::manager::widgets::icon(icon))
            .child(crate::i18n::tr(title));
        menu = menu.child(if enabled {
            row.cursor_pointer()
                .hover(|s| s.bg(crate::appearance::token("elevated")))
                .on_click(move |_, _, _| {
                    let (tx, _) = std::sync::mpsc::sync_channel(1);
                    let _ = sender.try_send(Request {
                        body: json!({"op":"app_menu_action","command":command}),
                        reply: tx,
                        expires: None,
                    });
                })
        } else {
            row
        });
    }
    crate::manager::motion::popup_enter("app-menu-enter", menu.into_any_element())
}
impl Application {
    pub(crate) fn app_menu_command(
        &mut self,
        v: &Value,
        cx: &mut App,
    ) -> Option<anyhow::Result<Value>> {
        match v["op"].as_str()? {
            "app_menu_toggle" => Some((|| {
                if self.surfaces.contains_key(ID) {
                    self.close(ID, cx)?;
                } else {
                    self.open(ID, SurfaceKind::Popup, (260., 260.), cx)?;
                    let surface = &self.surfaces[ID];
                    surface.view.update(cx, |v, cx| {
                        v.app_menu = Some(self.backend_status);
                        cx.notify();
                    });
                    native::popup::register(surface.hwnd, ID.into())?;
                    native::popup::show_at(surface.hwnd, None)?;
                }
                Ok(json!({}))
            })()),
            "app_menu_action" => Some((|| {
                if self.surfaces.contains_key(ID) {
                    self.close(ID, cx)?;
                }
                self.command(&v["command"], cx)
            })()),
            _ => None,
        }
    }
}
