use crate::{bindings::Context, error::Fault};
use mlua::{Function, HookTriggers, Lua, LuaSerdeExt, MultiValue, Table, VmState};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    future::Future,
    rc::Rc,
    task::Poll,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

pub struct Task {
    pub cancel: CancellationToken,
    pub handle: Option<tokio::task::JoinHandle<()>>,
    pub status: String,
    pub interval: Option<u64>,
    pub runs: u64,
    pub error: Option<Value>,
    pub result: Option<Value>,
    pub done: Rc<tokio::sync::Notify>,
}
impl Task {
    pub fn info(&self, id: u64) -> Value {
        json!({"id":id,"interval_ms":self.interval,"runs":self.runs,"status":self.status,"error":self.error,"result":self.result})
    }
}
#[derive(Debug)]
struct CpuExceeded;

tokio::task_local! {
    pub static EXECUTION: CancellationToken;
    pub static OUTPUT: Rc<std::cell::RefCell<Vec<String>>>;
}

pub fn current_cancel() -> CancellationToken {
    EXECUTION
        .try_with(CancellationToken::child_token)
        .unwrap_or_default()
}

pub fn record_output(text: String) -> mlua::Result<()> {
    if text.len() > 4096 {
        return Err(Fault::new("resource_limit", "print", "print budget exceeded").lua());
    }
    OUTPUT
        .try_with(|output| {
            let mut output = output.borrow_mut();
            if text.len() > 4096
                || output.len() >= 64
                || output.iter().map(String::len).sum::<usize>() + text.len() > 16384
            {
                return Err(Fault::new("resource_limit", "print", "print budget exceeded").lua());
            }
            output.push(text);
            Ok(())
        })
        .unwrap_or(Ok(()))
}

/// CPU budget is accumulated only while Lua/future polling actually executes.
/// Waiting on sleep, GATT, input or a queue does not consume the CPU allowance.
pub async fn execute(
    lua: &Lua,
    function: Function,
    args: MultiValue,
    wall_ms: u64,
    cpu_ms: u64,
    cancel: CancellationToken,
) -> mlua::Result<MultiValue> {
    let execution = cancel.child_token();
    let _cancel_on_exit = execution.clone().drop_guard();
    let thread = lua.create_thread(function)?;
    struct Reset {
        thread: mlua::Thread,
        noop: Function,
        lua: Lua,
    }
    impl Drop for Reset {
        fn drop(&mut self) {
            self.thread.remove_hook();
            let _ = self.thread.reset(self.noop.clone());
            let _ = self.lua.gc_collect();
        }
    }
    let _reset = Reset {
        thread: thread.clone(),
        noop: lua.create_function(|_, ()| Ok(()))?,
        lua: lua.clone(),
    };

    let slice_deadline = Rc::new(Cell::new(Instant::now()));
    let hook_deadline = slice_deadline.clone();
    thread.set_hook(
        HookTriggers::new().every_nth_instruction(10000),
        move |_, _| {
            if Instant::now() >= hook_deadline.get() {
                std::panic::resume_unwind(Box::new(CpuExceeded));
            }
            Ok(VmState::Yield)
        },
    )?;
    let mut future = Box::pin(thread.into_async::<MultiValue>(args)?);
    let mut used = Duration::ZERO;
    let allowance = Duration::from_millis(cpu_ms.clamp(1, 2000));
    let work = futures::future::poll_fn(move |cx| {
        if used >= allowance {
            return Poll::Ready(Err(Fault::new(
                "cpu_budget",
                "lua.execute",
                "Lua CPU budget exceeded",
            )
            .lua()));
        }
        let start = Instant::now();
        slice_deadline.set(start + (allowance - used));
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| future.as_mut().poll(cx)));
        used += start.elapsed();
        match result {
            Err(p) if p.is::<CpuExceeded>() => Poll::Ready(Err(Fault::new(
                "cpu_budget",
                "lua.execute",
                "Lua CPU budget exceeded",
            )
            .lua())),
            Err(p) => std::panic::resume_unwind(p),
            Ok(_) if used > allowance => Poll::Ready(Err(Fault::new(
                "cpu_budget",
                "lua.execute",
                "native execution exceeded CPU budget",
            )
            .lua())),
            Ok(result) => result,
        }
    });
    let output = OUTPUT.try_with(Clone::clone).unwrap_or_default();
    let work = EXECUTION.scope(execution, OUTPUT.scope(output, work));
    tokio::select! {
        biased;
        _=cancel.cancelled()=>Err(Fault::new("cancelled","lua.execute","execution cancelled; existing side effects are not rolled back").lua()),
        result=tokio::time::timeout(Duration::from_millis(wall_ms.clamp(1,120000)),work)=>result.unwrap_or_else(|_|Err(Fault::new("timeout","lua.execute","execution deadline exceeded; existing side effects are not rolled back").lua())),
    }
}

