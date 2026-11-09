//! Persistent errors and timed success messages in a fixed sidebar slot.
use crate::{
    app::Application,
    control::Request,
    manager::widgets::{button, icon},
};
use gpui_kit::*;
use serde_json::json;
pub struct Notice {
    pub revision: u64,
    pub message: String,
    pub kind: &'static str,
}
impl Notice {
    pub fn render(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        let (token, glyph, title) = match self.kind {
            "error" => ("error", "circle-alert", "操作未完成"),
            "success" => ("success", "circle-check", "操作成功"),
            _ => ("accent", "info", "操作提示"),
        };
        div()
            .id("operation-notice")
            .size_full()
            .p_2()
            .rounded_lg()
            .border_1()
            .border_color(crate::appearance::token(token))
            .bg(crate::appearance::token("surface"))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .child(icon(glyph).text_color(crate::appearance::token(token)))
                    .child(div().flex_1().text_sm().child(crate::i18n::tr(title)))
                    .child(button(
                        "dismiss-notice",
                        "",
                        "x",
                        json!({"op":"notice_dismiss","revision":self.revision}),
                        sender,
                    )),
            )
            .child(
                div()
                    .id("operation-notice-scroll")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .text_sm()
                    .child(crate::i18n::tr(&self.message)),
            )
            .into_any_element()
    }
}
impl Application {
    pub fn notify(&mut self, kind: &'static str, message: impl Into<String>, cx: &mut App) {
        let message = message.into();
        let mut revision = 0;
        self.manager(cx, |m, _| {
            m.notice_revision = m.notice_revision.wrapping_add(1);
            revision = m.notice_revision;
            m.status = message.clone();
            m.notice = Some(Notice {
                revision,
                message,
                kind,
            });
        });
        if kind != "error" {
            let sender = self.commands.clone();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(7))
                    .await;
                crate::app::send(&sender, json!({"op":"notice_dismiss","revision":revision}));
            })
            .detach();
        }
    }
}
