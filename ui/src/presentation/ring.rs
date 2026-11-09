use super::*;
pub fn graphic(
    size: f32,
    thickness: f32,
    progress: f32,
    fg: Hsla,
    track: Hsla,
    angle: f32,
) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let center = bounds.center();
            let radius = f32::from(bounds.size.width.min(bounds.size.height)) / 2. - thickness / 2.;
            for (fraction, ink, offset) in [(1., track, 0.), (progress, fg, angle)] {
                if fraction <= 0. {
                    continue;
                }
                let steps = (96. * fraction).ceil() as usize;
                let mut builder = PathBuilder::stroke(px(thickness));
                for i in 0..=steps {
                    let a = std::f32::consts::TAU * (i as f32 / steps as f32) * fraction
                        - std::f32::consts::FRAC_PI_2
                        + offset;
                    let pos = point(
                        center.x + px(radius * a.cos()),
                        center.y + px(radius * a.sin()),
                    );
                    if i == 0 {
                        builder.move_to(pos);
                    } else {
                        builder.line_to(pos);
                    }
                }
                if let Ok(path) = builder.build() {
                    window.paint_path(path, ink);
                }
            }
        },
    )
    .w(px(size))
    .h(px(size))
    .into_any_element()
}
pub fn element(n: &Node, scale: f32, animate: bool, fg: Hsla, track: Hsla) -> AnyElement {
    let size = n.style.size.unwrap_or(36.).max(2.) * scale;
    let stroke = n
        .style
        .stroke
        .unwrap_or(3.)
        .clamp(0.1, n.style.size.unwrap_or(36.).max(2.) / 2.)
        * scale;
    let progress = n.value / 100.;
    let layer = div().absolute().size_full();
    if animate && let Some(animation) = &n.animation {
        layer
            .with_animation(
                "ring-spin",
                Animation::new(std::time::Duration::from_millis(animation.duration_ms))
                    .repeat_synced()
                    .with_max_fps(animation.fps as f32),
                move |layer, phase| {
                    layer.child(graphic(
                        size,
                        stroke,
                        progress,
                        fg,
                        track,
                        phase * std::f32::consts::TAU,
                    ))
                },
            )
            .into_any_element()
    } else {
        layer
            .child(graphic(size, stroke, progress, fg, track, 0.))
            .into_any_element()
    }
}
