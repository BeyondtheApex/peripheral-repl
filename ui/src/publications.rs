//! Reconcile declarative publications into stable native surfaces.
use crate::{app::Application, model::SurfaceKind, native};
use anyhow::{Context, Result};
use gpui_kit::*;
use peripheral_repl::ui::Publication;
use peripheral_ui_protocol::Target;
use serde_json::Value;
use std::collections::BTreeMap;
impl Application {
    pub fn accept_publications(&mut self, snapshot: &Value, cx: &mut App) -> Result<()> {
        let revision = snapshot["revision"].as_u64();
        if revision
            .zip(self.revision)
            .is_some_and(|(new, old)| new < old)
        {
            return Ok(());
        }
        self.revision = revision;
        let publications: Vec<Publication> = serde_json::from_value(snapshot["surfaces"].clone())?;
        anyhow::ensure!(publications.len() <= 64, "too many published surfaces");
        for p in &publications {
            p.surface.validate()?;
        }
        self.manager(cx,|m,_|{
            if let Some(plugins)=m.backend["plugins"].as_array_mut(){for plugin in plugins{
                plugin["views"]=serde_json::json!(publications.iter().filter(|p|plugin["instance"]==p.instance).map(|p|serde_json::json!({"id":p.id,"title":p.surface.title,"target":p.surface.target,"publication":p})).collect::<Vec<_>>());
            }}
        });
        self.publications = publications;
        self.reconcile(cx)
    }
    pub fn reconcile(&mut self, cx: &mut App) -> Result<()> {
        self.refresh_dashboard(cx)?;
        let mut desired: BTreeMap<String, (SurfaceKind, Vec<Publication>)> = BTreeMap::new();
        for p in &self.publications {
            if !p.surface.visible {
                continue;
            }
            let kind = match p.surface.target {
                Target::Taskbar => SurfaceKind::Taskbar,
                Target::Tray => SurfaceKind::Tray,
                Target::Overlay => SurfaceKind::Overlay,
                Target::Window => SurfaceKind::Window,
                Target::Popup => SurfaceKind::Popup,
                Target::TaskbarPreview => SurfaceKind::TaskbarPreview,
                Target::Texture => SurfaceKind::Texture,
                Target::Xaml => SurfaceKind::Xaml,
                Target::Panel | Target::TrayList | Target::TrayCard => continue,
            };
            if kind == SurfaceKind::Taskbar && !self.taskbar_enabled {
                continue;
            }
            let id = if kind == SurfaceKind::Taskbar {
                "taskbar".into()
            } else {
                use sha2::{Digest, Sha256};
                format!(
                    "view-{:x}",
                    Sha256::digest(format!("{}/{}", p.instance, p.id))
                )[..37]
                    .to_owned()
            };
            desired
                .entry(id)
                .or_insert_with(|| (kind, Vec::new()))
                .1
                .push(p.clone());
        }
        let obsolete = self
            .surfaces
            .keys()
            .filter(|id| {
                id.as_str() != "main"
                    && id.as_str() != crate::dashboard::ID
                    && id.as_str() != crate::app_menu::ID
                    && !desired.contains_key(*id)
            })
            .cloned()
            .collect::<Vec<_>>();
        for id in obsolete {
            self.close(&id, cx)?;
        }
        // Parents are created before submenus, independent of hashed native IDs.
        let mut desired = desired.into_iter().collect::<Vec<_>>();
        desired.sort_by_key(|(_, (_, items))| {
            let mut depth = 0;
            let mut parent = items[0].surface.parent.as_deref();
            while let Some(id) = parent {
                depth += 1;
                if depth > 8 {
                    break;
                }
                parent = self
                    .publications
                    .iter()
                    .find(|p| p.instance == items[0].instance && p.id == id)
                    .and_then(|p| p.surface.parent.as_deref());
            }
            depth
        });
        for (id, (kind, items)) in desired {
            let width = items.iter().map(|p| p.surface.width).sum::<f32>();
            let height = items.iter().map(|p| p.surface.height).fold(16., f32::max);
            anyhow::ensure!(
                kind != SurfaceKind::Taskbar || width <= 640.,
                "combined taskbar views exceed 640 logical pixels"
            );
            if self
                .surfaces
                .get(&id)
                .is_some_and(|s| s.view.read(cx).kind != kind)
            {
                self.close(&id, cx)?;
            }
            if !self.surfaces.contains_key(&id) {
                self.open(&id, kind, (width, height), cx)?;
            }
            let s = self.surfaces.get(&id).context("surface")?;
            let unchanged = {
                let old = &s.view.read(cx).publications;
                old.len() == items.len()
                    && old.iter().zip(&items).all(|(a, b)| {
                        a.revision == b.revision && a.generation == b.generation && a.id == b.id
                    })
            };
            if !unchanged {
                if kind == SurfaceKind::Popup {
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
                    let family =
                        if matches!(root.surface.target, Target::TrayList | Target::TrayCard) {
                            crate::dashboard::ID.into()
                        } else {
                            format!("{}/{}", root.instance, root.id)
                        };
                    native::popup::register(s.hwnd, family)?;
                    let parent = items[0]
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
                        .map(|s| s.hwnd);
                    if s.view.read(cx).publications.is_empty() {
                        native::popup::show_at(s.hwnd, parent)?;
                    }
                }
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
            ) && self.surfaces[&id].bridge.is_none()
            {
                self.start_bridge(&id, cx)?;
            }
        }
        Ok(())
    }
}
