use super::plugins::target_name;
use super::*;
pub(super) fn details(plugin: &Value) -> AnyElement {
    let owner = plugin["id"].as_str().unwrap_or("plugin");
    let mut card = div()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap_2();
    let views = plugin["views"].as_array().cloned().unwrap_or_default();
    for (key, value) in plugin["devices"].as_object().into_iter().flatten() {
        let name = value["name"].as_str().unwrap_or(key);
        let mut device = div()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .p_3()
            .rounded_lg()
            .bg(crate::appearance::color(BG))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name.to_owned()),
            );
        if let Some(fields) = value.as_object() {
            device = device.child(
                div().w_full().min_w_0().flex().flex_col().gap_3().children(
                    fields
                        .iter()
                        .filter(|(k, _)| !k.starts_with('_') && k.as_str() != "name")
                        .map(|(key, value)| {
                            let title = match key.as_str() {
                                "battery" => "电量",
                                "connected" => "设备连接",
                                "status" => "状态",
                                "charging" => "充电",
                                "unit" => "单位",
                                other => other,
                            };
                            let value = if key == "connected" {
                                if value == true {
                                    "已连接".into()
                                } else {
                                    "未连接".into()
                                }
                            } else {
                                value.as_str().map(str::to_owned).unwrap_or_else(|| {
                                    serde_json::to_string_pretty(value).unwrap_or_default()
                                })
                            };
                            div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .gap_3()
                                .items_start()
                                .child(
                                    label(title.to_owned())
                                        .w(px(84.))
                                        .flex_shrink_0()
                                        .truncate(),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "telemetry-{owner}-{name}-{key}"
                                        )))
                                        .flex_1()
                                        .min_w_0()
                                        .max_h(px(144.))
                                        .overflow_scroll()
                                        .child(div().flex().flex_col().children(
                                            value.lines().map(|line| {
                                                label(line.to_owned())
                                                    .whitespace_nowrap()
                                                    .flex_shrink_0()
                                            }),
                                        )),
                                )
                        }),
                ),
            );
        } else {
            device = device.child(
                div()
                    .id(SharedString::from(format!("telemetry-{owner}-{key}")))
                    .w_full()
                    .min_w_0()
                    .max_h(px(144.))
                    .overflow_scroll()
                    .child(label(value.to_string()).whitespace_nowrap()),
            );
        }
        card = card.child(device);
    }

    if !views.is_empty() {
        card = card.child(
            div()
                .flex()
                .w_full()
                .min_w_0()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(label("由此插件显示"))
                .children(views.iter().map(|v| {
                    div()
                        .max_w_full()
                        .min_w_0()
                        .truncate()
                        .px_3()
                        .py_1()
                        .rounded_lg()
                        .bg(crate::appearance::color(0x28334a))
                        .text_xs()
                        .child(format!(
                            "{} · {}",
                            target_name(v["target"].as_str().unwrap_or("")),
                            v["title"].as_str().unwrap_or("")
                        ))
                })),
        );
    }

    card.into_any_element()
}
