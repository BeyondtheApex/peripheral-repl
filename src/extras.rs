use crate::{
    bindings::{Context, Resource, to_json},
    error::Fault,
    queue::{Queue, Subscription},
    scheduler,
};
use mlua::{Function, Lua, LuaSerdeExt, MultiValue, Table, Value as LValue};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

pub struct Action {
    schema: Value,
    function: Function,
}
pub struct Extension {
    pub config: Value,
    pub config_schema: Option<Value>,
    pub hooks: BTreeMap<String, Function>,
    pub running: bool,
    initialized: bool,
    actions: BTreeMap<String, Action>,
    state_meta: BTreeMap<String, (Instant, Option<u64>, Option<Value>)>,
}
impl Default for Extension {
    fn default() -> Self {
        Self {
            config: json!({}),
            config_schema: None,
            hooks: BTreeMap::new(),
            running: false,
            initialized: false,
            actions: BTreeMap::new(),
            state_meta: BTreeMap::new(),
        }
    }
}
fn fault(op: &str, msg: impl ToString) -> mlua::Error {
    Fault::new("invalid_argument", op, msg).lua()
}

/// Deliberately bounded JSON-schema subset, rejects unknown schema keywords.
pub fn validate(schema: &Value, value: &Value) -> Result<(), Fault> {
    validate_at(schema, value, "$", 0)
}
fn validate_at(s: &Value, v: &Value, path: &str, depth: usize) -> Result<(), Fault> {
    let fail = |m: String| Fault::new("validation", "schema", format!("{path}: {m}"));
    if depth > 8 {
        return Err(fail("schema nesting exceeded".into()));
    }
    let object = s
        .as_object()
        .ok_or_else(|| fail("schema must be an object".into()))?;
    for key in object.keys() {
        if ![
            "type",
            "properties",
            "required",
            "additionalProperties",
            "items",
            "minimum",
            "maximum",
            "enum",
            "enumNames",
            "description",
            "maxLength",
            "maxItems",
            "default",
        ]
        .contains(&key.as_str())
        {
            return Err(fail(format!("unsupported schema keyword {key}")));
        }
    }
    let kind = s["type"].as_str().unwrap_or("object");
    if let Some(labels) = s.get("enumNames") {
        let labels = labels
            .as_array()
            .ok_or_else(|| fail("enumNames must be an array".into()))?;
        if s["enum"]
            .as_array()
            .is_none_or(|choices| choices.len() != labels.len())
            || labels
                .iter()
                .any(|l| l.as_str().is_none_or(|s| s.len() > 256))
        {
            return Err(fail(
                "enumNames must match enum choices with bounded text labels".into(),
            ));
        }
    }
    let matches = match kind {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => return Err(fail(format!("unsupported type {kind}"))),
    };
    if !matches {
        return Err(fail(format!("expected {kind}")));
    }
    if let Some(allowed) = s["enum"].as_array()
        && !allowed.contains(v)
    {
        return Err(fail("value not in enum".into()));
    }
    if let Some(n) = v.as_f64()
        && (s["minimum"].as_f64().is_some_and(|min| n < min)
            || s["maximum"].as_f64().is_some_and(|max| n > max))
    {
        return Err(fail("number out of range".into()));
    }
    if let Some(text) = v.as_str()
        && text.len() > s["maxLength"].as_u64().unwrap_or(16384) as usize
    {
        return Err(fail("string too long".into()));
    }
    if let Some(array) = v.as_array() {
        if array.len() > s["maxItems"].as_u64().unwrap_or(256) as usize {
            return Err(fail("array too long".into()));
        }
        if let Some(items) = s.get("items") {
            for (i, item) in array.iter().enumerate() {
                validate_at(items, item, &format!("{path}[{i}]"), depth + 1)?;
            }
        }
    }
    if let Some(obj) = v.as_object() {
        if let Some(required) = s["required"].as_array() {
            for key in required {
                let key = key
                    .as_str()
                    .ok_or_else(|| fail("required must contain strings".into()))?;
                if !obj.contains_key(key) {
                    return Err(fail(format!("missing {key}")));
                }
            }
        }
        for (key, value) in obj {
            if let Some(prop) = s["properties"].get(key) {
                validate_at(prop, value, &format!("{path}.{key}"), depth + 1)?;
            } else if s["additionalProperties"] == false {
                return Err(fail(format!("unknown field {key}")));
            }
        }
    }
    Ok(())
}
pub fn actions(ctx: &Context) -> Value {
    json!(
        ctx.extension
            .borrow()
            .actions
            .iter()
            .map(|(name, a)| json!({"name":name,"schema":a.schema}))
            .collect::<Vec<_>>()
    )
}
pub async fn invoke(
    lua: &Lua,
    ctx: &Context,
    req: &Value,
    cancel: CancellationToken,
) -> anyhow::Result<Value> {
    let name = req["name"]
        .as_str()
        .ok_or_else(|| Fault::new("invalid_argument", "invoke", "missing name"))?;
    let (schema, function) = {
        let e = ctx.extension.borrow();
        let a = e
            .actions
            .get(name)
            .ok_or_else(|| Fault::new("not_found", "invoke", "unknown action"))?;
        (a.schema.clone(), a.function.clone())
    };
    let args = req.get("arguments").cloned().unwrap_or(json!({}));
    validate(&schema, &args)?;
    let values = scheduler::execute(
        lua,
        function,
        MultiValue::from_vec(vec![lua.to_value(&args)?]),
        req["timeout_ms"].as_u64().unwrap_or(30000),
        500,
        cancel,
    )
    .await?;
    let values = values
        .into_iter()
        .map(|v| to_json(v, 0))
        .collect::<mlua::Result<Vec<_>>>()?;
    Ok(json!({"values":values}))
}
pub async fn lifecycle(
    lua: &Lua,
    ctx: &Context,
    phase: &str,
    cancel: CancellationToken,
) -> mlua::Result<()> {
    let function = ctx.extension.borrow().hooks.get(phase).cloned();
    if let Some(function) = function {
        scheduler::execute(lua, function, MultiValue::new(), 2000, 500, cancel).await?;
    }
    ctx.emit("lifecycle", json!({"phase":phase}));
    Ok(())
}
pub async fn start(lua: &Lua, ctx: &Context, cancel: CancellationToken) -> mlua::Result<()> {
    if ctx.extension.borrow().running {
        return Ok(());
    }
    if !ctx.extension.borrow().initialized {
        lifecycle(lua, ctx, "init", cancel.clone()).await?;
        ctx.extension.borrow_mut().initialized = true;
    }
    lifecycle(lua, ctx, "start", cancel).await?;
    ctx.extension.borrow_mut().running = true;
    Ok(())
}
pub async fn configure(
    lua: &Lua,
    ctx: &Context,
    value: Value,
    cancel: CancellationToken,
) -> mlua::Result<()> {
    if !value.is_object() || value.to_string().len() > 16384 {
        return Err(fault("config", "config must be an object <=16 KiB"));
    }
    if let Some(schema) = &ctx.extension.borrow().config_schema {
        validate(schema, &value).map_err(Fault::lua)?;
    }
    let (previous, previous_schema) = {
        let mut e = ctx.extension.borrow_mut();
        (
            std::mem::replace(&mut e.config, value),
            e.config_schema.clone(),
        )
    };
    let function = ctx.extension.borrow().hooks.get("configure").cloned();
    if let Some(function) = function {
        let value = ctx.extension.borrow().config.clone();
        if let Err(error) = scheduler::execute(
            lua,
            function,
            MultiValue::from_vec(vec![lua.to_value(&value)?]),
            2000,
            500,
            cancel,
        )
        .await
        {
            let mut e = ctx.extension.borrow_mut();
            e.config = previous;
            e.config_schema = previous_schema;
            return Err(error);
        }
    }
    let value = ctx.extension.borrow().config.clone();
    ctx.emit("config", value);
    Ok(())
}
pub fn metadata(ctx: &Context) -> Value {
    let values = ctx.extension.borrow().state_meta.iter().map(|(key,(at,ttl,schema))| {
        (key.clone(),json!({"age_ms":at.elapsed().as_millis() as u64,"ttl_ms":ttl,"stale":ttl.is_some_and(|ms|at.elapsed()>=Duration::from_millis(ms)),"schema":schema}))
    }).collect::<serde_json::Map<String,Value>>();
    Value::Object(values)
}
pub fn snapshot(ctx: &Context) -> Value {
    let mut states = ctx.states.borrow().clone();
    let ext = ctx.extension.borrow();
    for (key, (at, ttl, _)) in &ext.state_meta {
        if let Some(v) = states.get_mut(key)
            && let Some(object) = v.as_object_mut()
        {
            object.insert("_meta".into(),json!({"age_ms":at.elapsed().as_millis() as u64,"stale":ttl.is_some_and(|ms|at.elapsed()>=Duration::from_millis(ms))}));
        }
    }
    json!(states)
}
struct StreamResource(Arc<Queue>);
impl Resource for StreamResource {
    fn close(&self) {
        self.0.close();
    }
    fn closed(&self) -> bool {
        self.0.is_closed()
    }
}

