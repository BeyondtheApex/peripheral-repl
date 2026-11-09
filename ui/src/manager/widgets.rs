use super::*;
pub(super) const BG: u32 = 0x11151b;
pub(super) const PANEL: u32 = 0x1a202a;
pub(super) const TEXT: u32 = 0xe7edf7;
pub(super) const MUTED: u32 = 0x96a2b7;
pub fn icon(name: &str) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(17.))
        .text_color(crate::appearance::color(0x9aafff))
}
pub(crate) fn label(text: impl Into<SharedString>) -> Div {
    let text: SharedString = text.into();
    div()
        .text_sm()
        .text_color(crate::appearance::color(MUTED))
        .child(crate::i18n::tr(&text))
}
pub(super) fn send(sender: &async_channel::Sender<Request>, body: Value) {
    let (tx, _) = std::sync::mpsc::sync_channel(1);
    let _ = sender.try_send(Request {
        body,
        reply: tx,
        expires: None,
    });
}
pub(crate) fn button(
    id: impl Into<SharedString>,
    title: &str,
    ico: &str,
    body: Value,
    sender: &async_channel::Sender<Request>,
) -> AnyElement {
    button_with_icon(id, title, icon(ico).into_any_element(), body, sender)
}
pub(crate) fn button_with_icon(
    id: impl Into<SharedString>,
    title: &str,
    graphic: AnyElement,
    body: Value,
    sender: &async_channel::Sender<Request>,
) -> AnyElement {
    let sender = sender.clone();
    let icon_only = title.is_empty();
    let id = id.into();
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(if icon_only { 0. } else { 8. }))
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(crate::appearance::color(0x293349))
        .text_sm()
        .text_color(crate::appearance::color(TEXT))
        .cursor_pointer()
        .hover(|s| s.bg(crate::appearance::interactive_hover()))
        .active(|s| s.bg(crate::appearance::token("elevated")).opacity(0.8))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            send(&sender, body.clone());
        })
        .child(
            div()
                .size(px(17.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(graphic),
        )
        .children((!icon_only).then(|| div().child(crate::i18n::tr(title))))
        .into_any_element()
}

pub(crate) fn toggle(
    id: impl Into<SharedString>,
    enabled: bool,
    body: Value,
    sender: &async_channel::Sender<Request>,
) -> AnyElement {
    let sender = sender.clone();
    let id = id.into();
    div()
        .id(id.clone())
        .group(id.clone())
        .w(px(44.))
        .h(px(25.))
        .relative()
        .rounded_full()
        .cursor_pointer()
        .hover(|s| s.opacity(0.82))
        .active(|s| s.opacity(0.65))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            send(&sender, body.clone());
        })
        .with_spring(
            SharedString::from(format!("switch-{id}")),
            motion::control_spring(if enabled { 1. } else { 0. }),
            move |d, t| {
                d.bg(motion::mix_color(
                    crate::appearance::token("border"),
                    crate::appearance::token("accent"),
                    t,
                ))
                .child(
                    div()
                        .absolute()
                        .top(px(3.))
                        .left(px(3. + 19. * t.clamp(0., 1.)))
                        .size(px(19.))
                        .rounded_full()
                        .bg(crate::appearance::color(0xf4f7fb))
                        .group_hover(id.clone(), |s| s.bg(crate::appearance::token("text"))),
                )
            },
        )
        .into_any_element()
}
