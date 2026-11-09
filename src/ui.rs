//! Lua presentation API. Owns data, never HWNDs, textures or renderer objects.
use crate::bindings::{Context, to_ui_json};
use mlua::{Lua, LuaSerdeExt, Value as LuaValue};
use peripheral_ui_protocol::{Surface, identifier};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
mod events;
pub use events::{dismiss, event_request};

#[derive(Clone, Serialize, Deserialize)]
pub struct Publication {
    pub instance: String,
    pub id: String,
    pub revision: u64,
    pub generation: String,
    pub surface: Surface,
    #[serde(default)]
    pub resources: BTreeMap<String, peripheral_ui_protocol::ResourceRef>,
}
#[derive(Default)]
pub struct Registry {
    pub revision: u64,
    pub surfaces: BTreeMap<String, Publication>,
}
impl Registry {
    pub fn snapshot(&self) -> Value {
        json!({"revision":self.revision,"surfaces":self.surfaces.values().collect::<Vec<_>>()})
    }
}
fn error(message: impl ToString) -> mlua::Error {
    crate::error::Fault::new("validation", "ui", message).lua()
}
fn publish(ctx: &Context, id: &str, surface: Surface) -> mlua::Result<u64> {
    if !identifier(id) {
        return Err(error("invalid surface ID"));
    }
    surface.validate().map_err(error)?;
    let mut resources = BTreeMap::new();
    collect_resources(&surface.root, ctx, &mut resources)?;
    let key = format!("{}/{}", ctx.name, id);
    let mut registry = ctx.shared.ui.lock().unwrap();
    let mut parent = surface.parent.as_deref();
    let mut depth = 0;
    while let Some(id_parent) = parent {
        if id_parent == id || depth >= 8 {
            return Err(error("cyclic or too deep popup ownership"));
        }
        let owner = registry
            .surfaces
            .get(&format!("{}/{}", ctx.name, id_parent))
            .ok_or_else(|| error("popup parent must be published by this session"))?;
        if !owner.surface.visible {
            return Err(error("popup parent is hidden"));
        }
        parent = owner.surface.parent.as_deref();
        depth += 1;
    }
    if let Some(old) = registry.surfaces.get(&key)
        && old.surface == surface
    {
        return Ok(old.revision);
    }
    if !registry.surfaces.contains_key(&key)
        && registry
            .surfaces
            .values()
            .filter(|s| s.instance == ctx.name)
            .count()
            >= 32
    {
        return Err(error("32 surfaces per session limit"));
    }
    if !registry.surfaces.contains_key(&key) && registry.surfaces.len() >= 64 {
        return Err(error("64 global surfaces limit"));
    }
    if surface.target == peripheral_ui_protocol::Target::Taskbar {
        let width = registry
            .surfaces
            .iter()
            .filter(|(k, p)| {
                **k != key && p.surface.target == peripheral_ui_protocol::Target::Taskbar
            })
            .map(|(_, p)| p.surface.width)
            .sum::<f32>();
        if width + surface.width > 640. {
            return Err(error("combined taskbar views exceed 640 logical pixels"));
        }
    }
    let total = registry
        .surfaces
        .iter()
        .filter(|(k, _)| **k != key)
        .map(|(_, s)| serde_json::to_vec(s).map_or(65536, |b| b.len()))
        .sum::<usize>();
    if total
        + serde_json::to_vec(&surface).map_err(error)?.len()
        + serde_json::to_vec(&resources).map_err(error)?.len()
        + 512
        > 60000
    {
        return Err(error("global UI description budget exceeded"));
    }
    registry.revision += 1;
    let revision = registry.revision;
    let publication = Publication {
        instance: ctx.name.clone(),
        id: id.into(),
        revision,
        generation: ctx.generation.clone(),
        surface,
        resources,
    };
    registry.surfaces.insert(key, publication.clone());
    drop(registry);
    ctx.emit("ui.publish", json!(publication));
    Ok(revision)
}
fn collect_resources(
    node: &peripheral_ui_protocol::Node,
    ctx: &Context,
    out: &mut BTreeMap<String, peripheral_ui_protocol::ResourceRef>,
) -> mlua::Result<()> {
    if let Some(image) = &node.image {
        let resource = ctx
            .assets
            .borrow()
            .get(&image.resource)
            .cloned()
            .ok_or_else(|| error("image resource is not declared by this plugin"))?;
        out.insert(image.resource.clone(), resource);
    }
    for child in &node.children {
        collect_resources(child, ctx, out)?;
    }
    Ok(())
}
pub fn clear(ctx: &Context) {
    let mut registry = ctx.shared.ui.lock().unwrap();
    let before = registry.surfaces.len();
    registry.surfaces.retain(|_, s| s.instance != ctx.name);
    if registry.surfaces.len() != before {
        registry.revision += 1;
        drop(registry);
        ctx.emit("ui.clear", json!({"generation":ctx.generation}));
    }
}
pub fn install(lua: &Lua, ctx: &Context) -> mlua::Result<()> {
    let ui = lua.create_table()?;
    let c = ctx.clone();
    ui.set(
        "publish",
        lua.create_function(move |_, (id, value): (String, LuaValue)| {
            let surface: Surface = serde_json::from_value(to_ui_json(value)?).map_err(error)?;
            publish(&c, &id, surface)
        })?,
    )?;
    let c = ctx.clone();
    ui.set(
        "update",
        lua.create_function(move |_, (id, value): (String, LuaValue)| {
            let key = format!("{}/{}", c.name, id);
            let mut surface = c
                .shared
                .ui
                .lock()
                .unwrap()
                .surfaces
                .get(&key)
                .ok_or_else(|| error("surface not published"))?
                .surface
                .clone();
            surface.patch(&to_ui_json(value)?).map_err(error)?;
            publish(&c, &id, surface)
        })?,
    )?;
    let c = ctx.clone();
    ui.set(
        "remove",
        lua.create_function(move |_, id: String| {
            let mut registry = c.shared.ui.lock().unwrap();
            let removed = events::remove_tree(&mut registry, &c.name, &id);
            if removed {
                registry.revision += 1;
            }
            drop(registry);
            if removed {
                c.emit("ui.remove", json!({"id":id,"generation":c.generation}));
            }
            Ok(removed)
        })?,
    )?;
    let c = ctx.clone();
    ui.set(
        "hide",
        lua.create_function(move |_, id: String| events::hide_tree(&c, &id).map_err(error))?,
    )?;
    let c = ctx.clone();
    ui.set(
        "list",
        lua.create_function(move |lua, ()| {
            let surfaces = c
                .shared
                .ui
                .lock()
                .unwrap()
                .surfaces
                .values()
                .filter(|s| s.instance == c.name)
                .cloned()
                .collect::<Vec<_>>();
            lua.to_value(&surfaces)
        })?,
    )?;
    let action: mlua::Table = lua.globals().get("action")?;
    // Explicit typed actions already have CPU/wall budgets and instance ownership.
    ui.set("on", action.get::<mlua::Function>("register")?)?;
    lua.globals().set("ui", ui)
}
