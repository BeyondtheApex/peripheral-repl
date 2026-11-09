use super::*;
impl Manager {
    pub(super) fn repl(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        if let Some(language) = &self.guide {
            return self.guide_reader(language, sender);
        }
        let editor = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(crate::appearance::color(PANEL))
            .rounded_xl()
            .child(
                div()
                    .text_3xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::i18n::tr("调试控制台")),
            )
            .child(label(
                "选择插件后，直接在它的运行环境中调试。普通使用无需操作此页。",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button(
                        "default-session",
                        "独立调试环境",
                        "terminal",
                        json!({"op":"select_session","instance":"default"}),
                        sender,
                    ))
                    .children(
                        self.backend["plugins"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|p| p["running"] == true)
                            .map(|p| {
                                button(
                                    format!("repl-{}", p["id"]),
                                    p["name"].as_str().unwrap_or("插件"),
                                    "plug",
                                    json!({"op":"select_session","instance":p["instance"]}),
                                    sender,
                                )
                            }),
                    ),
            )
            .child(label(format!("当前环境：{}", self.selected)))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(div().flex_1().child(Input::new(&self.code)))
                    .child(button(
                        "eval",
                        "执行",
                        "terminal",
                        json!({"op":"eval_selected"}),
                        sender,
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(div().flex_1().child(Input::new(&self.config)))
                    .child(button(
                        "configure",
                        "应用 JSON",
                        "settings",
                        json!({"op":"configure_selected"}),
                        sender,
                    )),
            )
            .child(
                div()
                    .id("repl-output")
                    .h(px(105.))
                    .overflow_y_scroll()
                    .bg(crate::appearance::color(BG))
                    .rounded_lg()
                    .p_3()
                    .font_family("Cascadia Code")
                    .text_xs()
                    .children(
                        self.output
                            .iter()
                            .rev()
                            .take(30)
                            .rev()
                            .map(|s| div().child(s.clone())),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(editor)
            .child(self.ai_setup(sender))
            .child(self.guide_list(sender))
            .into_any_element()
    }
}
