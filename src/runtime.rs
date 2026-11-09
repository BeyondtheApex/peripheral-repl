use crate::{
    bindings::{self, Context, Shared},
    error::Fault,
    scheduler,
};
use anyhow::{Result, anyhow, bail};
use mlua::{Lua, LuaOptions, MultiValue, StdLib};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub const MAX_SOURCE: usize = 65536;
pub const MAX_INSTANCES: usize = 100;
const LUA_MEMORY: usize = 8 * 1024 * 1024;
struct Job {
    op: Value,
    reply: mpsc::SyncSender<Result<Value>>,
}
struct Instance {
    tx: tokio::sync::mpsc::Sender<Job>,
    cancel: CancellationToken,
    thread: Option<thread::JoinHandle<()>>,
}
struct Inner {
    identity: String,
    plugins: crate::plugins::Manager,
    lifecycle: Mutex<()>,
    instances: Mutex<BTreeMap<String, Instance>>,
    root: PathBuf,
}
#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
    pub shared: Arc<Shared>,
}
impl Runtime {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let shared = Shared::default();
        if let Ok(bytes) = std::fs::read(root.as_ref().join(".peripheral/ui-environment.json")) {
            if bytes.len() <= 2048 {
                if let Ok(env) = serde_json::from_slice::<crate::environment::Environment>(&bytes) {
                    if env.validate().is_ok() {
                        *shared.environment.lock().unwrap() = env;
                    }
                }
            }
        }
        Ok(Self {
            inner: Arc::new(Inner {
                identity: uuid::Uuid::new_v4().to_string(),
                plugins: crate::plugins::Manager::new(&root.as_ref().canonicalize()?)?,
                lifecycle: Mutex::new(()),
                instances: Mutex::new(BTreeMap::new()),
                root: root.as_ref().canonicalize()?,
            }),
            shared: Arc::new(shared),
        })
    }
    pub fn dispatch(&self, req: Value) -> Result<Value> {
        let op = req["op"]
            .as_str()
            .ok_or_else(|| Fault::new("invalid_argument", "command", "missing op"))?;
        if op == "plugins" || op.starts_with("plugin_") || op == "plugins_restore" {
            return self.inner.plugins.dispatch(self, &self.inner.root, &req);
        }
        let name = req["instance"].as_str().unwrap_or("default").to_owned();
        let _lifecycle = matches!(op, "create" | "destroy" | "reset" | "reload")
            .then(|| self.inner.lifecycle.lock().unwrap());
        match op {
            "ui_environment" => {
                if req.get("value").is_none() {
                    return Ok(self.shared.environment.lock().unwrap().value());
                }
                let env: crate::environment::Environment =
                    serde_json::from_value(req["value"].clone())?;
                env.validate()?;
                if *self.shared.environment.lock().unwrap() == env {
                    return Ok(json!({"changed":false,"environment":env.value()}));
                }
                let dir = self.inner.root.join(".peripheral");
                std::fs::create_dir_all(&dir)?;
                anyhow::ensure!(
                    dir.canonicalize()?.starts_with(&self.inner.root),
                    "preferences escape workspace"
                );
                use std::io::Write;
                let mut file = tempfile::NamedTempFile::new_in(&dir)?;
                file.write_all(&serde_json::to_vec(&env)?)?;
                file.persist(dir.join("ui-environment.json"))
                    .map_err(|e| e.error)?;
                *self.shared.environment.lock().unwrap() = env.clone();
                self.shared.emit("host", "ui.environment", env.value());
                let names = self
                    .inner
                    .instances
                    .lock()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>();
                let mut errors = BTreeMap::new();
                for name in names {
                    if let Err(e) = self.call(&name, json!({"op":"environment_changed"})) {
                        errors.insert(name, e.to_string());
                    }
                }
                Ok(json!({"changed":true,"environment":env.value(),"callback_errors":errors}))
            }
            "scripts" => crate::management::scripts(&self.inner.root),
            "script_read" => crate::management::read(
                &self.inner.root,
                req["path"]
                    .as_str()
                    .ok_or_else(|| anyhow!("missing path"))?,
            ),
            "script_import" => crate::management::import(
                &self.inner.root,
                req["name"]
                    .as_str()
                    .ok_or_else(|| anyhow!("missing name"))?,
                req["code"]
                    .as_str()
                    .ok_or_else(|| anyhow!("missing source"))?,
            ),
            "ui_snapshot" => Ok(self.shared.ui.lock().unwrap().snapshot()),
            "reset" => {
                let config = self.call(&name, json!({"op":"config"}))?;
                self.destroy(&name)?;
                self.create(&name)?;
                self.call(&name, json!({"op":"configure","value":config}))?;
                Ok(json!({"reset":name}))
            }
            "ping" => Ok(
                json!({"runtime_id":self.inner.identity,"ui_protocol":peripheral_ui_protocol::VERSION,"plugin_api":2,"version":env!("CARGO_PKG_VERSION"),"api_version":2,"policy":if self.shared.policy.name.is_empty(){"read-only"}else{&self.shared.policy.name},"root":self.inner.root,"transports":["hid","ble","mock"]}),
            ),
            "instances" => Ok(json!(
                self.inner
                    .instances
                    .lock()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
            )),
            "devices" => {
                if req["filter"]["backend"] == "ble" {
                    crate::bluetooth::list_sync(req.get("filter").cloned().unwrap_or(json!({})))
                } else {
                    bindings::enumerate(req.get("filter"))
                }
            }
            "bluetooth" => crate::bluetooth::command_sync(req),
            "events" => {
                let after = req["after"].as_u64().unwrap_or(0);
                let wait = req["wait_ms"].as_u64().unwrap_or(0).min(30000);
                let guard = self.shared.events.lock().unwrap();
                let (guard, _) = self
                    .shared
                    .changed
                    .wait_timeout_while(guard, Duration::from_millis(wait), |log| {
                        log.read(after)["events"]
                            .as_array()
                            .is_some_and(Vec::is_empty)
                    })
                    .unwrap();
                Ok(guard.read(after))
            }
            "create" => {
                anyhow::ensure!(
                    !name.starts_with("plugin_"),
                    "plugin_ sessions are managed by plugins; use plugin_enable"
                );
                self.create(&name)?;
                if let Some(config) = req.get("config")
                    && let Err(e) = self.call(&name, json!({"op":"configure","value":config}))
                {
                    let _ = self.destroy(&name);
                    return Err(e);
                }
                Ok(json!({"instance":name}))
            }
            "destroy" => {
                self.destroy(&name)?;
                Ok(json!({"destroyed":name}))
            }
            "reload" | "load" => {
                let path = req["path"]
                    .as_str()
                    .ok_or_else(|| Fault::new("invalid_argument", op, "missing path"))?;
                let code = read_source(&self.inner.root, path)?;
                if op == "reload" {
                    let config = req
                        .get("config")
                        .cloned()
                        .or_else(|| self.call(&name, json!({"op":"config"})).ok());
                    if self.inner.instances.lock().unwrap().contains_key(&name) {
                        self.destroy(&name)?;
                    }
                    self.create(&name)?;
                    if let Some(config) = config {
                        self.call(&name, json!({"op":"configure","value":config}))?;
                    }
                }
                self.call(&name,json!({"op":"eval","code":code,"label":path,"timeout_ms":req["timeout_ms"],"cpu_ms":req["cpu_ms"]}))
            }
            "eval" | "stats" | "tasks" | "pause" | "resume" | "stop" | "snapshot" | "cancel"
            | "actions" | "invoke" | "configure" | "config" | "config_description" | "ui_event"
            | "ui_dismiss" | "start" | "lifecycle" | "device_snapshot" => self.call(&name, req),
            _ => Err(Fault::new(
                "invalid_argument",
                "command",
                format!("unknown operation: {op}"),
            )
            .into()),
        }
    }
    fn create(&self, name: &str) -> Result<()> {
        self.create_at(name, self.inner.root.clone())
    }
    pub(crate) fn create_plugin_session(&self, name: &str, path: &Path) -> Result<()> {
        self.create_at(name, path.canonicalize()?)
    }
    fn create_at(&self, name: &str, session_root: PathBuf) -> Result<()> {
        if name.is_empty()
            || name.len() > 64
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            bail!("invalid instance name");
        }
        let mut instances = self.inner.instances.lock().unwrap();
        if instances.contains_key(name) {
            bail!("instance already exists: {name}");
        }
        if instances.len() >= MAX_INSTANCES {
            bail!("instance limit reached");
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Job>(32);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let cancel = CancellationToken::new();
        let (name_owned, shared, root, shutdown) = (
            name.to_owned(),
            self.shared.clone(),
            session_root,
            cancel.clone(),
        );
        let handle=thread::Builder::new().name(format!("lua-{name}")).stack_size(2*1024*1024).spawn(move||{
            let rt=tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
            let local=tokio::task::LocalSet::new();
            local.block_on(&rt,async move{
                let setup=(||->Result<(Lua,Context)>{
                    let lua=Lua::new_with(StdLib::TABLE|StdLib::STRING|StdLib::MATH|StdLib::UTF8,LuaOptions::default().catch_rust_panics(false))?;
                    lua.set_memory_limit(LUA_MEMORY)?;let ctx=Context::new(name_owned,shared,root);bindings::install(&lua,&ctx)?;Ok((lua,ctx))
                })();
                let(lua,ctx)=match setup{Ok(x)=>{let _=ready_tx.send(Ok(()));x},Err(e)=>{let _=ready_tx.send(Err(e.to_string()));return;}};
                let operations=std::rc::Rc::new(std::cell::RefCell::new(BTreeMap::<String,CancellationToken>::new()));
                let mut inflight=tokio::task::JoinSet::new();
                loop{
                    tokio::select!{
                        biased;
                        _=shutdown.cancelled()=>break,
                        job=rx.recv()=>{
                            let Some(job)=job else{break};
                            if job.op["op"]=="cancel" && job.op["request_id"].is_string(){
                                let key=job.op["request_id"].as_str().unwrap();let found=operations.borrow().get(key).cloned();
                                if let Some(token)=&found{token.cancel();}
                                let _=job.reply.send(Ok(json!({"cancelled":found.is_some()})));continue;
                            }
                            if ctx.stopping.get() && !matches!(job.op["op"].as_str(),Some("stats"|"tasks"|"snapshot"|"lifecycle"|"device_snapshot")) {
                                let _=job.reply.send(Err(Fault::new("busy","instance","instance is stopping").into()));continue;
                            }
                            if job.op["op"]=="stop"{for token in operations.borrow().values(){token.cancel();}}
                            if inflight.len()>=16{let _=job.reply.send(Err(Fault::new("busy","instance.queue","maximum 16 concurrent commands").into()));continue;}
                            let token=shutdown.child_token();
                            let key=job.op["request_id"].as_str().map(str::to_owned).unwrap_or_else(||uuid::Uuid::new_v4().to_string());
                            if operations.borrow().contains_key(&key){let _=job.reply.send(Err(Fault::new("busy","command","request_id is already active").into()));continue;}
                            operations.borrow_mut().insert(key.clone(),token.clone());
                            let(lua,context,ops)=(lua.clone(),ctx.clone(),operations.clone());
                            let is_stop = job.op["op"]=="stop";
                            if is_stop {ctx.stopping.set(true);}
                            inflight.spawn_local(async move{
                                struct ResetStop(std::rc::Rc<std::cell::Cell<bool>>, bool);
                                impl Drop for ResetStop {fn drop(&mut self){if self.1 {self.0.set(false);}}}
                                let _reset_stop=ResetStop(context.stopping.clone(),is_stop);

                                let result=handle_job(&lua,&context,&job.op,token).await;
                                ops.borrow_mut().remove(&key);let _=job.reply.send(result);
                            });
                        },
                        _=inflight.join_next(),if !inflight.is_empty()=>{},
                    }
                }
                for token in operations.borrow().values(){token.cancel();}
                inflight.abort_all();while inflight.join_next().await.is_some(){}
                let _=crate::extras::lifecycle(&lua,&ctx,"stop",CancellationToken::new()).await;
                ctx.cleanup();
                ctx.settle_resources().await;
                let _=crate::extras::lifecycle(&lua,&ctx,"unload",CancellationToken::new()).await;
            });
        })?;
        ready_rx
            .recv_timeout(Duration::from_secs(10))?
            .map_err(|e| anyhow!(e))?;
        instances.insert(
            name.into(),
            Instance {
                tx,
                cancel,
                thread: Some(handle),
            },
        );
        self.shared.emit(name, "session.created", json!({}));
        Ok(())
    }
    fn call(&self, name: &str, op: Value) -> Result<Value> {
        let tx = self
            .inner
            .instances
            .lock()
            .unwrap()
            .get(name)
            .ok_or_else(|| {
                Fault::new(
                    "not_found",
                    "instance",
                    format!("unknown instance: {name}; use create first"),
                )
            })?
            .tx
            .clone();
        let wall = op["timeout_ms"].as_u64().unwrap_or(30000).clamp(1, 120000);
        let (reply, rx) = mpsc::sync_channel(1);
        tx.try_send(Job { op, reply })
            .map_err(|e| Fault::new("busy", "instance.queue", e))?;
        rx.recv_timeout(Duration::from_millis(wall + 5000))
            .map_err(|_| {
                Fault::new(
                    "stalled",
                    "instance",
                    "worker did not respond; no replacement was spawned",
                )
            })?
    }
    fn destroy(&self, name: &str) -> Result<()> {
        let mut instance = self
            .inner
            .instances
            .lock()
            .unwrap()
            .remove(name)
            .ok_or_else(|| Fault::new("not_found", "destroy", "unknown instance"))?;
        instance.cancel.cancel();
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !instance.thread.as_ref().unwrap().is_finished()
            && std::time::Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        if !instance.thread.as_ref().unwrap().is_finished() {
            self.inner
                .instances
                .lock()
                .unwrap()
                .insert(name.into(), instance);
            return Err(Fault::new(
                "stalled",
                "destroy",
                "instance retained until native call returns",
            )
            .into());
        }
        instance
            .thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| anyhow!("instance thread panicked"))?;
        self.shared.emit(name, "session.destroyed", json!({}));
        Ok(())
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        let instances = std::mem::take(self.instances.get_mut().unwrap());
        for i in instances.values() {
            i.cancel.cancel();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        for (_, mut i) in instances {
            if let Some(t) = i.thread.take() {
                while !t.is_finished() && std::time::Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                if t.is_finished() {
                    let _ = t.join();
                }
            }
        }
    }
}
pub fn read_source(root: &Path, path: &str) -> Result<String> {
    let path = root.join(path).canonicalize()?;
    if !path.starts_with(root) {
        bail!("script path is outside configured root");
    }
    if std::fs::metadata(&path)?.len() > MAX_SOURCE as u64 {
        bail!("source exceeds 64 KiB");
    }
    let source = std::fs::read_to_string(path)?;
    if source.len() > MAX_SOURCE {
        bail!("source exceeds 64 KiB");
    }
    Ok(source)
}
async fn handle_job(
    lua: &Lua,
    ctx: &Context,
    op: &Value,
    cancel: CancellationToken,
) -> Result<Value> {
    if op["op"] == "invoke" && op.get("generation").is_some() && op["generation"] != ctx.generation
    {
        bail!("stale UI action: session was reset or reloaded");
    }
    let _control = if matches!(op["op"].as_str(), Some("stop" | "start" | "configure")) {
        Some(ctx.control.lock().await)
    } else {
        None
    };
    match op["op"].as_str().unwrap_or("") {
        "environment_changed" => {
            use mlua::LuaSerdeExt;
            if ctx.device_state.borrow().is_some() {
                ctx.emit("device.changed", crate::device_state::snapshot(ctx));
            }
            let callback = ctx.environment_callback.borrow().clone();
            if let Some(callback) = callback {
                let value = ctx.shared.environment.lock().unwrap().value();
                scheduler::execute(
                    lua,
                    callback,
                    MultiValue::from_vec(vec![lua.to_value(&value)?]),
                    2000,
                    100,
                    cancel,
                )
                .await?;
            }
            Ok(json!({"notified":true}))
        }
        "eval" => {
            let code = op["code"].as_str().ok_or_else(|| anyhow!("missing code"))?;
            if code.len() > MAX_SOURCE {
                bail!("source exceeds 64 KiB");
            }
            let label = op["label"].as_str().unwrap_or("=repl");
            let function = lua
                .load(format!("return {code}"))
                .set_name(label)
                .into_function()
                .or_else(|_| lua.load(code).set_name(label).into_function())?;
            let output = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let values = scheduler::OUTPUT
                .scope(
                    output.clone(),
                    scheduler::execute(
                        lua,
                        function,
                        MultiValue::new(),
                        op["timeout_ms"].as_u64().unwrap_or(30000),
                        op["cpu_ms"].as_u64().unwrap_or(500),
                        cancel,
                    ),
                )
                .await?;
            let values = values
                .into_iter()
                .map(|v| bindings::to_json(v, 0))
                .collect::<mlua::Result<Vec<_>>>()?;
            if serde_json::to_vec(&values)?.len() > 65536 {
                bail!("evaluation result exceeds 64 KiB");
            }
            Ok(
                json!({"values":values,"output":output.borrow().clone(),"lua_bytes":lua.used_memory()}),
            )
        }
        "stats" => Ok(
            json!({"lua_bytes":lua.used_memory(),"lua_limit":LUA_MEMORY,"native_buffer_bytes":ctx.native_bytes(),"tasks":ctx.tasks.borrow().len(),"paused":ctx.paused.get(),"connections":ctx.connection_count()}),
        ),
        "tasks" => Ok(ctx.task_info()),
        "pause" => {
            ctx.paused.set(true);
            Ok(json!({"paused":true}))
        }
        "resume" => {
            ctx.paused.set(false);
            ctx.wake.notify_waiters();
            Ok(json!({"paused":false}))
        }
        "cancel" => Ok(
            json!({"cancelled":scheduler::cancel(ctx,op["task"].as_u64().ok_or_else(||anyhow!("missing task"))?)}),
        ),
        "snapshot" => Ok(if op["metadata"] == true {
            json!({"values":crate::extras::snapshot(ctx),"metadata":crate::extras::metadata(ctx)})
        } else {
            crate::extras::snapshot(ctx)
        }),
        "stop" => {
            let result = crate::extras::lifecycle(lua, ctx, "stop", cancel).await;
            ctx.cleanup();
            ctx.settle_resources().await;
            result?;
            Ok(json!({"stopped":true}))
        }
        "start" => {
            crate::extras::start(lua, ctx, cancel).await?;
            ctx.extension.borrow_mut().running = true;
            Ok(json!({"running":true}))
        }
        "lifecycle" => Ok(
            json!({"running":ctx.extension.borrow().running,"hooks":ctx.extension.borrow().hooks.keys().collect::<Vec<_>>()}),
        ),
        "device_snapshot" => Ok(crate::device_state::snapshot(ctx)),
        "actions" => Ok(crate::extras::actions(ctx)),
        "invoke" => crate::extras::invoke(lua, ctx, op, cancel).await,
        "ui_event" => {
            let request = crate::ui::event_request(ctx, op)?;
            crate::extras::invoke(lua, ctx, &request, cancel).await
        }
        "ui_dismiss" => crate::ui::dismiss(ctx, op),
        "config" => Ok(ctx.extension.borrow().config.clone()),
        "config_description" => {
            let e = ctx.extension.borrow();
            Ok(json!({"schema":e.config_schema,"value":e.config}))
        }
        "configure" => {
            if let Some(expected) = op.get("schema") {
                anyhow::ensure!(
                    ctx.extension.borrow().config_schema.as_ref() == Some(expected),
                    "configuration fields changed; reopen configuration"
                );
            }
            crate::extras::configure(lua, ctx, op["value"].clone(), cancel).await?;
            Ok(ctx.extension.borrow().config.clone())
        }
        _ => bail!("unknown instance operation"),
    }
}
