use super::State;
use crate::{
    control::Request,
    manager::widgets::{button, label, toggle},
};
use gpui_kit::*;
use serde_json::json;

pub fn panel(state: &State, sender: &async_channel::Sender<Request>) -> AnyElement {
    let mut panel = div().p_5().rounded_xl().bg(crate::appearance::panel("surface"))
        .flex().flex_col().gap_3()
        .child(crate::i18n::tr("启动与登录"))
        .child(div().flex().items_center().justify_between().gap_3()
            .child(label("登录后自动启动"))
            .child(toggle("startup-enabled",state.registered,
                json!({"op":"startup_setting","field":"enabled","value":!state.registered}),sender)))
        .child(div().flex().items_center().justify_between().gap_3()
            .child(label("自动启动时收起到托盘"))
            .child(toggle("startup-tray",state.start_in_tray,
                json!({"op":"startup_setting","field":"start_in_tray","value":!state.start_in_tray}),sender)))
        .child(label("仅对当前 Windows 用户生效。自动启动时连接或启动后台，并恢复已启用的插件。"));
    if let Some(error) = &state.error {
        panel = panel.child(label(error.clone()));
    }
    if state.registered && !state.current_installation {
        panel = panel
            .child(label("自启动指向其他安装位置，可修复为当前程序。"))
            .child(button(
                "startup-repair",
                "使用当前程序",
                "refresh-cw",
                json!({"op":"startup_setting","field":"enabled","value":true}),
                sender,
            ));
    }
    if state.registered && state.system_disabled {
        panel = panel
            .child(label("Windows 已禁用此启动项，请在系统启动设置中启用。"))
            .child(button(
                "startup-system-settings",
                "打开系统启动设置",
                "external-link",
                json!({"op":"startup_system_settings"}),
                sender,
            ));
    }
    panel.into_any_element()
}
