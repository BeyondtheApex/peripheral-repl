//! Shared finite motion policy; stable IDs avoid replay on telemetry updates.
use gpui_kit::*;
use std::time::Duration;

pub fn enter(id: impl Into<SharedString>, child: AnyElement) -> AnyElement {
    div()
        .relative()
        .w_full()
        .min_w_0()
        .child(child)
        .with_animation(
            id.into(),
            Animation::new(Duration::from_millis(180))
                .with_easing(|t| 1. - (1. - t).powi(3))
                .with_max_fps(60.),
            |d, t| d.opacity(t).top(px(8. * (1. - t))),
        )
        .into_any_element()
}
/// Popup backgrounds cover the client area throughout entry; translating their
/// full-height root would expose a strip of native backdrop along an edge.
pub fn popup_enter(id: impl Into<SharedString>, child: AnyElement) -> AnyElement {
    div()
        .size_full()
        .overflow_hidden()
        .child(child)
        .with_animation(
            id.into(),
            Animation::new(Duration::from_millis(140)).with_max_fps(60.),
            |d, t| d.opacity(t),
        )
        .into_any_element()
}
pub fn reveal(id: impl Into<SharedString>, open: bool, child: AnyElement) -> AnyElement {
    gpui_kit::component::collapsible::Collapsible::new()
        .motion_id(id.into())
        .open(open)
        .w_full()
        .min_w_0()
        .content(child)
        .into_any_element()
}
pub fn control_spring(target: f32) -> SpringAnimation<f32> {
    SpringAnimation::new(SpringConfig::new(900., 60., 1.))
        .to(target)
        .with_epsilon(0.002)
}
pub fn mix_color(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0., 1.);
    let a: Rgba = a.into();
    let b: Rgba = b.into();
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}