pub fn install(lua: &Lua, ctx: &Context) -> mlua::Result<()> {
    let log = lua.create_table()?;
    for level in ["debug", "info", "warn", "error"] {
        let c = ctx.clone();
        log.set(
            level,
            lua.create_function(move |_, (message, data): (String, Option<LValue>)| {
                if message.len() > 4096 {
                    return Err(fault("log", "message too long"));
                }
                let data = data.map(|v| to_json(v, 0)).transpose()?;
                c.emit(
                    "log",
                    json!({"level":level,"message":message,"fields":data}),
                );
                Ok(())
            })?,
        )?;
    }
    lua.globals().set("log", log)?;
    let config = lua.create_table()?;
    let c = ctx.clone();
    config.set(
        "get",
        lua.create_function(move |lua, ()| lua.to_value(&c.extension.borrow().config))?,
    )?;
    let c = ctx.clone();
    config.set(
        "define",
        lua.create_function(move |_, (schema, defaults): (Table, Table)| {
            let schema = to_json(LValue::Table(schema), 0)?;
            let defaults = to_json(LValue::Table(defaults), 0)?;
            if schema.to_string().len() > 16384 || defaults.to_string().len() > 16384 {
                return Err(fault(
                    "config.define",
                    "configuration description exceeds 16 KiB",
                ));
            }
            validate(&schema, &defaults).map_err(Fault::lua)?;
            let mut merged = defaults
                .as_object()
                .cloned()
                .ok_or_else(|| fault("config.define", "defaults must be object"))?;
            if let Some(current) = c.extension.borrow().config.as_object() {
                // Dynamic device inventories may remove fields or change enum choices.
                // Retain still-valid user choices, otherwise use the new Lua defaults.
                for (key, value) in current {
                    if let Some(property) = schema["properties"].get(key) {
                        if validate(property, value).is_ok() {
                            merged.insert(key.clone(), value.clone());
                        }
                    } else if schema["additionalProperties"] != false {
                        merged.insert(key.clone(), value.clone());
                    }
                }
            }
            let value = json!(merged);
            validate(&schema, &value).map_err(Fault::lua)?;
            let mut e = c.extension.borrow_mut();
            e.config_schema = Some(schema);
            e.config = value;
            drop(e);
            c.emit("config.defined", json!({}));
            Ok(())
        })?,
    )?;
    lua.globals().set("config", config)?;
    let lifecycle = lua.create_table()?;
    let c = ctx.clone();
    lifecycle.set(
        "on",
        lua.create_function(move |_, (phase, f): (String, Function)| {
            if !["init", "start", "stop", "unload", "configure"].contains(&phase.as_str()) {
                return Err(fault("lifecycle.on", "unknown phase"));
            }
            c.extension.borrow_mut().hooks.insert(phase, f);
            Ok(())
        })?,
    )?;
    let c = ctx.clone();
    lifecycle.set(
        "start",
        lua.create_async_function(move |lua, ()| {
            let c = c.clone();
            async move {
                self::start(&lua, &c, CancellationToken::new()).await?;
                Ok(())
            }
        })?,
    )?;
    lua.globals().set("lifecycle", lifecycle)?;
    let action = lua.create_table()?;
    let c = ctx.clone();
    action.set(
        "register",
        lua.create_function(
            move |_, (name, schema, function): (String, Table, Function)| {
                if name.is_empty() || name.len() > 64 || c.extension.borrow().actions.len() >= 32 {
                    return Err(fault("action.register", "invalid name or action limit"));
                }
                let schema = to_json(LValue::Table(schema), 0)?;
                if schema.to_string().len() > 8192 {
                    return Err(fault("action.register", "schema too large"));
                }
                c.extension
                    .borrow_mut()
                    .actions
                    .insert(name, Action { schema, function });
                Ok(())
            },
        )?,
    )?;
    lua.globals().set("action", action)?;
    let events = lua.create_table()?;
    let c = ctx.clone();
    events.set(
        "subscribe",
        lua.create_function(move |_, capacity: Option<usize>| {
            let queue = Queue::new(capacity.unwrap_or(64));
            c.shared
                .observers
                .lock()
                .unwrap()
                .push(Arc::downgrade(&queue));
            c.manage(Rc::new(StreamResource(queue.clone())))?;
            Ok(Subscription(queue))
        })?,
    )?;
    lua.globals().set("events", events)?;
    let storage = lua.create_table()?;
    let c = ctx.clone();
    storage.set(
        "get",
        lua.create_function(move |lua, key: String| {
            let path = storage_path(&c, &key)?;
            if !path.exists() {
                return Ok(LValue::Nil);
            }
            let metadata = std::fs::symlink_metadata(&path).map_err(mlua::Error::external)?;
            if metadata.file_type().is_symlink() || metadata.len() > 16384 {
                return Err(fault("storage.get", "invalid storage file"));
            }
            let value: Value = serde_json::from_str(
                &std::fs::read_to_string(path).map_err(mlua::Error::external)?,
            )
            .map_err(mlua::Error::external)?;
            lua.to_value(&value)
        })?,
    )?;
    let c = ctx.clone();
    storage.set(
        "set",
        lua.create_function(move |_, (key, value): (String, LValue)| {
            let path = storage_path(&c, &key)?;
            let text = to_json(value, 0)?.to_string();
            if text.len() > 16384 {
                return Err(fault("storage.set", "value exceeds 16 KiB"));
            }
            let dir = path.parent().unwrap();
            std::fs::create_dir_all(dir).map_err(mlua::Error::external)?;
            let base = c
                .root
                .join(".peripheral")
                .canonicalize()
                .map_err(mlua::Error::external)?;
            if !dir
                .canonicalize()
                .map_err(mlua::Error::external)?
                .starts_with(&base)
            {
                return Err(fault("storage.set", "storage path escapes root"));
            }
            if !path.exists()
                && std::fs::read_dir(dir)
                    .map_err(mlua::Error::external)?
                    .count()
                    >= 32
            {
                return Err(fault("storage.set", "32 storage key limit"));
            }
            if path.exists()
                && std::fs::symlink_metadata(&path)
                    .map_err(mlua::Error::external)?
                    .file_type()
                    .is_symlink()
            {
                return Err(fault("storage.set", "symlink denied"));
            }
            use std::io::Write;
            let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(mlua::Error::external)?;
            temp.write_all(text.as_bytes())
                .map_err(mlua::Error::external)?;
            temp.as_file().sync_all().map_err(mlua::Error::external)?;
            temp.persist(&path).map_err(mlua::Error::external)?;
            Ok(())
        })?,
    )?;
    lua.globals().set("storage", storage)?;
    Ok(())
}
fn storage_path(ctx: &Context, key: &str) -> mlua::Result<std::path::PathBuf> {
    if key.is_empty()
        || key.len() > 64
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(fault("storage", "invalid key"));
    }
    let mut path = ctx.root.clone();
    for component in [".peripheral", "storage", &ctx.name, &format!("{key}.json")] {
        path.push(component);
        if let Ok(metadata) = std::fs::symlink_metadata(&path)
            && (metadata.file_type().is_symlink()
                || !path
                    .canonicalize()
                    .map_err(mlua::Error::external)?
                    .starts_with(&ctx.root))
        {
            return Err(fault(
                "storage",
                "storage path escapes root or contains a link",
            ));
        }
    }
    Ok(path)
}
pub fn state_metadata(
    ctx: &Context,
    key: &str,
    ttl: Option<u64>,
    schema: Option<Value>,
    value: &Value,
) -> mlua::Result<()> {
    let mut e = ctx.extension.borrow_mut();
    let existing = e.state_meta.get(key).and_then(|(_, _, s)| s.clone());
    let schema = schema.or(existing);
    if let Some(s) = &schema {
        validate(s, value).map_err(Fault::lua)?;
    }
    e.state_meta
        .insert(key.into(), (Instant::now(), ttl, schema));
    Ok(())
}
