//! Application-owned tray dashboard. Lua contributes items, never its window layout settings.
mod commands;
mod options;
#[cfg(test)]
mod tests;
use crate::{control::Request, presentation};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
pub use options::{Alignment, Mode, Options, save_preference};
use peripheral_repl::ui::Publication;
use peripheral_ui_protocol::Target;
use serde_json::json;
use std::collections::BTreeMap;
pub const ID: &str = "tray-dashboard";
#[derive(Default)]
pub struct Dashboard {
    pub options: Options,
    pub owners: BTreeMap<String, String>,
}
impl Dashboard {
    pub fn render(
        &self,
        items: &[Publication],
        sender: &async_channel::Sender<Request>,
        bounds: Size<Pixels>,
    ) -> AnyElement {
        let cols = if self.options.mode == Mode::List {
            1
        } else {
            self.options.columns.min(items.len().max(1) as u32)
        };
        let list = self.options.mode == Mode::List;
        let available_width = (f32::from(bounds.width) - 32.).max(1.);
        let available_height = (f32::from(bounds.height) - 108.).max(1.);
        let visible_rows = self
            .options
            .rows
            .min((items.len().max(1) as u32).div_ceil(cols));
        let scale = if list {
            1.
        } else {
            (available_width
                / (cols as f32 * self.options.card_width as f32 + (cols - 1) as f32 * 8.))
                .min(
                    available_height
                        / (visible_rows as f32 * self.options.card_height as f32
                            + (visible_rows - 1) as f32 * 8.),
                )
                .min(1.)
        };
        let width = if list {
            available_width
        } else {
            self.options.card_width as f32 * scale
        };
        let height = if list {
            self.options.row_height as f32
        } else {
            self.options.card_height as f32 * scale
        };
        let mut content = div()
            // Align the complete grid as one block in the outer container.
            // Per-line flex centering would shift an incomplete final row.
            .grid()
            .grid_cols(cols as u16)
            .flex_shrink_0()
            .w(px(if list {
                width
            } else {
                cols as f32 * width + (cols - 1) as f32 * 8. * scale
            }))
            .gap(px(if list { 4. } else { 8. * scale }))
            .items_start();
        for p in items {
            let owner = self
                .owners
                .get(&p.instance)
                .map(String::as_str)
                .unwrap_or(&p.instance);
            let mut item = div()
                .id(SharedString::from(format!("item-{}/{}", p.instance, p.id)))
                .debug_selector({
                    let id = format!("dashboard-item-{}", p.id);
                    move || id
                })
                .w(px(width))
                .h(px(height))
                .flex_shrink_0()
                .rounded_lg()
                .bg(crate::appearance::color(0x222d3d))
                .overflow_hidden()
                .flex()
                .flex_col();
            if !list {
                item = item.child(
                    div()
                        .px(px(8. * scale))
                        .h(px(24. * scale))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .text_size(px(12. * scale))
                        .text_color(crate::appearance::color(0x9cabc0))
                        .child(
                            div()
                                .truncate()
                                .child(format!("{owner} · {}", p.surface.title)),
                        ),
                );
            }
            content = content.child(item.child(presentation::document_in(
                p,
                sender.clone(),
                width,
                if list {
                    height
                } else {
                    (height - 24. * scale).max(1.)
                },
            )));
        }
        let body = if items.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(crate::appearance::color(0x96a2b7))
                .child(crate::i18n::tr("暂无插件提供此视图"))
                .into_any_element()
        } else {
            div()
                .w_full()
                .min_h(px(available_height))
                .flex()
                .flex_col()
                .when(
                    self.options.horizontal_alignment == Alignment::Center,
                    |d| d.items_center(),
                )
                .when(self.options.horizontal_alignment == Alignment::End, |d| {
                    d.items_end()
                })
                .when(self.options.vertical_alignment == Alignment::Center, |d| {
                    d.justify_center()
                })
                .when(self.options.vertical_alignment == Alignment::End, |d| {
                    d.justify_end()
                })
                .child(content)
                .into_any_element()
        };
        div()
            .size_full()
            .rounded(px(14.))
            .bg(crate::appearance::popup_background(None))
            .text_color(crate::appearance::color(0xe7edf7))
            .font_family("Microsoft YaHei UI")
            .overflow_hidden()
            .flex()
            .flex_col()
            .p_4()
            .gap_3()
            .child(
                div()
                    .h(px(36.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::i18n::tr("插件速览")),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(control(
                                "dashboard-settings",
                                "设置",
                                "settings",
                                json!({"op":"dashboard_open_page","page":"settings"}),
                                sender,
                            ))
                            .child(control(
                                "dashboard-manage",
                                "管理",
                                "blocks",
                                json!({"op":"dashboard_open_page","page":"plugins"}),
                                sender,
                            )),
                    ),
            )
            .child(
                div()
                    .id("dashboard-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(
                div()
                    .h(px(16.))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(crate::appearance::color(0x96a2b7))
                    .child(crate::i18n::tr(&format!(
                        "{} · {} 项",
                        if self.options.mode == Mode::List {
                            crate::i18n::text("项目视图")
                        } else {
                            crate::i18n::text("卡片视图")
                        },
                        items.len()
                    ))),
            )
            .into_any_element()
    }
}
fn control(
    id: &'static str,
    title: &str,
    icon: &str,
    body: serde_json::Value,
    sender: &async_channel::Sender<Request>,
) -> AnyElement {
    let sender = sender.clone();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_md()
        .text_xs()
        .cursor_pointer()
        .hover(|s| s.bg(crate::appearance::color(0x2c394e)))
        .child(
            svg()
                .path(SharedString::from(format!("icons/{icon}.svg")))
                .text_color(crate::appearance::color(0x9cabc0))
                .size(px(14.)),
        )
        .child(crate::i18n::tr(title))
        .on_click(move |_, _, _| {
            let (tx, _) = std::sync::mpsc::sync_channel(1);
            let _ = sender.try_send(Request {
                body: body.clone(),
                reply: tx,
                expires: None,
            });
        })
        .into_any_element()
}
pub fn matches(options: &Options, p: &Publication) -> bool {
    p.surface.visible
        && p.surface.target
            == if options.mode == Mode::List {
                Target::TrayList
            } else {
                Target::TrayCard
            }
}
