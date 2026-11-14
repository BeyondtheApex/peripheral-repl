use super::*;
impl Manager {
    pub(super) fn settings(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_3xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::i18n::tr("设置")),
            )
            .child(label("后台连接、插件存放位置和系统显示选项。"))
            .child(crate::appearance::settings::panel(sender))
            .child(crate::startup::panel(&self.startup, sender))
            .child(self.backend_privileges(sender))
            .child(
                div()
                    .p_5()
                    .rounded_xl()
                    .bg(crate::appearance::color(PANEL))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(crate::i18n::tr("插件工作目录"))
                    .child(label(self.workspace.clone()))
                    .child(label(format!(
                        "默认 REPL 策略：{}",
                        self.backend["ping"]["policy"].as_str().unwrap_or("未连接")
                    )))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(button(
                                "workspace",
                                "更改目录…",
                                "folder",
                                json!({"op":"pick_workspace"}),
                                sender,
                            ))
                            .child(button(
                                "refresh",
                                "重新连接",
                                "refresh-cw",
                                json!({"op":"backend_command","command":{"op":"refresh"}}),
                                sender,
                            )),
                    ),
            )
            .child(self.dashboard_settings(sender))
            .child(self.shell(sender))
            .into_any_element()
    }
    fn backend_privileges(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        let connected = self.backend["connection"] == "connected";
        let elevated = connected && self.backend["ping"]["host"]["elevated"] == true;
        let start_elevated = self
            .backend_preferences
            .as_ref()
            .is_ok_and(|p| p.start_elevated);
        let mut panel=div().p_5().rounded_xl().bg(crate::appearance::color(PANEL)).flex().flex_col().gap_3()
            .child(crate::i18n::tr("后台设备访问权限"))
            .child(label(if !connected{"后台未连接"}else if elevated{"当前后台：管理员"}else{"当前后台：普通权限"}))
            .child(label("部分 GIP 手柄需要管理员后台才能持续接收数据。GUI 保持普通权限；设备写入仍需逐插件授权。"))
            .child(div().flex().items_center().justify_between().gap_3()
                .child(label("默认以管理员权限启动后台"))
                .child(toggle("backend-start-elevated",start_elevated,
                    json!({"op":"backend_preferences","start_elevated":!start_elevated}),sender)))
            .child(label("用于手动启动和登录自启动，下次启动后台时生效；仍需确认 Windows 权限提示。重启和应用更新保持当前后台权限。"));
        if let Err(error) = &self.backend_preferences {
            panel = panel.child(label(error.clone()));
        }
        if !elevated && !self.backend_status.busy() {
            panel = panel.child(button(
                "elevate-backend",
                "以管理员权限启动后台",
                "shield-check",
                json!({"op":"backend_command","command":{"op":"elevate_backend"}}),
                sender,
            ));
        }
        panel
            .child(label(
                "取消系统授权不会停止现有后台。恢复普通权限请关闭默认管理员启动，再停止并启动后台。",
            ))
            .into_any_element()
    }
}
