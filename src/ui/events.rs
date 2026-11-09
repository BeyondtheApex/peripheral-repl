//! Resolve input against the current publication, never trust a GUI-supplied action name.
use super::*;
use anyhow::{Context as _, Result, ensure};
use peripheral_ui_protocol::Node;

fn node<'a>(root: &'a Node, id: &str) -> Option<&'a Node> {
    if root.id == id {
        Some(root)
    } else {
        root.children.iter().find_map(|n| node(n, id))
    }
}
fn publication(ctx: &Context, request: &Value) -> Result<Publication> {
    ensure!(
        request["generation"] == ctx.generation,
        "stale UI action: session was reset or reloaded"
    );
    let id = request["surface"].as_str().context("surface ID")?;
    let p = ctx
        .shared
        .ui
        .lock()
        .unwrap()
        .surfaces
        .get(&format!("{}/{}", ctx.name, id))
        .cloned()
        .context("surface no longer published")?;
    ensure!(
        request["revision"] == p.revision,
        "stale UI action: view changed"
    );
    ensure!(p.surface.visible, "surface is hidden");
    Ok(p)
}
pub fn event_request(ctx: &Context, request: &Value) -> Result<Value> {
    let p = publication(ctx, request)?;
    ensure!(
        p.surface.interactive || p.surface.target != peripheral_ui_protocol::Target::Overlay,
        "passive surface cannot receive input"
    );
    let button = request["button"].as_str().context("mouse button")?;
    ensure!(
        ["left", "right"].contains(&button),
        "unsupported mouse button"
    );
    let id = request["node"].as_str();
    // A tray is a single OS hit target, not a miniature hit-testable window.
    ensure!(
        id.is_none() || p.surface.target != peripheral_ui_protocol::Target::Tray,
        "tray has no child hit targets"
    );
    let (events, legacy) = if let Some(id) = id {
        let n = node(&p.surface.root, id).context("node no longer exists")?;
        (&n.events, &n.action)
    } else {
        (&p.surface.events, &p.surface.action)
    };
    let modern = events.action(button);
    let action = modern
        .or_else(|| {
            if button == "left" {
                legacy.as_deref()
            } else {
                None
            }
        })
        .context("no action bound to this button")?;
    let arguments = if modern.is_some() {
        let mut args = json!({"surface":p.id,"button":button});
        if let Some(id) = id {
            args["node"] = json!(id);
        }
        args
    } else {
        json!({})
    };
    Ok(json!({"name":action,"arguments":arguments}))
}
fn descendants(registry: &Registry, instance: &str, id: &str) -> Vec<String> {
    let mut ids = vec![id.to_owned()];
    for _ in 0..8 {
        let children = registry
            .surfaces
            .values()
            .filter(|p| {
                p.instance == instance
                    && !ids.contains(&p.id)
                    && p.surface.parent.as_ref().is_some_and(|v| ids.contains(v))
            })
            .map(|p| p.id.clone())
            .collect::<Vec<_>>();
        if children.is_empty() {
            break;
        }
        ids.extend(children);
    }
    ids
}
pub(super) fn remove_tree(registry: &mut Registry, instance: &str, id: &str) -> bool {
    let before = registry.surfaces.len();
    for id in descendants(registry, instance, id) {
        registry.surfaces.remove(&format!("{instance}/{id}"));
    }
    registry.surfaces.len() != before
}
pub(super) fn hide_tree(ctx: &Context, id: &str) -> Result<()> {
    let mut registry = ctx.shared.ui.lock().unwrap();
    ensure!(
        registry
            .surfaces
            .contains_key(&format!("{}/{}", ctx.name, id)),
        "surface not published"
    );
    let ids = descendants(&registry, &ctx.name, id);
    for id in ids {
        registry.revision += 1;
        let revision = registry.revision;
        let p = registry
            .surfaces
            .get_mut(&format!("{}/{}", ctx.name, id))
            .unwrap();
        p.surface.visible = false;
        p.revision = revision;
    }
    drop(registry);
    ctx.emit("ui.hide", json!({"id":id}));
    Ok(())
}
pub fn dismiss(ctx: &Context, request: &Value) -> Result<Value> {
    let p = publication(ctx, request)?;
    ensure!(
        p.surface.target == peripheral_ui_protocol::Target::Popup,
        "only popups can be dismissed"
    );
    hide_tree(ctx, &p.id)?;
    Ok(json!({"hidden":p.id}))
}
