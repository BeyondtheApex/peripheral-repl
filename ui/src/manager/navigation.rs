use super::*;
impl Manager {
    pub(super) fn navigation(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        let connected = self.backend_status == crate::backend_status::BackendStatus::Running;
        div()
            .w(px(208.))
            .h_full()
            .flex_shrink_0()
            .px_5()
            .py_6()
            .flex()
            .flex_col()
            .gap_3()
            .bg(crate::appearance::color(0x151a22))
            .child(
                div()
                    .id("sidebar-message-slot")
                    .h(px(140.))
                    .w_full()
                    .flex_shrink_0()
                    .child(match &self.notice {
                        Some(notice) => notice.render(sender),
                        None => div()
                            .size_full()
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(icon("cpu"))
                            .child("Peripheral")
                            .into_any_element(),
                    }),
            )
            .children(
                [
                    ("plugins", "我的插件", "blocks"),
                    ("devices", "我的设备", "monitor"),
                    ("repl", "调试控制台", "terminal"),
                ]
                .into_iter()
                .map(|(page, title, ico)| {
                    let body = json!({"op":"navigate","page":page});
                    let sender = sender.clone();
                    div()
                        .id(SharedString::from(format!("nav-{page}")))
                        .rounded_lg()
                        .px_3()
                        .py_3()
                        .flex()
                        .items_center()
                        .gap_3()
                        .text_sm()
                        .cursor_pointer()
                        .bg(crate::appearance::color(if self.page == page {
                            0x29374e
                        } else {
                            0x151a22
                        }))
                        .hover(|s| s.bg(crate::appearance::interactive_hover()))
                        .active(|s| s.bg(crate::appearance::token("border")))
                        .on_click(move |_, _, _| send(&sender, body.clone()))
                        .child(icon(ico))
                        .child(crate::i18n::tr(title))
                }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .px_3()
                    .py_3()
                    .rounded_lg()
                    .bg(crate::appearance::color(PANEL))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(7.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(crate::appearance::color(self.backend_status.color())),
                            )
                            .child(div().text_xs().child(self.backend_status.label())),
                    )
                    .child(div().flex_shrink_0().child(toggle(
                        "backend-power",
                        connected,
                        json!({"op":"backend_toggle"}),
                        sender,
                    ))),
            )
            .child(button(
                "settings",
                "设置",
                "settings",
                json!({"op":"navigate","page":"settings"}),
                sender,
            ))
            .child(button(
                "diagnostics",
                "诊断信息",
                "file-text",
                json!({"op":"installation_report"}),
                sender,
            ))
            .into_any_element()
    }
}
