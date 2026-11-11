//! Apply one presentation target; native failures do not block sibling views.
use super::*;
use anyhow::Context;
impl Application {
    pub(super) fn reconcile_surface(
        &mut self,
        id: &str,
        kind: SurfaceKind,
        items: Vec<Publication>,
        cx: &mut App,
    ) -> Result<()> {
        let width = items.iter().map(|p| p.surface.width).sum::<f32>();
        let height = items.iter().map(|p| p.surface.height).fold(16., f32::max);
        anyhow::ensure!(
            kind != SurfaceKind::Taskbar || width <= 640.,
            "combined taskbar views exceed 640 logical pixels"
        );
        if self
            .surfaces
            .get(id)
            .is_some_and(|s| s.view.read(cx).kind != kind)
        {
            self.close(id, cx)?;
        }
        if !self.surfaces.contains_key(id) {
            self.open(id, kind, (width, height), cx)?;
        }
        let s = self.surfaces.get(id).context("surface")?;
        let unchanged = {
            let old = &s.view.read(cx).publications;
            old.len() == items.len()
                && old.iter().zip(&items).all(|(a, b)| {
                    a.revision == b.revision
                        && a.generation == b.generation
                        && a.instance == b.instance
                        && a.id == b.id
                })
        };
        if !unchanged {
            let popup_position = items[0].surface.x.zip(items[0].surface.y);
            let popup_parent = if kind == SurfaceKind::Popup {
                let mut root = &items[0];
                for _ in 0..8 {
                    let Some(parent) = root.surface.parent.as_deref() else {
                        break;
                    };
                    let Some(p) = self
                        .publications
                        .iter()
                        .find(|p| p.instance == root.instance && p.id == parent)
                    else {
                        break;
                    };
                    root = p;
                }
                let family = if matches!(root.surface.target, Target::TrayList | Target::TrayCard) {
                    crate::dashboard::ID.into()
                } else {
                    format!("{}/{}", root.instance, root.id)
                };
                native::popup::register(s.hwnd, family)?;
                items[0]
                    .surface
                    .parent
                    .as_ref()
                    .and_then(|id| {
                        self.surfaces.values().find(|s| {
                            s.view.read(cx).kind == SurfaceKind::Popup
                                && s.view
                                    .read(cx)
                                    .publications
                                    .iter()
                                    .any(|p| p.instance == items[0].instance && p.id == *id)
                        })
                    })
                    .map(|s| s.hwnd)
            } else {
                None
            };
            native::set_title(
                s.hwnd,
                &items
                    .iter()
                    .map(|p| p.surface.title.as_str())
                    .collect::<Vec<_>>()
                    .join(" · "),
            )?;
            let dimensions_changed = {
                let old = &s.view.read(cx).publications;
                old.iter().map(|p| p.surface.width).sum::<f32>() != width
                    || old.iter().map(|p| p.surface.height).fold(16., f32::max) != height
            };
            if dimensions_changed {
                if matches!(kind, SurfaceKind::Window | SurfaceKind::TaskbarPreview) {
                    s.window
                        .update(cx, |_, w, _| w.resize(size(px(width), px(height))))?;
                } else {
                    let scale = s.window.update(cx, |_, w, _| w.scale_factor())?;
                    native::resize_floating(
                        s.hwnd,
                        (width * scale).round() as i32,
                        (height * scale).round() as i32,
                    )?;
                }
            }
            if let (Some(x), Some(y)) = (items[0].surface.x, items[0].surface.y)
                && matches!(
                    kind,
                    SurfaceKind::Overlay
                        | SurfaceKind::Window
                        | SurfaceKind::Popup
                        | SurfaceKind::TaskbarPreview
                )
            {
                let scale = s.window.update(cx, |_, w, _| w.scale_factor())?;
                native::place(
                    s.hwnd,
                    x.round() as i32,
                    y.round() as i32,
                    (width * scale).round() as i32,
                    (height * scale).round() as i32,
                )?;
            }
            if kind == SurfaceKind::Overlay {
                native::set_passive(s.hwnd, !items[0].surface.interactive)?;
            }
            s.view.update(cx, |v, cx| {
                v.passive = kind == SurfaceKind::Overlay && !items[0].surface.interactive;
                v.publications = items;
                cx.notify();
            });
            if kind == SurfaceKind::Popup && !native::is_visible(s.hwnd) {
                if let Some((x, y)) = popup_position {
                    native::popup::show_positioned(s.hwnd, x.round() as i32, y.round() as i32)?;
                } else {
                    native::popup::show_at(s.hwnd, popup_parent)?;
                }
            }
            if kind == SurfaceKind::Tray {
                let image = s.window.update(cx, |_, w, cx| {
                    w.refresh();
                    let _ = w.draw(cx);
                    w.render_to_image()
                })??;
                native::set_rendered_tray(s.hwnd, 1, &image)?;
            }
        }
        if matches!(
            kind,
            SurfaceKind::Taskbar | SurfaceKind::Texture | SurfaceKind::Xaml
        ) && self.surfaces[id].bridge.is_none()
        {
            self.start_bridge(id, cx)?;
        }
        Ok(())
    }
}
