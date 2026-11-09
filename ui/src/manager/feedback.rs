//! Copy affordances share successful state across document list and reader.
use super::*;
use std::time::Duration;

impl Manager {
    pub(super) fn copy_button(
        &self,
        id: impl Into<SharedString>,
        title: &str,
        key: &str,
        body: Value,
        sender: &async_channel::Sender<Request>,
    ) -> AnyElement {
        let graphic = if let Some(revision) = self.copied.get(key) {
            div()
                .size(px(17.))
                .flex()
                .items_center()
                .justify_center()
                .with_animation(
                    SharedString::from(format!("copied-{key}-{revision}")),
                    Animation::new(Duration::from_millis(220)).with_max_fps(60.),
                    |d, t| {
                        let ease = 1. - (1. - t).powi(3);
                        d.child(
                            svg()
                                .path("icons/check.svg")
                                .size(px(11. + 6. * ease))
                                .opacity(ease)
                                .text_color(crate::appearance::token("success")),
                        )
                    },
                )
                .into_any_element()
        } else {
            icon("copy").into_any_element()
        };
        widgets::button_with_icon(id, title, graphic, body, sender)
    }
}
