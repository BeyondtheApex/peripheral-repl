//! Paint-only geometry effect for finite transitions; the caller owns input blocking.
use gpui_kit::*;
pub struct HorizontalCompression {
    pub child: AnyElement,
    pub width: f32,
    pub height: f32,
    pub factor: f32,
}
impl IntoElement for HorizontalCompression {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for HorizontalCompression {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            size: size(px(self.width).into(), px(self.height).into()),
            flex_shrink: 0.,
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.layout_as_root(
            size(
                AvailableSpace::Definite(bounds.size.width),
                AvailableSpace::Definite(bounds.size.height),
            ),
            window,
            cx,
        );
        self.child.prepaint_at(bounds.origin, window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.paint_with_horizontal_scale(bounds.center().x, self.factor, |window| {
            self.child.paint(window, cx)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::HorizontalCompression;
    use gpui::{
        AppContext, Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window,
        div, px, rgb,
    };
    use gpui_core as gpui;

    #[gpui::test]
    fn projection_moves_all_children_but_leaves_neighbours_and_layout_unchanged(
        cx: &mut TestAppContext,
    ) {
        struct Fixture;
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .child(HorizontalCompression {
                        width: 100.,
                        height: 80.,
                        factor: 0.5,
                        child: div()
                            .w(px(100.))
                            .h(px(80.))
                            .bg(rgb(0xff0000))
                            .child(div().w(px(20.)).h(px(30.)).bg(rgb(0x00ff00)))
                            .into_any_element(),
                    })
                    .child(div().w(px(100.)).h(px(20.)).bg(rgb(0x0000ff)))
            }
        }
        let window = cx.add_window(|_, _| Fixture);
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let mut q = window.painted_quads();
            q.sort_by(|a, b| {
                a.bounds
                    .origin
                    .y
                    .0
                    .total_cmp(&b.bounds.origin.y.0)
                    .then(b.bounds.size.width.0.total_cmp(&a.bounds.size.width.0))
            });
            assert_eq!(q.len(), 3);
            let scale = window.scale_factor();
            assert_eq!(q[0].bounds.origin.x.0, 25. * scale);
            assert_eq!(q[0].bounds.size.width.0, 50. * scale);
            assert_eq!(q[1].bounds.origin.x.0, 25. * scale);
            assert_eq!(q[1].bounds.size.width.0, 10. * scale);
            assert_eq!(q[2].bounds.origin.x.0, 0.);
            assert_eq!(q[2].bounds.origin.y.0, 80. * scale);
            assert_eq!(q[2].bounds.size.width.0, 100. * scale);
        })
        .unwrap();
    }
}