pub fn spawn(
    lua: &Lua,
    ctx: &Context,
    function: Function,
    interval: Option<u64>,
    wall: u64,
    cpu: u64,
) -> mlua::Result<u64> {
    let mut tasks = ctx.tasks.borrow_mut();
    if tasks
        .values()
        .filter(|t| matches!(t.status.as_str(), "waiting" | "running"))
        .count()
        >= 32
    {
        return Err(Fault::new("resource_limit", "runtime.spawn", "maximum 32 active tasks").lua());
    }
    if tasks.len() >= 64 {
        let removable = tasks
            .iter()
            .find(|(_, t)| !matches!(t.status.as_str(), "waiting" | "running"))
            .map(|(id, _)| *id);
        if let Some(id) = removable {
            tasks.remove(&id);
        }
    }
    let id = ctx.next_task.get();
    ctx.next_task.set(id + 1);
    let token = CancellationToken::new();
    tasks.insert(
        id,
        Task {
            cancel: token.clone(),
            handle: None,
            status: "waiting".into(),
            interval,
            runs: 0,
            error: None,
            result: None,
            done: Rc::new(tokio::sync::Notify::new()),
        },
    );
    drop(tasks);
    let (lua, context) = (lua.clone(), ctx.clone());
    let handle = tokio::task::spawn_local(async move {
        loop {
            if let Some(ms) = interval {
                tokio::select! {_=token.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(ms))=>{}}
            }
            while context.paused.get() {
                tokio::select! {_=token.cancelled()=>return,_=context.wake.notified()=>{}}
            }
            if token.is_cancelled() {
                break;
            }
            if let Some(t) = context.tasks.borrow_mut().get_mut(&id) {
                t.status = "running".into();
            }
            let result = execute(
                &lua,
                function.clone(),
                MultiValue::new(),
                wall,
                cpu,
                token.clone(),
            )
            .await;
            let result = result.and_then(|values| {
                let values = values
                    .into_iter()
                    .map(|v| crate::bindings::to_json(v, 0))
                    .collect::<mlua::Result<Vec<_>>>()?;
                let value = json!(values);
                if value.to_string().len() > 65536 {
                    return Err(Fault::new(
                        "resource_limit",
                        "task.result",
                        "result exceeds 64 KiB",
                    )
                    .lua());
                }
                Ok(value)
            });
            let mut tasks = context.tasks.borrow_mut();
            let Some(task) = tasks.get_mut(&id) else {
                return;
            };
            task.runs += 1;
            match result {
                Ok(value) => {
                    task.result = Some(value);
                    task.status = if interval.is_some() {
                        "waiting"
                    } else {
                        "completed"
                    }
                    .into();
                }
                Err(error) => {
                    let info = json!(crate::error::detail(&error));
                    task.error = Some(info.clone());
                    task.status = if token.is_cancelled() {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                    context.emit("task_error", json!({"task":id,"error":info}));
                }
            }
            task.done.notify_waiters();
            if task.error.is_some() || interval.is_none() {
                break;
            }
        }
    });
    ctx.tasks.borrow_mut().get_mut(&id).unwrap().handle = Some(handle);
    Ok(id)
}
pub fn cancel(ctx: &Context, id: u64) -> bool {
    let mut tasks = ctx.tasks.borrow_mut();
    if let Some(task) = tasks.get_mut(&id) {
        if !matches!(task.status.as_str(), "waiting" | "running") {
            return false;
        }
        task.cancel.cancel();
        if let Some(handle) = task.handle.take() {
            handle.abort();
        }
        task.status = "cancelled".into();
        task.done.notify_waiters();
        true
    } else {
        false
    }
}
async fn protected(
    lua: &Lua,
    f: Function,
    args: MultiValue,
    handler: Option<Function>,
) -> mlua::Result<MultiValue> {
    let result = Box::pin(execute(lua, f, args, 120000, 500, current_cancel())).await;
    match result {
        Ok(mut values) => {
            values.push_front(mlua::Value::Boolean(true));
            Ok(values)
        }
        Err(e) => {
            if crate::error::detail(&e).code == "cpu_budget" {
                std::panic::resume_unwind(Box::new(CpuExceeded));
            }
            let error = mlua::Value::Error(Box::new(e));
            let error = if let Some(handler) = handler {
                let values = Box::pin(execute(
                    lua,
                    handler,
                    MultiValue::from_vec(vec![error]),
                    2000,
                    100,
                    CancellationToken::new(),
                ))
                .await?;
                values.front().cloned().unwrap_or(mlua::Value::Nil)
            } else {
                error
            };
            Ok(MultiValue::from_vec(vec![
                mlua::Value::Boolean(false),
                error,
            ]))
        }
    }
}

