use anyhow::{Result, anyhow, bail};
use hidapi::HidApi;
use mlua::{Lua, MultiValue, Table, UserData, UserDataMethods, Value as LValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

fn err(e: impl ToString) -> mlua::Error {
    mlua::Error::RuntimeError(e.to_string())
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn unhex(s: &str) -> mlua::Result<Vec<u8>> {
    let s: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if s.len() > 131072 || !s.len().is_multiple_of(2) || !s.is_ascii() {
        return Err(err("invalid or oversized hex buffer"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(err))
        .collect()
}

#[derive(Default)]
pub struct EventLog {
    sequence: u64,
    entries: VecDeque<Value>,
    origin: Option<Instant>,
}
impl EventLog {
    pub fn push(&mut self, instance: &str, kind: &str, data: Value) {
        let encoded = data.to_string();
        let data = if encoded.len() > 8192 {
            json!({"truncated":true,"preview":encoded.chars().take(2048).collect::<String>()})
        } else {
            data
        };
        self.sequence += 1;
        let elapsed_us = self
            .origin
            .get_or_insert_with(Instant::now)
            .elapsed()
            .as_micros() as u64;
        self.entries.push_back(json!({"seq":self.sequence,"unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,"elapsed_us":elapsed_us,"instance":instance,"kind":kind,"data":data}));
        if self.entries.len() > 512 {
            self.entries.pop_front();
        }
    }
    pub fn read(&self, after: u64) -> Value {
        let oldest = self
            .entries
            .front()
            .and_then(|v| v["seq"].as_u64())
            .unwrap_or(self.sequence + 1);
        let mut size = 0;
        let items: Vec<_> = self
            .entries
            .iter()
            .filter(|v| v["seq"].as_u64().unwrap() > after)
            .take(64)
            .take_while(|v| {
                size += v.to_string().len();
                size <= 128 * 1024
            })
            .collect();
        let cursor = items
            .last()
            .and_then(|v| v["seq"].as_u64())
            .unwrap_or(after);
        json!({"events":items,"cursor":cursor,"latest":self.sequence,"gap":after.saturating_add(1) < oldest})
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct OutputRule {
    pub vendor_id: u16,
    pub product_id: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub interface: i32,
    pub packets: Vec<String>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub output_rules: Vec<OutputRule>,
    #[serde(default)]
    pub hid_rules: Vec<ReportRule>,
    #[serde(default)]
    pub ble_write_rules: Vec<crate::bluetooth::WriteRule>,
    #[serde(default)]
    pub ble_pair_rules: Vec<crate::bluetooth::PairRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gip_write_rules: Vec<crate::gip::WriteRule>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct ReportRule {
    pub operation: String,
    #[serde(flatten)]
    pub device: OutputRule,
}
impl Policy {
    pub fn allows_report(&self, info: &Value, operation: &str, bytes: &[u8]) -> bool {
        self.hid_rules
            .iter()
            .any(|r| r.operation == operation && matches_output(&r.device, info, bytes))
    }
    pub fn allows(&self, info: &Value, bytes: &[u8]) -> bool {
        self.output_rules
            .iter()
            .any(|r| matches_output(r, info, bytes))
    }
}
fn matches_output(r: &OutputRule, info: &Value, bytes: &[u8]) -> bool {
    info["vendor_id"] == r.vendor_id
        && info["product_id"] == r.product_id
        && info["usage_page"] == r.usage_page
        && info["usage"] == r.usage
        && info["interface"] == r.interface
        && r.packets.iter().any(|p| p == &hex(bytes))
}

#[derive(Default)]
pub struct Shared {
    pub environment: Mutex<crate::environment::Environment>,
    pub ui: Mutex<crate::ui::Registry>,
    pub events: Mutex<EventLog>,
    pub leases: Mutex<BTreeMap<String, String>>,
    pub changed: std::sync::Condvar,
    pub observers: Mutex<Vec<std::sync::Weak<crate::queue::Queue>>>,
    pub policy: Policy,
}

impl Shared {
    pub fn emit(&self, instance: &str, kind: &str, data: Value) {
        self.events
            .lock()
            .unwrap()
            .push(instance, kind, data.clone());
        self.changed.notify_all();
        let mut observers = self.observers.lock().unwrap();
        observers.retain(|o| o.strong_count() > 0);
        for o in observers.iter().filter_map(std::sync::Weak::upgrade) {
            o.push(json!({"instance":instance,"kind":kind,"data":data}));
        }
    }
}

pub trait Resource {
    fn close(&self);
    fn closed(&self) -> bool;
}

#[derive(Clone)]
pub struct Context {
    pub policy: Arc<Policy>,
    pub translations: Rc<RefCell<crate::environment::Translations>>,
    pub environment_callback: Rc<RefCell<Option<mlua::Function>>>,
    pub generation: String,
    pub name: String,
    pub shared: Arc<Shared>,
    pub root: PathBuf,
    pub tasks: Rc<RefCell<BTreeMap<u64, crate::scheduler::Task>>>,
    pub paused: Rc<Cell<bool>>,
    pub wake: Rc<tokio::sync::Notify>,
    pub states: Rc<RefCell<BTreeMap<String, Value>>>,
    pub device_state: Rc<RefCell<Option<crate::device_state::Record>>>,
    pub assets: Rc<RefCell<BTreeMap<String, peripheral_ui_protocol::ResourceRef>>>,
    pub output: Rc<RefCell<Vec<String>>>,
    pub budget: Arc<Budget>,
    pub resources: Rc<RefCell<Vec<Rc<dyn Resource>>>>,
    pub next_task: Rc<Cell<u64>>,
    pub started: Instant,
    pub extension: Rc<RefCell<crate::extras::Extension>>,
    pub control: Rc<tokio::sync::Mutex<()>>,
    pub stopping: Rc<Cell<bool>>,
}
impl Context {
    pub fn new(name: String, shared: Arc<Shared>, root: PathBuf) -> Self {
        Self {
            generation: uuid::Uuid::new_v4().to_string(),
            policy: Arc::new(shared.policy.clone()),
            environment_callback: Default::default(),
            translations: Default::default(),
            name,
            shared,
            root,
            tasks: Default::default(),
            paused: Default::default(),
            wake: Default::default(),
            states: Default::default(),
            device_state: Default::default(),
            assets: Default::default(),
            output: Default::default(),
            budget: Default::default(),
            resources: Default::default(),
            next_task: Rc::new(Cell::new(1)),
            started: Instant::now(),
            extension: Default::default(),
            control: Default::default(),
            stopping: Default::default(),
        }
    }
    pub fn emit(&self, kind: &str, data: Value) {
        self.shared.emit(&self.name, kind, data);
    }
    pub fn begin_eval(&self) {
        self.output.borrow_mut().clear();
    }
    pub fn output(&self) -> Vec<String> {
        self.output.borrow().clone()
    }
    pub fn native_bytes(&self) -> usize {
        self.budget.get()
    }
    pub fn connection_count(&self) -> usize {
        self.resources
            .borrow()
            .iter()
            .filter(|r| !r.closed())
            .count()
    }
    pub fn manage(&self, resource: Rc<dyn Resource>) -> mlua::Result<()> {
        let mut resources = self.resources.borrow_mut();
        resources.retain(|r| !r.closed());
        if resources.len() >= 32 {
            resource.close();
            return Err(crate::error::Fault::new(
                "resource_limit",
                "resource.open",
                "maximum 32 live resources per instance",
            )
            .lua());
        }
        resources.push(resource);
        Ok(())
    }
    pub fn cleanup(&self) {
        crate::ui::clear(self);
        for task in self.tasks.borrow_mut().values_mut() {
            task.cancel.cancel();
            if let Some(handle) = task.handle.take() {
                handle.abort();
            }
        }
        self.tasks.borrow_mut().clear();
        for resource in self.resources.borrow().iter() {
            resource.close();
        }
        self.extension.borrow_mut().running = false;
        self.emit("stopped", json!({}));
    }
    pub async fn settle_resources(&self) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(1000);
        while self.connection_count() > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        self.resources.borrow_mut().retain(|r| !r.closed());
    }
    pub fn task_info(&self) -> Value {
        json!(
            self.tasks
                .borrow()
                .iter()
                .map(|(id, t)| t.info(*id))
                .collect::<Vec<_>>()
        )
    }
}

#[derive(Default)]
pub struct Budget(std::sync::atomic::AtomicUsize);
impl Budget {
    pub fn get(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn reserve(&self, n: usize) -> mlua::Result<()> {
        self.0
            .try_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |v| v.checked_add(n).filter(|total| *total <= 1024 * 1024),
            )
            .map_err(|_| {
                crate::error::Fault::new(
                    "resource_limit",
                    "buffer",
                    "native Buffer budget exceeded",
                )
                .lua()
            })?;
        Ok(())
    }
    pub fn release(&self, n: usize) {
        self.0.fetch_sub(n, std::sync::atomic::Ordering::AcqRel);
    }
}
pub struct Charge {
    budget: Arc<Budget>,
    size: usize,
}
impl Charge {
    pub fn new(budget: Arc<Budget>, size: usize) -> mlua::Result<Self> {
        budget.reserve(size)?;
        Ok(Self { budget, size })
    }
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.budget.release(self.size);
    }
}

struct Bytes {
    bytes: Vec<u8>,
    budget: Arc<Budget>,
}
impl Drop for Bytes {
    fn drop(&mut self) {
        self.budget.release(self.bytes.len());
    }
}
#[derive(Clone)]
pub struct Buffer(Rc<Bytes>);
impl Buffer {
    pub fn bytes(&self) -> &[u8] {
        &self.0.bytes
    }
    pub fn budget(&self) -> &Arc<Budget> {
        &self.0.budget
    }
    pub fn new(bytes: Vec<u8>, budget: &Arc<Budget>) -> mlua::Result<Self> {
        if bytes.len() > 65536 || budget.get() + bytes.len() > 1024 * 1024 {
            return Err(err(
                "native Buffer budget exceeded (64 KiB/object, 1 MiB/instance)",
            ));
        }
        budget.reserve(bytes.len())?;
        Ok(Self(Rc::new(Bytes {
            bytes,
            budget: budget.clone(),
        })))
    }
}
impl UserData for Buffer {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        crate::buffer::methods(methods);
        methods.add_method("len", |_, this, ()| Ok(this.0.bytes.len()));
        methods.add_method("hex", |_, this, ()| Ok(hex(&this.0.bytes)));
        methods.add_method("byte", |_, this, offset: usize| {
            this.0
                .bytes
                .get(offset)
                .copied()
                .ok_or_else(|| err("Buffer offset out of range (0-based)"))
        });
        methods.add_method("slice", |_, this, (offset, len): (usize, usize)| {
            let end = offset
                .checked_add(len)
                .ok_or_else(|| err("slice overflow"))?;
            Buffer::new(
                this.0
                    .bytes
                    .get(offset..end)
                    .ok_or_else(|| err("slice out of range"))?
                    .to_vec(),
                &this.0.budget,
            )
        });
        methods.add_method("u16_le", |_, this, offset: usize| {
            let end = offset
                .checked_add(2)
                .ok_or_else(|| err("offset overflow"))?;
            let bytes = this
                .0
                .bytes
                .get(offset..end)
                .ok_or_else(|| err("u16 out of range"))?;
            Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
        });
    }
}

pub fn to_json(value: LValue, depth: usize) -> mlua::Result<Value> {
    let mut remaining = 2048;
    convert(value, depth, 8, &mut remaining)
}
/// UI trees contain a children array between each node level. Keep their bounded
/// conversion separate from general REPL values; Surface::validate checks nodes.
pub fn to_ui_json(value: LValue) -> mlua::Result<Value> {
    convert(value, 0, 32, &mut 8192)
}
fn convert(
    value: LValue,
    depth: usize,
    max_depth: usize,
    remaining: &mut usize,
) -> mlua::Result<Value> {
    if depth > max_depth || *remaining == 0 {
        return Err(err(
            "result nesting/entry budget exceeded (cycles are not serializable)",
        ));
    }
    *remaining -= 1;
    Ok(match value {
        LValue::Nil => Value::Null,
        LValue::LightUserData(p) if p.0.is_null() => Value::Null,
        LValue::Boolean(x) => json!(x),
        LValue::Integer(x) => json!(x),
        LValue::Number(x) => json!(x),
        LValue::String(s) => {
            if s.as_bytes().len() > 16384 {
                return Err(err("result string exceeds 16 KiB"));
            }
            json!(s.to_string_lossy())
        }
        LValue::Table(t) => {
            let mut map = serde_json::Map::new();
            let mut array = BTreeMap::new();
            let mut all_array = true;
            for pair in t.pairs::<LValue, LValue>() {
                let (k, v) = pair?;
                let key = match k {
                    LValue::String(s) => {
                        all_array = false;
                        s.to_str()?.to_string()
                    }
                    LValue::Integer(i) if i > 0 => {
                        array.insert(i, ());
                        i.to_string()
                    }
                    _ => return Err(err("JSON table keys must be strings or positive integers")),
                };
                if key.len() > 256 {
                    return Err(err("table key too long"));
                }
                map.insert(key, convert(v, depth + 1, max_depth, remaining)?);
            }
            if map.is_empty() {
                // Preserve mlua's array marker on empty backend lists. Plain {}
                // remains an object; no recursive serialization is delegated.
                serde_json::to_value(&t).map_err(err)?
            } else if all_array
                && !array.is_empty()
                && array.keys().copied().eq(1..=array.len() as i64)
            {
                Value::Array(
                    (1..=array.len())
                        .map(|i| map.remove(&i.to_string()).unwrap())
                        .collect(),
                )
            } else {
                Value::Object(map)
            }
        }
        LValue::UserData(d) if d.is::<Buffer>() => {
            let b = d.borrow::<Buffer>()?;
            json!({"type":"buffer","length":b.0.bytes.len(),"hex":hex(&b.0.bytes)})
        }
        _ => json!(format!("<{:?}>", value.type_name())),
    })
}

pub fn enumerate(filter: Option<&Value>) -> Result<Value> {
    crate::native_hid::NativeHid::enumerate(filter).map_err(Into::into)
}
pub fn enumerate_local(filter: Option<&Value>) -> Result<Value> {
    let api = HidApi::new()?;
    let list: Vec<_> = api.device_list().map(|d| json!({
        "backend":"hid", "path":d.path().to_string_lossy(),"vendor_id":d.vendor_id(),"product_id":d.product_id(),
        "usage_page":d.usage_page(),"usage":d.usage(),"interface":d.interface_number(),
        "manufacturer":d.manufacturer_string(),"product":d.product_string(),"serial":d.serial_number(),
        "bus":format!("{:?}",d.bus_type()),"physical_identity":"not inferred; path identifies current HID collection"
    })).filter(|d| filter.is_none_or(|f| ["vendor_id","product_id","usage_page","usage","interface","backend","path"].iter().all(|k| f.get(k).is_none_or(|v| d[*k] == *v)))).collect();
    let mut list = list;
    #[cfg(windows)]
    if filter.is_some_and(|f| f.get("vendor_id").is_some() || f.get("path").is_some()) {
        for d in &mut list {
            d["container_id"] = json!(crate::windows_gatt::container_id(
                d["path"].as_str().unwrap()
            ));
        }
    }
    Ok(json!(list))
}

pub fn install(lua: &Lua, ctx: &Context) -> Result<()> {
    // No filesystem/process/native-library access from scripts. Host load/require stays below root.
    for key in ["dofile", "loadfile", "collectgarbage"] {
        lua.globals().set(key, LValue::Nil)?;
    }
    // Script finalizers could run during VM destruction outside any evaluation
    // deadline. Keep lifecycle under the host: copy/protect installed metatables
    // and do not expose mutable metatables for built-in scalar values.
    lua.load(r#"
        local setmt, getmt, kind, check, nextpair = setmetatable, getmetatable, type, assert, next
        function setmetatable(value, meta)
            if meta == nil then return setmt(value, nil) end
            check(kind(meta) == 'table', 'metatable must be a table')
            local copy = {}
            for k,v in nextpair,meta do
                check(k ~= '__gc' and k ~= '__close', 'script finalizers are disabled; use host stop/close')
                copy[k] = v
            end
            copy.__metatable = meta
            return setmt(value, copy)
        end
        function getmetatable(value)
            local meta = getmt(value)
            if kind(value) == 'table' or kind(meta) ~= 'table' then return meta end
            local copy = {}; for k,v in nextpair,meta do copy[k] = v end
            return copy
        end
    "#).exec()?;
    let ctx_print = ctx.clone();
    lua.globals().set(
        "print",
        lua.create_function(move |_, args: MultiValue| {
            let text = args
                .into_iter()
                .map(|v| {
                    to_json(v, 0).map(|v| {
                        if let Some(s) = v.as_str() {
                            s.to_owned()
                        } else {
                            v.to_string()
                        }
                    })
                })
                .collect::<mlua::Result<Vec<_>>>()?
                .join("\t");
            crate::scheduler::record_output(text.clone())?;
            ctx_print.emit("print", json!({"text":text}));
            Ok(())
        })?,
    )?;
    let buffer = lua.create_table()?;
    let budget = ctx.budget.clone();
    buffer.set(
        "hex",
        lua.create_function(move |_, s: String| Buffer::new(unhex(&s)?, &budget))?,
    )?;
    let budget = ctx.budget.clone();
    buffer.set(
        "from",
        lua.create_function(move |_, t: Table| {
            let mut bytes = Vec::new();
            for b in t.sequence_values::<u8>() {
                if bytes.len() >= 65536 {
                    return Err(err("buffer too large"));
                }
                bytes.push(b?);
            }
            Buffer::new(bytes, &budget)
        })?,
    )?;
    crate::buffer::install(lua, &buffer, &ctx.budget)?;
    lua.globals().set("buffer", buffer)?;
    crate::scheduler::install(lua, ctx)?;
    crate::io::install(lua, ctx)?;
    crate::device_state::install(lua, ctx)?;
    crate::bluetooth::install(lua, ctx)?;
    crate::gip::install(lua, ctx)?;
    crate::gamepad::install(lua)?;
    crate::error::install(lua)?;
    crate::extras::install(lua, ctx)?;
    crate::ui::install(lua, ctx)?;
    crate::environment::install(lua, ctx)?;
    let state = lua.create_table()?;
    let context = ctx.clone();
    state.set(
        "publish",
        lua.create_function(
            move |_, (key, value, opts): (String, LValue, Option<Table>)| {
                if key.len() > 128
                    || (!context.states.borrow().contains_key(&key)
                        && context.states.borrow().len() >= 32)
                {
                    return Err(err("state key limit reached"));
                }
                let value = to_json(value, 0)?;
                if value.to_string().len() > 8192 {
                    return Err(err("state value exceeds 8 KiB"));
                }
                let total: usize = context
                    .states
                    .borrow()
                    .iter()
                    .filter(|(k, _)| *k != &key)
                    .map(|(_, v)| v.to_string().len())
                    .sum();
                if total + value.to_string().len() > 65536 {
                    return Err(err("total state budget exceeds 64 KiB"));
                }
                let ttl = opts
                    .as_ref()
                    .map(|o| o.get::<Option<u64>>("ttl_ms"))
                    .transpose()?
                    .flatten();
                let schema = opts
                    .as_ref()
                    .map(|o| o.get::<Option<Table>>("schema"))
                    .transpose()?
                    .flatten()
                    .map(|t| to_json(LValue::Table(t), 0))
                    .transpose()?;
                crate::extras::state_metadata(&context, &key, ttl, schema, &value)?;
                context
                    .states
                    .borrow_mut()
                    .insert(key.clone(), value.clone());
                context.emit("state", json!({"key":key,"value":value}));
                Ok(())
            },
        )?,
    )?;
    lua.globals().set("state", state)?;
    let root = ctx.root.clone();
    lua.globals().set(
        "_module_source",
        lua.create_function(move |_, name: String| {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            {
                return Err(err("invalid module name"));
            }
            crate::runtime::read_source(&root, &format!("scripts/{}.lua", name.replace('.', "/")))
                .map_err(err)
        })?,
    )?;
    lua.load(
        r#"
        local source, cache, loading, manifests = _module_source, {}, {}, {}
        _module_source = nil
        function require(name)
          if cache[name] ~= nil then return cache[name] end
          assert(not loading[name], 'circular or concurrently loading module: '..name)
          loading[name] = true
          local ok, result = pcall(function()
            local chunk = assert(load(source(name), '@'..name, 't', _ENV))
            local result = chunk()
            if type(result)=='table' and result.manifest~=nil then
              local m=result.manifest
              assert(type(m)=='table' and m.api_version==2,'incompatible module API version')
              assert(m.name==name and type(m.version)=='string' and #m.version<=64,'invalid module manifest')
              assert(type(m.dependencies)=='table' and #m.dependencies<=16,'invalid module dependencies')
              for _,dependency in ipairs(m.dependencies) do require(dependency) end
              manifests[name]={name=m.name,version=m.version,api_version=m.api_version}
            end
            return result
          end)
          loading[name] = nil
          if not ok then error(result) end
          if result == nil then result = true end
          cache[name] = result
          return result
        end
        runtime.modules=function()
          local result={}
          for name in pairs(cache) do
            local m=manifests[name]
            result[#result+1]={name=name,version=m and m.version or 'unversioned',api_version=m and m.api_version or 1}
          end
          table.sort(result,function(a,b) return a.name<b.name end)
          return result
        end
    "#,
    )
    .exec()?;
    lua.globals().set("coroutine", LValue::Nil)?;
    Ok(())
}

pub fn load_policy(path: &std::path::Path) -> Result<Policy> {
    if std::fs::metadata(path)?.len() > 128 * 1024 {
        bail!("policy too large");
    }
    let policy: Policy = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    validate_policy(&policy)?;
    Ok(policy)
}

pub fn validate_policy(policy: &Policy) -> Result<()> {
    if policy.output_rules.len() + policy.hid_rules.len() > 64 {
        bail!("too many rules");
    }
    for rule in &policy.hid_rules {
        if !["get_feature", "set_feature", "send_output_report"].contains(&rule.operation.as_str())
        {
            bail!("unknown HID policy operation");
        }
    }
    for rule in policy
        .output_rules
        .iter()
        .chain(policy.hid_rules.iter().map(|r| &r.device))
    {
        if rule.packets.len() > 256 {
            bail!("too many packets");
        }
        for packet in &rule.packets {
            let bytes = unhex(packet).map_err(|e| anyhow!(e.to_string()))?;
            if bytes.is_empty() || bytes.len() > 1024 || hex(&bytes) != *packet {
                bail!("policy packets must be canonical lowercase wire hex, 1..1024 bytes");
            }
        }
    }
    crate::bluetooth::validate_rules(&policy.ble_write_rules)?;
    crate::bluetooth::validate_pair_rules(&policy.ble_pair_rules)?;
    crate::gip::validate_rules(&policy.gip_write_rules)?;
    Ok(())
}
