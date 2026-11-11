//! Reconcile declarative publications into stable native surfaces.
use crate::{app::Application, model::SurfaceKind, native};
use anyhow::Result;
mod surface;
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
        let mut errors = Vec::new();
        for (id, (kind, mut items)) in desired {
            if kind == SurfaceKind::Taskbar {
                peripheral_repl::ui::order::taskbar(&mut items);
            }
            if let Err(error) = self.reconcile_surface(&id, kind, items, cx) {
                errors.push(format!("{id}: {error:#}"));
            }
        }
        anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
        Ok(())
    }
}
