//! One logical device per plugin. Transport availability and business state are distinct.
use crate::bindings::{Context, to_json};
use anyhow::{Result, ensure};
use mlua::{Lua, LuaSerdeExt, Table, Value as LuaValue};
use peripheral_ui_protocol::{ImageSpec, identifier};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub image: Option<ImageSpec>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    pub status: String,
    pub label: String,
    pub detail: String,
    pub connected: Option<bool>,
    pub receiver_present: Option<bool>,
    pub battery_percent: Option<f32>,
    pub image: Option<ImageSpec>,
    pub values: BTreeMap<String, Value>,
    pub ttl_ms: u64,
}
impl Default for State {
    fn default() -> Self {
        Self {
            status: "unknown".into(),
            label: "等待设备状态".into(),
            detail: String::new(),
            connected: None,
            receiver_present: None,
            battery_percent: None,
            image: None,
            values: BTreeMap::new(),
            ttl_ms: 30000,
        }
    }
}
pub struct Record {
    pub descriptor: Descriptor,
    pub state: State,
    updated: Instant,
    updated_unix_ms: u64,
    last_seen_unix_ms: Option<u64>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn fault(e: impl ToString) -> mlua::Error {
    crate::error::Fault::new("validation", "device.state", e).lua()
}
fn check_image(ctx: &Context, image: &Option<ImageSpec>) -> Result<()> {
    if let Some(image) = image {
        image.validate()?;
        ensure!(
            ctx.assets.borrow().contains_key(&image.resource),
            "image resource is not declared in this plugin"
        );
    }
    Ok(())
}
pub fn snapshot(ctx: &Context) -> Value {
    let record = ctx.device_state.borrow();
    let Some(record) = record.as_ref() else {
        return Value::Null;
    };
    let image = record
        .state
        .image
        .as_ref()
        .or(record.descriptor.image.as_ref());
    let resource = image.and_then(|i| ctx.assets.borrow().get(&i.resource).cloned());
    // Presentation is derived from immutable measurement data. Changing language
    // never renews the TTL, last-seen time or raw state used by debugging clients.
    let locale = ctx.shared.environment.lock().unwrap().locale.clone();
    let translations = ctx.translations.borrow();
    let text = |key: &str| translations.text(&locale, key);
    let values=record.state.values.iter().map(|(key,value)|{
        json!({"key":key,"label":text(key),"value":value.as_str().map(|v|json!(text(v))).unwrap_or_else(||value.clone())})
    }).collect::<Vec<_>>();
    let display = json!({"name":text(&record.descriptor.name),"label":text(&record.state.label),"detail":text(&record.state.detail),"values":values});
    json!({"descriptor":record.descriptor,"state":record.state,"image":image,"resource":resource,
        "display":display,
        "generation":ctx.generation,"updated_unix_ms":record.updated_unix_ms,
        "last_seen_unix_ms":record.last_seen_unix_ms,
        "stale":record.updated.elapsed().as_millis() >= record.state.ttl_ms as u128})
}
pub fn install(lua: &Lua, ctx: &Context) -> mlua::Result<()> {
    if ctx.name.starts_with("plugin_") {
        let manifest = crate::plugins::package::read(&ctx.root).map_err(fault)?;
        *ctx.assets.borrow_mut() =
            crate::plugins::resources::resolve(&ctx.root, &manifest.resources).map_err(fault)?;
    }
    let resources = lua.create_table()?;
    let c = ctx.clone();
    resources.set(
        "list",
        lua.create_function(move |lua, ()| {
            lua.to_value(&c.assets.borrow().keys().collect::<Vec<_>>())
        })?,
    )?;
    lua.globals().set("resource", resources)?;
    let device: Table = lua.globals().get("device")?;
    let c = ctx.clone();
    device.set("register", lua.create_function(move |_, value: LuaValue| {
        let descriptor: Descriptor = serde_json::from_value(to_json(value, 0)?).map_err(fault)?;
        if !identifier(&descriptor.id) || descriptor.name.is_empty() || descriptor.name.len()>120 || descriptor.kind.len()>64 {
            return Err(fault("invalid device identity"));
        }
        check_image(&c, &descriptor.image).map_err(fault)?;
        let mut state = c.device_state.borrow_mut();
        if let Some(record) = state.as_mut() {
            if record.descriptor.id != descriptor.id { return Err(fault("one device per plugin; receivers, batteries and docks belong in its values")); }
            record.descriptor = descriptor;
        } else {
            *state = Some(Record {descriptor,state:State::default(),updated:Instant::now(),updated_unix_ms:now(),last_seen_unix_ms:None});
        }
        drop(state);
        c.emit("device.changed", snapshot(&c));
        Ok(())
    })?)?;
    let c = ctx.clone();
    device.set(
        "update",
        lua.create_function(move |_, value: LuaValue| {
            let value = to_json(value, 0)?;
            if serde_json::to_vec(&value).map_err(fault)?.len() > 4096 {
                return Err(fault("device state exceeds 4 KiB"));
            }
            let next: State = serde_json::from_value(value).map_err(fault)?;
            if !identifier(&next.status)
                || next.label.len() > 120
                || next.detail.len() > 512
                || !(100..=86400000).contains(&next.ttl_ms)
                || next.values.len() > 16
                || next
                    .battery_percent
                    .is_some_and(|v| !v.is_finite() || !(0. ..=100.).contains(&v))
            {
                return Err(fault("invalid device state"));
            }
            check_image(&c, &next.image).map_err(fault)?;
            let mut state = c.device_state.borrow_mut();
            let record = state
                .as_mut()
                .ok_or_else(|| fault("register the device before reporting its state"))?;
            record.updated = Instant::now();
            record.updated_unix_ms = now();
            if next.connected == Some(true) {
                record.last_seen_unix_ms = Some(record.updated_unix_ms);
            }
            record.state = next;
            drop(state);
            c.emit("device.changed", snapshot(&c));
            Ok(())
        })?,
    )?;
    let c = ctx.clone();
    device.set(
        "state",
        lua.create_function(move |lua, ()| lua.to_value(&snapshot(&c)))?,
    )?;
    Ok(())
}
