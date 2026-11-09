use super::*;
impl Manager {
    pub(super) fn titlebar(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        div()
            .h(px(38.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .child(
                div()
                    .id("window-drag")
                    .flex_1()
                    .h_full()
                    .px_4()
                    .flex()
                    .items_center()
                    .window_control_area(WindowControlArea::Drag)
                    .child(label("Peripheral")),
            )
            .children(
                [("minimize", "minus"), ("maximize", "square"), ("hide", "x")]
                    .into_iter()
                    .map(|(action, icon)| {
                        let sender = sender.clone();
                        let group = SharedString::from(format!("caption-{action}"));
                        div()
                            .id(group.clone())
                            .group(group.clone())
                            .w(px(44.))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(move |_, _, _| {
                                send(&sender, json!({"op":"window_action","action":action}));
                            })
                            .child(
                                svg()
                                    .path(SharedString::from(format!("icons/{icon}.svg")))
                                    .size(px(17.))
                                    // GPUI Svg paints only with an explicit local color.
                                    .text_color(crate::appearance::token("muted"))
                                    .group_hover(group, |s| {
                                        s.text_color(crate::appearance::token(
                                            if action == "hide" { "error" } else { "text" },
                                        ))
                                    }),
                            )
                    }),
            )
            .into_any_element()
    }
}
