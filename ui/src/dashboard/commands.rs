use super::*;
use crate::{app::Application, model::SurfaceKind, native};
use anyhow::{Context, Result};
use serde_json::Value;
impl Application {
    pub fn dashboard_command(&mut self, v: &Value, cx: &mut App) -> Option<Result<Value>> {
        let op = v["op"].as_str()?;
        if ![
            "dashboard_toggle",
            "dashboard_settings",
            "dashboard_open_page",
        ]
        .contains(&op)
        {
            return None;
        }
        Some((|| {
            match op {
                "dashboard_toggle" => {
                    if self.surfaces.contains_key(ID) {
                        self.dismiss_dashboard(cx)?;
                    } else {
                        let options = self.dashboard_options.clone();
                        self.open(
                            ID,
                            SurfaceKind::Popup,
                            (options.width as f32, options.height as f32),
                            cx,
                        )?;
                        let surface = &self.surfaces[ID];
                        let scale = surface.window.update(cx, |_, w, _| w.scale_factor())?;
                        native::resize_floating(
                            surface.hwnd,
                            (options.width as f32 * scale).round() as i32,
                            (options.height as f32 * scale).round() as i32,
                        )?;
                        self.refresh_dashboard(cx)?;
                        let s = &self.surfaces[ID];
                        native::popup::register(s.hwnd, ID.into())?;
                        native::popup::show_at(s.hwnd, None)?;
                    }
                }
                "dashboard_settings" => {
                    let field = v["field"].as_str().context("setting field")?;
                    anyhow::ensure!(
                        [
                            "mode",
                            "horizontal_alignment",
                            "vertical_alignment",
                            "width",
                            "height",
                            "rows",
                            "columns",
                            "row_height",
                            "card_width",
                            "card_height",
                            "device_card_width",
                            "device_card_height"
                        ]
                        .contains(&field),
                        "unknown dashboard setting"
                    );
                    let mut options = serde_json::to_value(&self.dashboard_options)?;
                    options[field] = v["value"].clone();
                    let options: Options = serde_json::from_value(options)?;
                    options.validate()?;
                    save_preference("dashboard", serde_json::to_value(&options)?)?;
                    self.dashboard_options = options.clone();
                    self.manager(cx, |m, _| m.dashboard_options = options);
                    self.refresh_dashboard(cx)?;
                    self.status("速览设置已保存".into(), cx);
                }
                "dashboard_open_page" => {
                    let page = v["page"].as_str().context("page")?;
                    anyhow::ensure!(["plugins", "settings"].contains(&page), "unknown page");
                    self.dismiss_dashboard(cx)?;
                    self.manager(cx, |m, _| {
                        m.transition();
                        m.page = page.into();
                        m.plugin_config = None;
                    });
                    native::show(self.surfaces["main"].hwnd, true);
                }
                _ => {}
            }
            Ok(json!({"options":self.dashboard_options,"open":self.surfaces.contains_key(ID)}))
        })())
    }
    pub fn dismiss_dashboard(&mut self, cx: &mut App) -> Result<()> {
        if self.surfaces.contains_key(ID) {
            self.close(ID, cx)?;
        }
        for p in self
            .publications
            .iter()
            .filter(|p| p.surface.target == Target::Popup && p.surface.visible)
        {
            if p.surface.parent.as_ref().is_some_and(|id| {
                self.publications.iter().any(|owner| {
                    owner.instance == p.instance
                        && owner.id == *id
                        && matches!(owner.surface.target, Target::TrayList | Target::TrayCard)
                })
            }) {
                self.backend.command(json!({"op":"ui_dismiss","instance":p.instance,"surface":p.id,"generation":p.generation,"revision":p.revision}))?;
            }
        }
        Ok(())
    }
    pub fn refresh_dashboard(&mut self, cx: &mut App) -> Result<()> {
        let Some(surface) = self.surfaces.get(ID) else {
            return Ok(());
        };
        let owners = self
            .surfaces
            .get("main")
            .and_then(|s| s.view.read(cx).manager.as_ref())
            .map(|m| {
                m.backend["plugins"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| {
                        Some((
                            p["instance"].as_str()?.to_owned(),
                            p["name"].as_str()?.to_owned(),
                        ))
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let items = self
            .publications
            .iter()
            .filter(|p| owners.contains_key(&p.instance) && matches(&self.dashboard_options, p))
            .cloned()
            .collect::<Vec<_>>();
        let options = self.dashboard_options.clone();
        let changed = {
            let view = surface.view.read(cx);
            let dashboard = view.dashboard.as_ref().context("dashboard missing")?;
            options != dashboard.options
                || owners != dashboard.owners
                || view.publications.len() != items.len()
                || view.publications.iter().zip(&items).any(|(a, b)| {
                    a.instance != b.instance
                        || a.id != b.id
                        || a.generation != b.generation
                        || a.revision != b.revision
                })
        };
        if changed {
            let dimensions_changed = surface.view.read(cx).dashboard.as_ref().is_none_or(|d| {
                d.options.width != options.width || d.options.height != options.height
            });
            if dimensions_changed {
                let scale = surface.window.update(cx, |_, w, _| w.scale_factor())?;
                native::resize_floating(
                    surface.hwnd,
                    (options.width as f32 * scale) as i32,
                    (options.height as f32 * scale) as i32,
                )?;
                native::popup::show_at(surface.hwnd, None)?;
            }
            surface.view.update(cx, |view, cx| {
                view.publications = items;
                view.dashboard = Some(Dashboard { options, owners });
                cx.notify();
            });
        }
        Ok(())
    }
}
