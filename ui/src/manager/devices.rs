//! Host-owned device grid. Lua owns identity, telemetry, labels and chosen artwork.
use super::*;
use peripheral_ui_protocol::{ImageSpec, ResourceRef};
impl Manager {
    pub(super) fn devices(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        let plugins = self.backend["plugins"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["enabled"] == true)
            .collect::<Vec<_>>();
        let mut grid = div().flex().flex_wrap().gap_4().items_start();
        for plugin in &plugins {
            grid = grid.child(self.device_card(plugin, sender));
        }
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(crate::i18n::tr("我的设备")),
            )
            .child(label(format!(
                "{} 个已启用设备 · 离线时保留，停用插件后移除",
                plugins.len()
            )))
            .child(if plugins.is_empty() {
                div()
                    .p_6()
                    .rounded_xl()
                    .bg(crate::appearance::color(PANEL))
                    .child(label("尚未启用设备插件。前往“我的插件”安装并启用。"))
            } else {
                grid
            })
            .into_any_element()
    }
    fn device_card(&self, plugin: &Value, sender: &async_channel::Sender<Request>) -> AnyElement {
        let id = plugin["id"].as_str().unwrap_or("");
        let device = &plugin["device"];
        let state = &device["state"];
        let display = &device["display"];
        let live = self.backend["connection"] == "connected"
            && plugin["running"] == true
            && device["stale"] != true;
        let name = display["name"]
            .as_str()
            .or(device["descriptor"]["name"].as_str())
            .or(plugin["name"].as_str())
            .unwrap_or("设备");
        let (color, status) = if self.backend["connection"] != "connected" {
            (MUTED, "后台未连接 · 状态待确认")
        } else if plugin["state"] == "error" {
            (0xf08075, "插件异常 · 状态待确认")
        } else if plugin["running"] != true {
            (MUTED, "插件已停止")
        } else if device.is_null() {
            (MUTED, "等待插件登记设备")
        } else if device["stale"] == true {
            (0xe8bc71, "数据已过期 · 等待更新")
        } else {
            (
                if state["connected"] == true {
                    0x36cc8b
                } else {
                    MUTED
                },
                display["label"]
                    .as_str()
                    .or(state["label"].as_str())
                    .filter(|s| !s.is_empty())
                    .or(state["status"].as_str())
                    .unwrap_or("等待状态"),
            )
        };
        let width = self.dashboard_options.device_card_width as f32;
        let height = self.dashboard_options.device_card_height as f32;
        let inner = width - 32.;
        let artwork_height = (height * 0.32).min(150.);
        let image = serde_json::from_value::<ImageSpec>(device["image"].clone()).ok();
        let resource = serde_json::from_value::<ResourceRef>(device["resource"].clone()).ok();
        let artwork = if let Some(image) = image {
            crate::presentation::images::element(&image, resource.as_ref(), inner, artwork_height)
        } else {
            div()
                .w(px(inner))
                .h(px(artwork_height))
                .rounded_lg()
                .bg(crate::appearance::color(0x222d3d))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("plug"))
                .into_any_element()
        };
        let frame = || {
            div()
                .w(px(width))
                .h(px(height))
                .flex_shrink_0()
                .p_4()
                .rounded_xl()
                .bg(crate::appearance::color(PANEL))
                .flex()
                .flex_col()
                .gap_3()
                .overflow_hidden()
        };
        let title = || {
            div()
                .flex()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name.to_owned()),
                )
                .child(button(
                    format!("device-config-{id}"),
                    "",
                    "settings",
                    json!({"op":"plugin_config_open","plugin":id}),
                    sender,
                ))
        };
        let status_line = || {
            div()
                .flex()
                .items_center()
                .gap_2()
                .flex_shrink_0()
                .child(
                    div()
                        .size(px(7.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(crate::appearance::color(color)),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(crate::appearance::color(color))
                        .truncate()
                        .child(crate::i18n::tr(status)),
                )
        };
        let mut front = frame()
            .child(artwork)
            .child(title())
            .child(
                div()
                    .text_xs()
                    .text_color(crate::appearance::color(MUTED))
                    .truncate()
                    .child(crate::i18n::tr(&format!(
                        "来自 {}",
                        plugin["name"].as_str().unwrap_or(id)
                    ))),
            )
            .child(status_line());
        if let Some(battery) = state["battery_percent"].as_f64() {
            let bar = (inner - 76.).max(60.);
            front = front.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(bar))
                            .h(px(5.))
                            .rounded_full()
                            .bg(crate::appearance::color(0x354156))
                            .child(
                                div()
                                    .w(px(bar * battery.clamp(0., 100.) as f32 / 100.))
                                    .h_full()
                                    .rounded_full()
                                    .bg(crate::appearance::color(color)),
                            ),
                    )
                    .child(div().text_sm().child(format!(
                        "{}{battery:.0}%",
                        if live {
                            ""
                        } else {
                            crate::i18n::text("上次 ")
                        }
                    ))),
            );
        } else {
            front = front.child(label(if live {
                "设备电量未知"
            } else {
                "等待有效测量"
            }));
        }
        front = front
            .child(div().flex_1())
            .child(div().flex().justify_end().child(button(
                format!("device-more-{id}"),
                "详情",
                "rotate-3d",
                json!({"op":"device_flip","plugin":id}),
                sender,
            )));
        let mut details = div().flex().flex_col().gap_3();
        if let Some(present) = state["receiver_present"].as_bool() {
            details = details.child(label(format!(
                "{}{}",
                if live { "" } else { "上次：" },
                if present {
                    "接收器已连接"
                } else {
                    "接收器未连接"
                }
            )));
        }
        let display_values = display["values"].as_array().cloned().unwrap_or_else(|| {
            state["values"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(key, value)| json!({"label":key,"value":value}))
                .collect()
        });
        {
            for entry in &display_values {
                let key = entry["label"].as_str().unwrap_or("");
                let value = &entry["value"];
                let text = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
                details = details.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_sm()
                        .child(label(key.to_owned()))
                        .child(text),
                );
            }
        }
        if let Some(detail) = plugin["error"]
            .as_str()
            .or(display["detail"].as_str())
            .or(state["detail"].as_str())
            .filter(|s| !s.is_empty())
        {
            details = details.child(label(detail.to_owned()));
        }
        if device["stale"] == true
            && let Some(last) = display["label"].as_str().or(state["label"].as_str())
        {
            details = details.child(label(format!("上次状态：{last}")));
        }
        let back = frame()
            .child(title())
            .child(status_line())
            .child(
                div()
                    .id(SharedString::from(format!("device-details-{id}")))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(details),
            )
            .child(div().flex().justify_end().child(button(
                format!("device-back-{id}"),
                "返回",
                "rotate-3d",
                json!({"op":"device_flip","plugin":id}),
                sender,
            )));
        let (show_back, revision) = self.device_faces.get(id).copied().unwrap_or_default();
        let faces = flip::Flip {
            front: front.into_any_element(),
            back: back.into_any_element(),
            show_back,
            phase: 1.,
            width,
            height,
        };
        if revision == 0 {
            faces.into_any_element()
        } else {
            faces
                .with_animation(
                    SharedString::from(format!("flip-{id}-{revision}")),
                    Animation::new(std::time::Duration::from_millis(360)).with_max_fps(30.),
                    |mut faces, phase| {
                        faces.phase = phase;
                        faces
                    },
                )
                .into_any_element()
        }
    }
}
