//! Finite whole-card projection, including text and artwork; no snapshots or timers.
use super::*;
#[derive(IntoElement)]
pub struct Flip {
    pub front: AnyElement,
    pub back: AnyElement,
    pub show_back: bool,
    pub phase: f32,
    pub width: f32,
    pub height: f32,
}
impl RenderOnce for Flip {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let second = self.phase >= 0.5;
        let back = if second {
            self.show_back
        } else {
            !self.show_back
        };
        let eased = 0.5 - 0.5 * (std::f32::consts::PI * self.phase).cos();
        let factor = (std::f32::consts::PI * eased).cos().abs().max(0.001);
        let content = div()
            .w(px(self.width))
            .h(px(self.height))
            .relative()
            .flex_shrink_0()
            .overflow_hidden()
            .child(if back { self.back } else { self.front })
            // Hitboxes intentionally retain layout coordinates during this paint-only effect.
            .children((self.phase < 1.).then(|| div().absolute().inset_0().occlude()));
        crate::presentation::transform::HorizontalCompression {
            child: content.into_any_element(),
            width: self.width,
            height: self.height,
            factor,
        }
    }
}
