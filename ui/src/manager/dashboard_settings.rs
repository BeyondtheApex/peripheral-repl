use super::*;
use crate::dashboard::{Alignment, Mode};
impl Manager {
    pub(super) fn dashboard_settings(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        let o = &self.dashboard_options;
        let mut panel = div()
            .p_5()
            .rounded_xl()
            .bg(crate::appearance::color(PANEL))
            .flex()
            .flex_col()
            .gap_3()
            .child(crate::i18n::tr("托盘左键 · 插件速览"))
            .child(label(
                "插件分别提供项目和卡片内容。未提供当前视图的插件不会显示。",
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(button(
                        "dashboard-list",
                        "项目视图",
                        if o.mode == Mode::List {
                            "circle-check"
                        } else {
                            "list"
                        },
                        json!({"op":"dashboard_settings","field":"mode","value":"list"}),
                        sender,
                    ))
                    .child(button(
                        "dashboard-grid",
                        "卡片视图",
                        if o.mode == Mode::Grid {
                            "circle-check"
                        } else {
                            "layout-grid"
                        },
                        json!({"op":"dashboard_settings","field":"mode","value":"grid"}),
                        sender,
                    )),
            );
        for (field, title, current, labels) in [
            (
                "horizontal_alignment",
                "水平对齐",
                o.horizontal_alignment,
                ["左对齐", "居中", "右对齐"],
            ),
            (
                "vertical_alignment",
                "垂直对齐",
                o.vertical_alignment,
                ["顶部", "居中", "底部"],
            ),
        ] {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(label(title))
                    .child(
                        div().flex().gap_2().children(
                            [
                                (Alignment::Start, "start"),
                                (Alignment::Center, "center"),
                                (Alignment::End, "end"),
                            ]
                            .into_iter()
                            .zip(labels)
                            .map(|((value, key), title)| {
                                button(
                                    format!("dashboard-{field}-{key}"),
                                    title,
                                    if current == value {
                                        "circle-check"
                                    } else {
                                        "circle"
                                    },
                                    json!({"op":"dashboard_settings","field":field,"value":key}),
                                    sender,
                                )
                            }),
                        ),
                    ),
            );
        }
        let mut fields = vec![
            ("width", "弹窗宽度", o.width, 40, 320, 960),
            ("height", "弹窗高度", o.height, 40, 240, 800),
        ];
        if o.mode == Mode::List {
            fields.push(("row_height", "项目行高", o.row_height, 4, 32, 64));
        } else {
            fields.extend([
                ("rows", "卡片行数", o.rows, 1, 1, 8),
                ("columns", "卡片列数", o.columns, 1, 1, 4),
                ("card_width", "速览卡片宽度", o.card_width, 20, 100, 400),
                ("card_height", "速览卡片高度", o.card_height, 20, 80, 360),
            ]);
        }
        fields.extend([
            (
                "device_card_width",
                "设备页卡片宽度",
                o.device_card_width,
                16,
                240,
                400,
            ),
            (
                "device_card_height",
                "设备页卡片高度",
                o.device_card_height,
                20,
                300,
                480,
            ),
        ]);
        for (field, title, value, step, min, max) in fields {
            panel=panel.child(div().flex().items_center().justify_between()
                .child(label(title)).child(div().flex().items_center().gap_3()
                    .child(button(format!("dashboard-{field}-less"),"","minus",json!({"op":"dashboard_settings","field":field,"value":value.saturating_sub(step).max(min)}),sender))
                    .child(div().w(px(48.)).text_sm().text_center().child(value.to_string()))
                    .child(button(format!("dashboard-{field}-more"),"","plus",json!({"op":"dashboard_settings","field":field,"value":(value+step).min(max)}),sender))));
        }
        panel
            .child(label(
                "尺寸为逻辑像素。速览按指定卡片尺寸和行列布局；空间不足时整体等比缩小，超出项滚动。设备卡片统一尺寸，详情在背面。",
            ))
            .child(div().flex().child(button(
                "dashboard-preview",
                "预览弹窗",
                "panel-top",
                json!({"op":"dashboard_toggle"}),
                sender,
            )))
            .into_any_element()
    }
}
