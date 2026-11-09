//! Measure actual GPUI text/layout, then scale all geometry together. No bitmap copy.
//! The final tree is laid out at its displayed size, so hitboxes scale with the artwork.
use super::*;
use peripheral_ui_protocol::ScaleMode;

pub fn factor(mode: ScaleMode, content: Size<Pixels>, available: Size<Pixels>) -> f32 {
    if mode == ScaleMode::None {
        return 1.;
    }
    let ratio = (f32::from(available.width) / f32::from(content.width).max(1.))
        .min(f32::from(available.height) / f32::from(content.height).max(1.));
    if mode == ScaleMode::Shrink {
        ratio.min(1.)
    } else {
        ratio
    }
}
pub struct Fitted {
    pub publication: Publication,
    pub commands: async_channel::Sender<Request>,
    pub width: f32,
    pub height: f32,
}
impl IntoElement for Fitted {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Fitted {
    type RequestLayoutState = ();
    type PrepaintState = AnyElement;
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
        let s = Style {
            size: size(px(self.width).into(), px(self.height).into()),
            flex_shrink: 0.,
            ..Default::default()
        };
        (window.request_layout(s, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let space = size(AvailableSpace::MaxContent, AvailableSpace::MaxContent);
        let mut measure = node(
            &self.publication.surface.root,
            &self.publication,
            &self.commands,
            1.,
            false,
        );
        let natural = measure.layout_as_root(space, window, cx);
        let scale = factor(self.publication.surface.scale, natural, bounds.size).max(0.0001);
        let mut content = node(
            &self.publication.surface.root,
            &self.publication,
            &self.commands,
            scale,
            true,
        );
        let fitted = content.layout_as_root(space, window, cx);
        content.prepaint_at(
            point(
                bounds.origin.x + (bounds.size.width - fitted.width) / 2.,
                bounds.origin.y + (bounds.size.height - fitted.height) / 2.,
            ),
            window,
            cx,
        );
        content
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        content: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        content.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{ScaleMode, factor, px, size};
    #[test]
    fn fit_preserves_aspect_in_both_axes() {
        assert_eq!(
            factor(
                ScaleMode::Shrink,
                size(px(200.), px(100.)),
                size(px(80.), px(30.))
            ),
            0.3
        );
        assert_eq!(
            factor(
                ScaleMode::Shrink,
                size(px(20.), px(10.)),
                size(px(80.), px(30.))
            ),
            1.
        );
        assert_eq!(
            factor(
                ScaleMode::Contain,
                size(px(20.), px(10.)),
                size(px(80.), px(30.))
            ),
            3.
        );
    }
}