pub fn install(lua: &Lua, ctx: &Context) -> mlua::Result<()> {
    // mlua's panic-safe C pcall cannot yield. Use budgeted child executions so
    // protected asynchronous IO works without exposing unrestricted coroutines.
    lua.globals().set(
        "pcall",
        lua.create_async_function(|lua, (f, args): (Function, MultiValue)| async move {
            protected(&lua, f, args, None).await
        })?,
    )?;
    lua.globals().set(
        "xpcall",
        lua.create_async_function(
            |lua, (f, handler, args): (Function, Function, MultiValue)| async move {
                protected(&lua, f, args, Some(handler)).await
            },
        )?,
    )?;

    let runtime = lua.create_table()?;
    let c = ctx.clone();
    runtime.set(
        "spawn",
        lua.create_function(move |lua, (function, opts): (Function, Option<Table>)| {
            let wall = opts
                .as_ref()
                .map(|o| o.get::<Option<u64>>("timeout_ms"))
                .transpose()?
                .flatten()
                .unwrap_or(120000);
            let cpu = opts
                .as_ref()
                .map(|o| o.get::<Option<u64>>("cpu_ms"))
                .transpose()?
                .flatten()
                .unwrap_or(500);
            spawn(lua, &c, function, None, wall, cpu)
        })?,
    )?;
    let c = ctx.clone();
    runtime.set(
        "every",
        lua.create_function(
            move |lua, (interval, function, opts): (u64, Function, Option<Table>)| {
                if !(100..=86400000).contains(&interval) {
                    return Err(Fault::new(
                        "invalid_argument",
                        "runtime.every",
                        "interval requires 100..86400000 ms",
                    )
                    .lua());
                }
                let wall = opts
                    .as_ref()
                    .map(|o| o.get::<Option<u64>>("timeout_ms"))
                    .transpose()?
                    .flatten()
                    .unwrap_or(30000);
                let cpu = opts
                    .as_ref()
                    .map(|o| o.get::<Option<u64>>("cpu_ms"))
                    .transpose()?
                    .flatten()
                    .unwrap_or(500);
                spawn(lua, &c, function, Some(interval), wall, cpu)
            },
        )?,
    )?;
    let c = ctx.clone();
    runtime.set(
        "cancel",
        lua.create_function(move |_, id: u64| Ok(cancel(&c, id)))?,
    )?;
    let c = ctx.clone();
    runtime.set(
        "tasks",
        lua.create_function(move |lua, ()| lua.to_value(&c.task_info()))?,
    )?;
    let c = ctx.clone();
    runtime.set(
        "join",
        lua.create_async_function(move |lua, (id, timeout): (u64, Option<u64>)| {
            let c = c.clone();
            async move {
                let wait = async {
                    loop {
                        let (info, done, finished) = {
                            let tasks = c.tasks.borrow();
                            let t = tasks.get(&id).ok_or_else(|| {
                                Fault::new("not_found", "runtime.join", "unknown task").lua()
                            })?;
                            (
                                t.info(id),
                                t.done.clone(),
                                !matches!(t.status.as_str(), "waiting" | "running"),
                            )
                        };
                        if finished {
                            return Ok::<Value, mlua::Error>(info);
                        }
                        done.notified().await;
                    }
                };
                let result = tokio::time::timeout(
                    Duration::from_millis(timeout.unwrap_or(30000).min(120000)),
                    wait,
                )
                .await
                .map_err(|_| {
                    Fault::new("timeout", "runtime.join", "task is still running").lua()
                })??;
                lua.to_value(&result)
            }
        })?,
    )?;
    runtime.set(
        "sleep",
        lua.create_async_function(|_, ms: u64| async move {
            if ms > 120000 {
                return Err(Fault::new(
                    "invalid_argument",
                    "runtime.sleep",
                    "sleep exceeds 120000 ms",
                )
                .lua());
            }
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(())
        })?,
    )?;
    runtime.set(
        "yield",
        lua.create_async_function(|_, ()| async {
            tokio::task::yield_now().await;
            Ok(())
        })?,
    )?;
    let start = ctx.started;
    runtime.set(
        "now_ms",
        lua.create_function(move |_, ()| Ok(start.elapsed().as_millis() as u64))?,
    )?;
    lua.globals().set("runtime", runtime)
}
