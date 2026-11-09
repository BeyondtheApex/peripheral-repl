use super::*;
use crate::{
    control::Request,
    manager::widgets::{button, label, toggle},
};
use serde_json::json;
pub fn panel(sender: &async_channel::Sender<Request>) -> AnyElement {
    let o = options();
    let mut panel = div()
        .p_5()
        .rounded_xl()
        .bg(super::panel("surface"))
        .flex()
        .flex_col()
        .gap_3()
        .child(label("外观与语言"));
    for (field, title, current, choices) in [
        (
            "mode",
            "明暗模式",
            o.mode.as_str(),
            vec![("system", "跟随系统"), ("light", "浅色"), ("dark", "深色")],
        ),
        (
            "language",
            "界面语言",
            o.language.as_str(),
            vec![
                ("system", "跟随系统"),
                ("zh-CN", "简体中文"),
                ("en", "English"),
            ],
        ),
        (
            "material",
            "窗口背景",
            o.material.as_str(),
            vec![
                ("solid", "不透明"),
                ("transparent", "半透明"),
                ("blur", "磨砂模糊"),
                ("mica", "云母"),
            ],
        ),
    ] {
        panel = panel.child(
            div().flex().flex_col().gap_2().child(label(title)).child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(choices.into_iter().map(|(value, title)| {
                        button(
                            format!("appearance-{field}-{value}"),
                            title,
                            if value == current {
                                "circle-check"
                            } else {
                                "circle"
                            },
                            json!({"op":"appearance_setting","field":field,"value":value}),
                            sender,
                        )
                    })),
            ),
        );
    }
    panel = panel.child(
        div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_3()
            .child(label("主题色"))
            .children(
                [
                    ("#8ca8ff", "蓝紫"),
                    ("#2984e3", "蓝色"),
                    ("#159c79", "青绿"),
                    ("#b876e5", "紫色"),
                    ("#db8050", "橙色"),
                ]
                .into_iter()
                .map(|(value, title)| {
                    button(
                        format!("accent-{}", &value[1..]),
                        title,
                        if o.accent == value {
                            "circle-check"
                        } else {
                            "palette"
                        },
                        json!({"op":"appearance_setting","field":"accent","value":value}),
                        sender,
                    )
                }),
            ),
    );
    panel=panel.child(div().flex().items_center().justify_between().child(label("背景不透明度")).child(div().flex().gap_2()
        .child(button("opacity-less","","minus",json!({"op":"appearance_setting","field":"opacity","value":o.opacity.saturating_sub(5).max(50)}),sender))
        .child(label(format!("{}%",o.opacity))).child(button("opacity-more","","plus",json!({"op":"appearance_setting","field":"opacity","value":(o.opacity+5).min(100)}),sender))));
    panel = panel.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(label("隐藏系统标题栏"))
            .child(toggle(
                "hide-titlebar",
                o.hide_titlebar,
                json!({"op":"appearance_setting","field":"hide_titlebar","value":!o.hide_titlebar}),
                sender,
            )),
    );
    panel.child(label("背景效果同时应用于主窗口、托盘速览和弹出菜单；系统禁用透明效果或不支持时使用不透明背景。"))
        .into_any_element()
}
