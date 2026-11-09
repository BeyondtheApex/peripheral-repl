//! Bounded declarative scene rendering; no device, session or update logic.
use crate::control::Request;
use gpui_kit::{prelude::FluentBuilder, *};
use peripheral_repl::ui::Publication;
use peripheral_ui_protocol::{Kind, Node, color};
use serde_json::json;
mod fit;
pub mod images;
pub mod input;
mod ring;
pub mod transform;

fn paint(value: &Option<String>, fallback: u32) -> Hsla {
    rgba(
        value
            .as_ref()
            .and_then(|s| color(s).ok())
            .unwrap_or(fallback),
    )
    .into()
}
pub fn document(publication: &Publication, commands: async_channel::Sender<Request>) -> AnyElement {
    document_in(
        publication,
        commands,
        publication.surface.width,
        publication.surface.height,
    )
}
pub fn document_in(
    publication: &Publication,
    commands: async_channel::Sender<Request>,
    width: f32,
    height: f32,
) -> AnyElement {
    let element = div()
        .id(SharedString::from(format!(
            "{}/{}",
            publication.instance, publication.id
        )))
        .w(px(width))
        .h(px(height))
        .overflow_hidden()
        .font_family("Microsoft YaHei UI")
        .flex()
        .items_center()
        .justify_center()
        .bg(
            if publication.surface.target == peripheral_ui_protocol::Target::Popup {
                crate::appearance::popup_background(publication.surface.background.as_ref())
            } else {
                paint(&publication.surface.background, 0x00000000)
            },
        )
        .rounded(px(publication.surface.radius.unwrap_or(
            if publication.surface.target == peripheral_ui_protocol::Target::Popup {
                12.
            } else {
                0.
            },
        )))
        .child(fit::Fitted {
            publication: publication.clone(),
            commands: commands.clone(),
            width,
            height,
        });
    input::bind(
        element,
        publication,
        None,
        &publication.surface.events,
        &publication.surface.action,
        &commands,
    )
    .into_any_element()
}
fn node(
    n: &Node,
    p: &Publication,
    commands: &async_channel::Sender<Request>,
    scale: f32,
    animate: bool,
) -> AnyElement {
    let fg = if n.style.color.is_none() {
        crate::appearance::token("text")
    } else {
        paint(&n.style.color, 0xeff3faff)
    };
    let bg = paint(&n.style.background, 0x00000000);
    let track = paint(&n.style.track, 0x454b55ff);
    let mut element = div()
        .id(SharedString::from(n.id.clone()))
        .relative()
        .flex_shrink_0()
        .bg(bg)
        .text_color(fg)
        .rounded(px(n.style.radius.unwrap_or(0.) * scale))
        .p(px(n.style.padding.unwrap_or(0.) * scale))
        .gap(px(n.style.gap.unwrap_or(6.) * scale))
        .when_some(n.style.width, |s, v| s.min_w(px(v * scale)))
        .when_some(n.style.height, |s, v| s.min_h(px(v * scale)))
        .when_some(n.style.size, |s, v| {
            s.min_w(px(v * scale)).min_h(px(v * scale))
        });
    match n.kind {
        Kind::Image => {
            let width = n.style.width.or(n.style.size).unwrap_or(48.).max(1.) * scale;
            let height = n.style.height.or(n.style.size).unwrap_or(48.).max(1.) * scale;
            if let Some(spec) = &n.image {
                let scaled_spec = peripheral_ui_protocol::ImageSpec {
                    radius: spec.radius * scale,
                    ..spec.clone()
                };
                element = element.w(px(width)).h(px(height)).child(images::element(
                    &scaled_spec,
                    p.resources.get(&spec.resource),
                    width,
                    height,
                ));
            }
        }
        Kind::Row | Kind::Column | Kind::Button => {
            element = element
                .flex()
                .items_center()
                .when(n.kind == Kind::Column, |s| s.flex_col())
                .children(
                    n.children
                        .iter()
                        .map(|child| node(child, p, commands, scale, animate)),
                );
            if n.kind == Kind::Button {
                element = element
                    .justify_center()
                    .text_size(px(n.style.font_size.unwrap_or(14.) * scale))
                    .line_height(px(n.style.font_size.unwrap_or(14.) * 1.4 * scale))
                    .whitespace_nowrap()
                    .when(!n.text.is_empty(), |d| d.child(n.text.clone()));
            }
        }
        Kind::Text => {
            element = element
                .text_size(px(n.style.font_size.unwrap_or(14.) * scale))
                .line_height(px(n.style.font_size.unwrap_or(14.) * 1.4 * scale))
                .whitespace_nowrap()
                .child(n.text.clone());
        }
        Kind::Icon => {
            element = element
                .size(px(n.style.size.unwrap_or(20.) * scale))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(SharedString::from(icon_path(&n.icon)))
                        .size_full()
                        .text_color(fg),
                );
        }
        Kind::Spacer => {
            element = element.w(px(n.style.width.unwrap_or(8.) * scale)).h(px(n
                .style
                .height
                .unwrap_or(1.)
                * scale));
        }
        Kind::Progress => {
            let w = n.style.width.unwrap_or(64.) * scale;
            let h = n.style.height.unwrap_or(6.) * scale;
            element = element
                .w(px(w))
                .h(px(h))
                .bg(track)
                .rounded(px(h / 2.))
                .child(
                    div()
                        .w(px(w * n.value / 100.))
                        .h_full()
                        .rounded(px(h / 2.))
                        .bg(fg),
                );
        }
        Kind::Ring => {
            let size = n.style.size.unwrap_or(36.).max(2.) * scale;
            element = element
                .size(px(size))
                .flex()
                .items_center()
                .justify_center()
                .child(ring::element(n, scale, animate, fg, track))
                .children(
                    n.children
                        .iter()
                        .map(|child| node(child, p, commands, scale, animate)),
                );
        }
    }
    input::bind(element, p, Some(&n.id), &n.events, &n.action, commands).into_any_element()
}

fn icon_path(name: &str) -> String {
    let path = format!("icons/{name}.svg");
    if gpui_kit::assets::AllAssets.load(&path).is_ok() {
        path
    } else {
        "icons/circle-help.svg".into()
    }
}
