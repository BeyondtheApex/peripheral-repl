//! Paint-only horizontal foreshortening. Atlas pixels remain resident on the GPU.
//! Draw bounds remain conservative because the public window API only compresses.
use super::*;

impl Scene {
    pub(crate) fn scale_checkpoint(&self) -> [usize; 9] {
        [self.quads.len(), self.shadows.len(), self.paths.len(), self.underlines.len(),
         self.monochrome_sprites.len(), self.subpixel_sprites.len(),
         self.polychrome_sprites.len(), self.surfaces.len(), self.paint_operations.len()]
    }
    pub(crate) fn scale_x_since(&mut self, mark: [usize; 9], center: f32, scale: f32) {
        let projection = Projection {center, scale};
        macro_rules! project {
            ($field:ident, $index:expr, $variant:ident) => {
                for item in &mut self.$field[mark[$index]..] {
                    let mut primitive = Primitive::$variant(item.clone());
                    projection.primitive(&mut primitive);
                    if let Primitive::$variant(value) = primitive { *item = value; }
                }
            };
        }
        project!(quads, 0, Quad);
        project!(shadows, 1, Shadow);
        project!(paths, 2, Path);
        project!(underlines, 3, Underline);
        project!(monochrome_sprites, 4, MonochromeSprite);
        project!(subpixel_sprites, 5, SubpixelSprite);
        project!(polychrome_sprites, 6, PolychromeSprite);
        project!(surfaces, 7, Surface);
        // Keep captured paint operations consistent with the submitted primitives.
        for op in &mut self.paint_operations[mark[8]..] {
            match op {
                PaintOperation::Primitive(p) => projection.primitive(p),
                PaintOperation::StartLayer(bounds) => projection.bounds(bounds),
                PaintOperation::EndLayer => {}
            }
        }
    }
}
struct Projection { center: f32, scale: f32 }
impl Projection {
    fn point(&self, p: &mut Point<ScaledPixels>) {
        p.x = ScaledPixels(self.center + (p.x.0 - self.center) * self.scale);
    }
    fn bounds(&self, b: &mut Bounds<ScaledPixels>) {
        self.point(&mut b.origin);
        b.size.width *= self.scale;
    }
    fn corners(&self, c: &mut Corners<ScaledPixels>) {
        // GPUI stores circular radii, not elliptical ones; avoid overlapping corners.
        *c = c.map(|r| *r * self.scale);
    }
    fn primitive(&self, p: &mut Primitive) {
        let matrix = TransformationMatrix {
            rotation_scale: [[self.scale, 0.], [0., 1.]],
            translation: [self.center * (1. - self.scale), 0.],
        };
        match p {
            Primitive::Quad(q) => {
                self.bounds(&mut q.bounds); self.bounds(&mut q.content_mask.bounds);
                self.corners(&mut q.corner_radii);
                q.border_widths.left *= self.scale; q.border_widths.right *= self.scale;
            }
            Primitive::Shadow(s) => {
                self.bounds(&mut s.bounds); self.bounds(&mut s.content_mask.bounds);
                self.bounds(&mut s.element_bounds);
                self.corners(&mut s.corner_radii); self.corners(&mut s.element_corner_radii);
                s.blur_radius *= self.scale;
            }
            Primitive::Path(p) => {
                self.bounds(&mut p.bounds); self.bounds(&mut p.content_mask.bounds);
                self.point(&mut p.start); self.point(&mut p.current);
                for v in &mut p.vertices {
                    self.point(&mut v.xy_position); self.bounds(&mut v.content_mask.bounds);
                }
            }
            Primitive::Underline(u) => {
                self.bounds(&mut u.bounds); self.bounds(&mut u.content_mask.bounds);
            }
            Primitive::MonochromeSprite(s) => {
                s.transformation = matrix.compose(s.transformation);
                self.bounds(&mut s.content_mask.bounds);
            }
            Primitive::SubpixelSprite(s) => {
                s.transformation = matrix.compose(s.transformation);
                self.bounds(&mut s.content_mask.bounds);
            }
            Primitive::PolychromeSprite(s) => {
                self.bounds(&mut s.bounds); self.bounds(&mut s.content_mask.bounds);
                self.corners(&mut s.corner_radii);
            }
            Primitive::Surface(s) => {
                self.bounds(&mut s.bounds); self.bounds(&mut s.content_mask.bounds);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Scene, Quad, Bounds, ScaledPixels, ContentMask};
    #[test]
    fn compression_is_scoped_and_replay_keeps_projected_geometry() {
        let quad = Quad { bounds: Bounds::new(crate::point(ScaledPixels(10.),ScaledPixels(20.)),crate::size(ScaledPixels(100.),ScaledPixels(80.))),
            content_mask: ContentMask {bounds:Bounds::new(crate::point(ScaledPixels(0.),ScaledPixels(0.)),crate::size(ScaledPixels(200.),ScaledPixels(200.)))}, ..Default::default() };
        let mut scene=Scene::default();scene.insert_primitive(quad);
        let mark=scene.scale_checkpoint();scene.insert_primitive(quad);
        scene.scale_x_since(mark,60.,0.5);
        assert_eq!(scene.quads[0].bounds,quad.bounds);
        assert_eq!(scene.quads[1].bounds.origin.x,ScaledPixels(35.));
        assert_eq!(scene.quads[1].bounds.size.width,ScaledPixels(50.));
        assert_eq!(scene.quads[1].bounds.size.height,ScaledPixels(80.));
        let mut replay=Scene::default();replay.replay(1..2,&scene);
        assert_eq!(replay.quads[0].bounds,scene.quads[1].bounds);
    }
}
