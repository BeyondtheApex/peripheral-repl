//! Native application menu; plugin tray menus stay owned by Lua.
use super::*;
use crate::backend_status::BackendStatus;
#[derive(serde::Serialize)]
pub(super) struct Item {
    id: usize,
    title: &'static str,
    enabled: bool,
}
pub(super) fn items(status: BackendStatus) -> Vec<Item> {
    let running = status == BackendStatus::Running;
    let mut entries = vec![
        Item {
            id: 1,
            title: "打开 Peripheral",
            enabled: true,
        },
        Item {
            id: 3,
            title: if running {
                "停止后台"
            } else {
                "启动后台"
            },
            enabled: !status.busy(),
        },
        Item {
            id: 4,
            title: "重启后台",
            enabled: running,
        },
        Item {
            id: 5,
            title: "重新连接后台",
            enabled: !status.busy(),
        },
        Item {
            id: 2,
            title: "退出界面（保留后台）",
            enabled: true,
        },
    ];
    for entry in &mut entries {
        entry.title = crate::i18n::text(entry.title);
    }
    entries
}
pub(super) fn show(_hwnd: HWND, h: &Hook) {
    enqueue(h, json!({"op":"app_menu_toggle"}));
}
