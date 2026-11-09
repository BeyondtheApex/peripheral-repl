use super::*;
impl Manager {
    pub(super) fn plugins(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        if let Some(editor) = &self.plugin_config {
            return editor.render(sender);
        }
        let plugins = self.backend["plugins"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let running = plugins.iter().filter(|p| p["running"] == true).count();
        let mut page = div().flex().flex_col().gap_3().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .mb_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .text_2xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(crate::i18n::tr("我的插件")),
                        )
                        .child(label(format!(
                            "{} 项 · {} 项运行中",
                            plugins.len(),
                            running
                        ))),
                )
                .child(button(
                    "install-plugin",
                    "安装插件…",
                    "plus",
                    json!({"op":"install_plugin_picker"}),
                    sender,
                )),
        );
        if plugins.is_empty() {
            page = page.child(
                div()
                    .p_6()
                    .rounded_lg()
                    .bg(crate::appearance::color(PANEL))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(crate::i18n::tr("从安装一个插件开始"))
                    .child(label("选择包含 plugin.json 的插件文件夹，即可安装并启用。")),
            );
        }
        for plugin in plugins {
            let id = plugin["id"].as_str().unwrap_or("");
            let enabled = plugin["enabled"] == true;
            let running = plugin["running"] == true;
            let failed = plugin["state"] == "error";
            let expanded = self.expanded_plugins.contains(id);
            let (color, status) = if failed {
                (0xf19b7c, "需要处理")
            } else if running {
                (0x3ed99a, "运行中")
            } else if enabled {
                (MUTED, "待启动")
            } else {
                (MUTED, "已停用")
            };
            let mut card = div()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .px_4()
                .py_3()
                .rounded_lg()
                .bg(crate::appearance::color(PANEL))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .size(px(32.))
                                .flex_shrink_0()
                                .rounded_lg()
                                .bg(crate::appearance::color(0x253247))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon("plug")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_base()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(
                                                    plugin["name"]
                                                        .as_str()
                                                        .unwrap_or(id)
                                                        .to_owned(),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_xs()
                                                .text_color(crate::appearance::color(color))
                                                .child(crate::i18n::tr(status)),
                                        ),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(crate::appearance::color(MUTED))
                                        .child(crate::i18n::tr(&summary(&plugin))),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(button(
                                    format!("config-{id}"),
                                    "配置",
                                    "settings-2",
                                    json!({"op":"plugin_config_open","plugin":id}),
                                    sender,
                                ))
                                .child(button(
                                    format!("restart-{id}"),
                                    "重启",
                                    "rotate-cw",
                                    json!({"op":"plugin_restart","plugin":id}),
                                    sender,
                                ))
                                .child(button(
                                    format!("details-{id}"),
                                    if expanded { "收起" } else { "详情" },
                                    if expanded {
                                        "chevron-up"
                                    } else {
                                        "chevron-down"
                                    },
                                    json!({"op":"plugin_details","plugin":id}),
                                    sender,
                                ))
                                .child(div().ml_2().child(toggle(
                                    format!("toggle-{id}"),
                                    enabled,
                                    json!({"op":"plugin_toggle","plugin":id,"enabled":!enabled}),
                                    sender,
                                ))),
                        ),
                );
            if let Some(error) = plugin["error"].as_str() {
                card = card.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(crate::appearance::color(0x3d2b2c))
                        .text_sm()
                        .child(error.to_owned()),
                );
            }
            card = card.child(motion::reveal(
                format!("plugin-details-{id}"),
                expanded,
                div()
                    .pt_2()
                    .border_t_1()
                    .border_color(crate::appearance::color(0x2a3342))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(label(format!(
                        "{} · v{}",
                        plugin["description"].as_str().unwrap_or(""),
                        plugin["version"].as_str().unwrap_or("")
                    )))
                    .child(super::plugin_details::details(&plugin))
                    .into_any_element(),
            ));
            // Plugin-owned business panels remain visible; only management metadata collapses.
            let views = plugin["views"].as_array().cloned().unwrap_or_default();
            for view in views.iter().filter(|v| {
                v["target"] == "panel" && v["publication"]["surface"]["visible"] != false
            }) {
                if let Ok(publication) = serde_json::from_value::<peripheral_repl::ui::Publication>(
                    view["publication"].clone(),
                ) {
                    card = card.child(
                        div()
                            .id(SharedString::from(format!(
                                "plugin-panel-{id}-{}",
                                publication.id
                            )))
                            .w_full()
                            .min_w_0()
                            .overflow_x_scroll()
                            .child(crate::presentation::document(&publication, sender.clone())),
                    );
                }
            }

            page = page.child(card);
        }
        page.into_any_element()
    }
}
fn summary(plugin: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(devices) = plugin["devices"].as_object() {
        for (key, value) in devices.iter().take(2) {
            let mut text = value["name"].as_str().unwrap_or(key).to_owned();
            if let Some(battery) = value.get("battery") {
                text.push_str(&format!(
                    " {}{}",
                    battery,
                    value["unit"].as_str().unwrap_or("%")
                ));
            }
            if let Some(status) = value["status"].as_str() {
                text.push_str(&format!(" · {status}"));
            } else if let Some(connected) = value["connected"].as_bool() {
                text.push_str(crate::i18n::text(if connected {
                    " · 已连接"
                } else {
                    " · 未连接"
                }));
            }
            parts.push(text);
        }
        if devices.len() > 2 {
            parts.push(crate::i18n::tr(&format!("另 {} 台设备", devices.len() - 2)));
        }
    }
    let mut targets = Vec::new();
    for view in plugin["views"].as_array().into_iter().flatten() {
        if view["publication"]["surface"]["visible"] == false {
            continue;
        }
        let name = target_name(view["target"].as_str().unwrap_or(""));
        if !targets.contains(&name) {
            targets.push(name);
        }
    }
    if !targets.is_empty() {
        parts.push(targets.join(" / "));
    }
    if parts.is_empty() {
        plugin["description"]
            .as_str()
            .unwrap_or("尚无设备状态")
            .to_owned()
    } else {
        parts.join(" · ")
    }
}
pub(super) fn target_name(target: &str) -> &str {
    crate::i18n::text(match target {
        "taskbar" => "任务栏",
        "tray" => "托盘图标",
        "overlay" => "桌面浮层",
        "window" => "独立窗口",
        "panel" => "插件面板",
        "popup" => "弹出菜单",
        "tray_list" => "速览项目",
        "tray_card" => "速览卡片",
        "taskbar_preview" => "任务栏预览",
        "texture" | "xaml" => "独立表面",
        _ => return target,
    })
}
